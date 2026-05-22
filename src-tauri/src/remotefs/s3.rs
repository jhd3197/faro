use super::{Capabilities, DirEntry, FileKind, RemoteFs};
use crate::session::S3Session;
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use futures::StreamExt;
use object_store::path::Path as ObjPath;
use object_store::ObjectStore;
use std::sync::Arc;

pub struct S3Fs {
    session: Arc<S3Session>,
}

impl S3Fs {
    pub fn new(session: Arc<S3Session>) -> Self {
        Self { session }
    }
}

/// Strip leading/trailing slashes and the legacy "." prefix. object_store
/// paths are POSIX with no leading slash. We accept "/" or "" as the bucket
/// root.
fn normalize_prefix(raw: &str) -> String {
    let trimmed = raw.trim().trim_matches('/');
    if trimmed.is_empty() || trimmed == "." {
        String::new()
    } else {
        trimmed.to_string()
    }
}

/// Map an object_store key into the display name + DirEntry path the UI uses.
fn entry_for_object(key: &str, size: u64, modified_secs: Option<i64>) -> DirEntry {
    let name = key.rsplit('/').next().unwrap_or(key).to_string();
    DirEntry {
        name,
        path: format!("/{key}"),
        kind: FileKind::File,
        size,
        modified: modified_secs,
        mode: None,
    }
}

fn entry_for_prefix(prefix: &str) -> DirEntry {
    let trimmed = prefix.trim_end_matches('/');
    let name = trimmed.rsplit('/').next().unwrap_or(trimmed).to_string();
    DirEntry {
        name,
        path: format!("/{trimmed}"),
        kind: FileKind::Directory,
        size: 0,
        modified: None,
        mode: None,
    }
}

#[async_trait]
impl RemoteFs for S3Fs {
    async fn list_dir(&self, path: &str) -> Result<Vec<DirEntry>> {
        let prefix = normalize_prefix(path);
        // object_store wants the prefix as ObjPath; for the bucket root pass
        // None. With a delimiter, list returns one level of objects+prefixes.
        let prefix_path = if prefix.is_empty() {
            None
        } else {
            Some(ObjPath::from(prefix.as_str()))
        };

        let listing = self
            .session
            .store
            .list_with_delimiter(prefix_path.as_ref())
            .await
            .with_context(|| format!("list s3://{}/{}", self.session.bucket, prefix))?;

        let mut out = Vec::with_capacity(listing.objects.len() + listing.common_prefixes.len());
        for cp in listing.common_prefixes {
            out.push(entry_for_prefix(cp.as_ref()));
        }
        for obj in listing.objects {
            let modified = obj.last_modified.timestamp().into();
            out.push(entry_for_object(
                obj.location.as_ref(),
                obj.size as u64,
                Some(modified),
            ));
        }
        Ok(out)
    }

    async fn rename(&self, from: &str, to: &str) -> Result<()> {
        let src = ObjPath::from(normalize_prefix(from));
        let dst = ObjPath::from(normalize_prefix(to));
        self.session
            .store
            .rename(&src, &dst)
            .await
            .with_context(|| format!("s3 rename {src} -> {dst}"))?;
        Ok(())
    }

    async fn delete(&self, path: &str, recursive: bool) -> Result<()> {
        let key = normalize_prefix(path);
        let target = ObjPath::from(key.as_str());

        // Probe whether `target` is itself an object. If head fails, treat it
        // as a prefix-only "folder" — delete everything under it.
        let is_object = self.session.store.head(&target).await.is_ok();

        if is_object && !recursive {
            self.session
                .store
                .delete(&target)
                .await
                .with_context(|| format!("s3 delete {target}"))?;
            return Ok(());
        }

        if is_object && recursive {
            // A file passed with recursive=true: just delete the file.
            self.session
                .store
                .delete(&target)
                .await
                .with_context(|| format!("s3 delete {target}"))?;
            return Ok(());
        }

        // Prefix delete: stream the list and delete each key.
        if !recursive {
            return Err(anyhow!(
                "{target} is a prefix; pass recursive=true to remove all objects under it"
            ));
        }
        let mut stream = self
            .session
            .store
            .list(Some(&target));
        while let Some(meta) = stream.next().await {
            let meta = meta.with_context(|| format!("list under {target}"))?;
            self.session
                .store
                .delete(&meta.location)
                .await
                .with_context(|| format!("s3 delete {}", meta.location))?;
        }
        Ok(())
    }

    async fn create_dir(&self, path: &str) -> Result<()> {
        // Object stores have no folders. Some clients write a zero-byte
        // marker object ending in "/" so the prefix shows up in flat
        // listings — but most modern S3 browsers don't, and listing with a
        // delimiter naturally surfaces sub-prefixes only when objects exist
        // beneath them. We silently accept the call so the UI's "mkdir"
        // flow doesn't error; the prefix becomes visible the moment the
        // first object is uploaded into it.
        let _ = path;
        Ok(())
    }

    async fn chmod(&self, _path: &str, _mode: u32) -> Result<()> {
        Err(anyhow!("S3 has no POSIX permissions"))
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            can_chmod: false,
            can_symlink: false,
            can_rename: true, // implemented as copy + delete by object_store
            has_directories: false,
        }
    }
}

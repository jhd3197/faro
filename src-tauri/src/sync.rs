use crate::remotefs::{FileKind, RemoteFs};
use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Direction of a one-way sync. Bidirectional is intentionally out of scope —
/// it needs conflict resolution UI that's a project of its own.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SyncDirection {
    LocalToRemote,
    RemoteToLocal,
}

/// How aggressive the sync is. Additive only copies missing/newer files;
/// Mirror also deletes files on the destination that don't exist on the
/// source. We default to Additive in the UI since Mirror is destructive.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SyncStrategy {
    Additive,
    Mirror,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SyncReason {
    Missing,     // not present on destination
    Newer,       // source mtime > destination mtime
    SizeChanged, // same name but bytes differ
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncFile {
    pub relative: String,
    pub source_path: String,
    pub destination_path: String,
    pub size: u64,
    pub reason: SyncReason,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncDelete {
    pub relative: String,
    pub path: String,
    pub kind: FileKind,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncPlan {
    pub direction: SyncDirection,
    pub strategy: SyncStrategy,
    pub local_root: String,
    pub remote_root: String,
    pub copies: Vec<SyncFile>,
    pub deletes: Vec<SyncDelete>,
    pub total_bytes: u64,
}

fn join(base: &str, rel: &str) -> String {
    let base = base.trim_end_matches('/').trim_end_matches('\\');
    if base.is_empty() {
        return rel.to_string();
    }
    // For local Windows paths, mirror the backslash separator. Everywhere
    // else POSIX `/` is fine.
    if base.len() >= 2 && base.as_bytes()[1] == b':' {
        format!("{base}\\{}", rel.replace('/', "\\"))
    } else {
        format!("{base}/{rel}")
    }
}

pub async fn plan(
    local_fs: &dyn RemoteFs,
    remote_fs: &dyn RemoteFs,
    local_root: &str,
    remote_root: &str,
    direction: SyncDirection,
    strategy: SyncStrategy,
) -> Result<SyncPlan> {
    let local_tree = crate::scan::walk_tree(local_fs, local_root).await?;
    let remote_tree = crate::scan::walk_tree(remote_fs, remote_root).await?;

    let (source, dest, source_root, dest_root) = match direction {
        SyncDirection::LocalToRemote => (&local_tree, &remote_tree, local_root, remote_root),
        SyncDirection::RemoteToLocal => (&remote_tree, &local_tree, remote_root, local_root),
    };

    let mut copies: Vec<SyncFile> = Vec::new();
    let mut total_bytes: u64 = 0;
    for (rel, src) in &source.files {
        let reason = match dest.files.get(rel) {
            None => Some(SyncReason::Missing),
            Some(d) if src.size != d.size => Some(SyncReason::SizeChanged),
            Some(d) if src.modified > d.modified && src.modified > 0 && d.modified > 0 => {
                Some(SyncReason::Newer)
            }
            _ => None,
        };
        if let Some(reason) = reason {
            total_bytes = total_bytes.saturating_add(src.size);
            copies.push(SyncFile {
                relative: rel.clone(),
                source_path: src.absolute.clone(),
                destination_path: join(dest_root, rel),
                size: src.size,
                reason,
            });
        }
    }

    let mut deletes: Vec<SyncDelete> = Vec::new();
    if matches!(strategy, SyncStrategy::Mirror) {
        for (rel, d) in &dest.files {
            if !source.files.contains_key(rel) {
                deletes.push(SyncDelete {
                    relative: rel.clone(),
                    path: d.absolute.clone(),
                    kind: FileKind::File,
                    size: d.size,
                });
            }
        }
    }

    let _ = source_root;
    Ok(SyncPlan {
        direction,
        strategy,
        local_root: local_root.to_string(),
        remote_root: remote_root.to_string(),
        copies,
        deletes,
        total_bytes,
    })
}

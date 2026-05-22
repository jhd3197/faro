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

/// In-memory representation of a flattened directory tree, keyed by the
/// relative path from the walk root. We only track files — directory entries
/// are implicit (we create destination directories on demand during execute).
struct Tree {
    files: std::collections::BTreeMap<String, FileEntry>,
}

struct FileEntry {
    absolute: String,
    size: u64,
    modified: i64,
}

async fn walk(fs: &dyn RemoteFs, root: &str) -> Result<Tree> {
    let mut tree = Tree {
        files: Default::default(),
    };
    let mut stack: Vec<String> = vec![root.to_string()];
    let normalized_root = root.trim_end_matches('/').to_string();

    while let Some(dir) = stack.pop() {
        let entries = match fs.list_dir(&dir).await {
            Ok(e) => e,
            Err(_) => continue, // tolerate unreadable directories
        };
        for entry in entries {
            // Skip symlinks — we don't want sync to follow them into cycles
            // or accidentally copy host-system pointers.
            match entry.kind {
                FileKind::Directory => {
                    stack.push(entry.path.clone());
                }
                FileKind::File => {
                    let rel = relative_of(&normalized_root, &entry.path);
                    tree.files.insert(
                        rel,
                        FileEntry {
                            absolute: entry.path.clone(),
                            size: entry.size,
                            modified: entry.modified.unwrap_or(0),
                        },
                    );
                }
                _ => {}
            }
        }
    }
    Ok(tree)
}

fn relative_of(root: &str, full: &str) -> String {
    let stripped = full.strip_prefix(root).unwrap_or(full);
    stripped
        .trim_start_matches(|c| c == '/' || c == '\\')
        .replace('\\', "/")
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
    let local_tree = walk(local_fs, local_root).await?;
    let remote_tree = walk(remote_fs, remote_root).await?;

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

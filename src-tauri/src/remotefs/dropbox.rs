use super::{Capabilities, ChangeSignal, DirEntry, FileKind, RemoteFs};
use crate::remotefs::webdav::days_from_civil;
use crate::session::DropboxSession;
use anyhow::{Context, Result};
use async_trait::async_trait;
use std::sync::Arc;

/// RemoteFs over the Dropbox API v2. Dropbox is path-addressed, so it slots
/// straight into the trait: real folders, rename via `move_v2`, no chmod. The
/// per-file `rev` is a reliable change token (`ChangeSignal::Etag`).
pub struct DropboxFs {
    session: Arc<DropboxSession>,
}

impl DropboxFs {
    pub fn new(session: Arc<DropboxSession>) -> Self {
        Self { session }
    }
}

/// Translate a Faro path (`/a/b`, or `/` for root) into a Dropbox API path
/// (`""` for root, otherwise `/a/b`).
pub fn dropbox_api_path(faro_path: &str) -> String {
    let trimmed = faro_path.trim().trim_matches('/');
    if trimmed.is_empty() || trimmed == "." {
        String::new()
    } else {
        format!("/{trimmed}")
    }
}

#[async_trait]
impl RemoteFs for DropboxFs {
    async fn list_dir(&self, path: &str) -> Result<Vec<DirEntry>> {
        let dbx = dropbox_api_path(path);
        let mut out = Vec::new();

        let mut resp = self
            .session
            .rpc(
                "/2/files/list_folder",
                serde_json::json!({ "path": dbx, "recursive": false }),
            )
            .await?;

        loop {
            if let Some(entries) = resp.get("entries").and_then(|e| e.as_array()) {
                for e in entries {
                    if let Some(entry) = entry_from_json(e) {
                        out.push(entry);
                    }
                }
            }
            let has_more = resp
                .get("has_more")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if !has_more {
                break;
            }
            let cursor = resp
                .get("cursor")
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .to_string();
            resp = self
                .session
                .rpc(
                    "/2/files/list_folder/continue",
                    serde_json::json!({ "cursor": cursor }),
                )
                .await?;
        }
        Ok(out)
    }

    async fn rename(&self, from: &str, to: &str) -> Result<()> {
        self.session
            .rpc(
                "/2/files/move_v2",
                serde_json::json!({
                    "from_path": dropbox_api_path(from),
                    "to_path": dropbox_api_path(to),
                    "autorename": false,
                }),
            )
            .await
            .with_context(|| format!("move {from} -> {to}"))?;
        Ok(())
    }

    async fn delete(&self, path: &str, _recursive: bool) -> Result<()> {
        // delete_v2 removes a folder and its contents; the flag needs no handling.
        self.session
            .rpc(
                "/2/files/delete_v2",
                serde_json::json!({ "path": dropbox_api_path(path) }),
            )
            .await
            .with_context(|| format!("delete {path}"))?;
        Ok(())
    }

    async fn create_dir(&self, path: &str) -> Result<()> {
        self.session
            .rpc(
                "/2/files/create_folder_v2",
                serde_json::json!({ "path": dropbox_api_path(path), "autorename": false }),
            )
            .await
            .with_context(|| format!("create dir {path}"))?;
        Ok(())
    }

    async fn chmod(&self, _path: &str, _mode: u32) -> Result<()> {
        Err(anyhow::anyhow!("Dropbox has no POSIX permissions"))
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            can_chmod: false,
            can_symlink: false,
            can_rename: true,
            has_directories: true,
            has_shell: false,
            // Dropbox's per-file `rev` changes on every edit — a reliable token.
            change_signal: ChangeSignal::Etag,
        }
    }
}

/// Parse one `list_folder` entry into a DirEntry, or `None` for a shape we don't
/// recognize (e.g. a deleted tombstone).
fn entry_from_json(e: &serde_json::Value) -> Option<DirEntry> {
    let tag = e.get(".tag").and_then(|t| t.as_str())?;
    let kind = match tag {
        "folder" => FileKind::Directory,
        "file" => FileKind::File,
        _ => return None, // "deleted" or unknown
    };
    let name = e.get("name").and_then(|n| n.as_str())?.to_string();
    // path_display is the properly-cased server path; fall back to path_lower.
    let path = e
        .get("path_display")
        .or_else(|| e.get("path_lower"))
        .and_then(|p| p.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("/{name}"));
    let size = e.get("size").and_then(|s| s.as_u64()).unwrap_or(0);
    let modified = e
        .get("server_modified")
        .and_then(|m| m.as_str())
        .and_then(parse_iso8601);
    let etag = e
        .get("rev")
        .and_then(|r| r.as_str())
        .map(|s| s.to_string());
    Some(DirEntry {
        name,
        path,
        kind,
        size,
        modified,
        mode: None,
        etag,
    })
}

/// Parse a fixed-form ISO-8601 UTC timestamp (`2024-07-15T09:30:00Z`, as Dropbox
/// emits) into a Unix timestamp. Dependency-free — the format is fixed.
fn parse_iso8601(s: &str) -> Option<i64> {
    let s = s.trim();
    let (date, rest) = s.split_once('T')?;
    let time = rest.trim_end_matches('Z');
    let d: Vec<&str> = date.split('-').collect();
    let t: Vec<&str> = time.split(':').collect();
    if d.len() != 3 || t.len() != 3 {
        return None;
    }
    let year: i64 = d[0].parse().ok()?;
    let month: i64 = d[1].parse().ok()?;
    let day: i64 = d[2].parse().ok()?;
    let hh: i64 = t[0].parse().ok()?;
    let mm: i64 = t[1].parse().ok()?;
    // Seconds may carry a fractional part (…:30.5Z); take the integer part.
    let ss: i64 = t[2].split('.').next()?.parse().ok()?;
    Some(days_from_civil(year, month, day) * 86400 + hh * 3600 + mm * 60 + ss)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_path_mapping() {
        assert_eq!(dropbox_api_path("/"), "");
        assert_eq!(dropbox_api_path(""), "");
        assert_eq!(dropbox_api_path("."), "");
        assert_eq!(dropbox_api_path("/docs"), "/docs");
        assert_eq!(dropbox_api_path("docs/sub/"), "/docs/sub");
    }

    #[test]
    fn iso8601_epoch() {
        assert_eq!(parse_iso8601("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso8601("2024-07-15T09:30:00Z"), Some(1_721_035_800));
        assert_eq!(parse_iso8601("2024-07-15T09:30:00.5Z"), Some(1_721_035_800));
        assert_eq!(parse_iso8601("nope"), None);
    }

    #[test]
    fn parses_list_folder_entries() {
        let listing = serde_json::json!({
            "entries": [
                { ".tag": "folder", "name": "Photos",
                  "path_display": "/Photos", "path_lower": "/photos", "id": "id:1" },
                { ".tag": "file", "name": "report.pdf", "path_display": "/report.pdf",
                  "size": 2048, "server_modified": "2024-07-15T09:30:00Z",
                  "rev": "0158abc", "id": "id:2" },
                { ".tag": "deleted", "name": "gone.txt", "path_display": "/gone.txt" }
            ],
            "cursor": "AAA",
            "has_more": false
        });
        let entries: Vec<DirEntry> = listing["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(entry_from_json)
            .collect();
        // The "deleted" tombstone is dropped.
        assert_eq!(entries.len(), 2);

        let dir = entries.iter().find(|e| e.name == "Photos").unwrap();
        assert_eq!(dir.kind, FileKind::Directory);
        assert_eq!(dir.path, "/Photos");

        let file = entries.iter().find(|e| e.name == "report.pdf").unwrap();
        assert_eq!(file.kind, FileKind::File);
        assert_eq!(file.size, 2048);
        assert_eq!(file.etag.as_deref(), Some("0158abc"));
        assert_eq!(file.modified, Some(1_721_035_800));
    }
}

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub mod ftp;
pub mod local;
pub mod object;
pub mod sftp;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileKind {
    File,
    Directory,
    Symlink,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirEntry {
    pub name: String,
    pub path: String,
    pub kind: FileKind,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<u32>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    pub can_chmod: bool,
    pub can_symlink: bool,
    pub can_rename: bool,
    pub has_directories: bool,
    /// Backend runs shell commands (gates server-side archive + "open terminal
    /// here"). True for SSH; false for local/FTP/object stores.
    pub has_shell: bool,
}

/// Unified filesystem abstraction. Each backend (Local, SFTP, S3, FTP, ...)
/// implements this trait. The UI queries `capabilities()` to know what actions
/// to expose per-backend.
#[async_trait]
pub trait RemoteFs: Send + Sync {
    async fn list_dir(&self, path: &str) -> anyhow::Result<Vec<DirEntry>>;
    async fn rename(&self, from: &str, to: &str) -> anyhow::Result<()>;
    /// Delete a file. For a directory, `recursive=true` walks children first.
    async fn delete(&self, path: &str, recursive: bool) -> anyhow::Result<()>;
    async fn create_dir(&self, path: &str) -> anyhow::Result<()>;
    async fn chmod(&self, path: &str, mode: u32) -> anyhow::Result<()>;
    fn capabilities(&self) -> Capabilities;
}

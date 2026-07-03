//! The request/response message set exchanged over a [`crate::SecureChannel`].
//!
//! These mirror the operations Faro already performs against a remote server —
//! browse, read, write, transfer, exec — so the daemon can satisfy the app's
//! `RemoteFs`, transfer, and bridge-exec paths without a bespoke API per call
//! site. Field names are camelCase to match the rest of Faro's JSON.

use serde::{Deserialize, Serialize};

/// Protocol version, bumped on a breaking change to these types. Exchanged in
/// the [`Hello`] so a mismatched daemon/controller fails loudly instead of
/// misparsing a frame.
pub const PROTOCOL_VERSION: u32 = 1;

/// mDNS service type the daemon advertises and the controller browses.
pub const SERVICE_TYPE: &str = "_faro-agent._tcp.local.";

/// Kind of a directory entry — mirrors Faro's `remotefs::FileKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileKind {
    File,
    Directory,
    Symlink,
    Other,
}

/// One directory entry — mirrors Faro's `remotefs::DirEntry` so `AgentFs` can
/// hand these straight back to the UI.
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

/// Facts about the controlled machine, shown in Faro's UI + handed to the AI so
/// it knows which OS's commands to run.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemInfo {
    pub os: String,       // "windows" | "macos" | "linux"
    pub hostname: String,
    pub arch: String,
    pub shell: String,    // the shell exec uses: "powershell" | "sh" | ...
    pub username: String,
    pub home_dir: String,
    pub agentd_version: String,
}

/// Sent by the controller as the very first (post-handshake) message so the
/// daemon can reject an incompatible protocol version before doing any work.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hello {
    pub protocol_version: u32,
    /// Human label for the controller (its hostname), for the daemon's audit log.
    pub client_name: String,
}

/// A request from the controller to the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum Request {
    Ping,
    SystemInfo,
    ListDir {
        path: String,
    },
    Stat {
        path: String,
    },
    /// Read a whole (text) file, capped at `maxBytes`.
    ReadFile {
        path: String,
        max_bytes: u64,
    },
    /// Ranged read for streaming a download. `len == 0` reads to EOF from
    /// `offset` up to the daemon's per-chunk cap.
    ReadChunk {
        path: String,
        offset: u64,
        len: u64,
    },
    /// Ranged write for streaming an upload. `truncate` opens/creates the file
    /// fresh (first chunk); `done` closes it. `data` is base64.
    WriteChunk {
        path: String,
        offset: u64,
        data: String,
        truncate: bool,
        done: bool,
    },
    Delete {
        path: String,
        recursive: bool,
    },
    CreateDir {
        path: String,
    },
    Rename {
        from: String,
        to: String,
    },
    Chmod {
        path: String,
        mode: u32,
    },
    /// Run a native shell command, bounded by time and output size.
    Exec {
        command: String,
        timeout_ms: u64,
        max_bytes: u64,
    },
}

/// The daemon's reply. `Error` carries a human-readable message for any request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "camelCase")]
pub enum Response {
    Pong,
    SystemInfo(SystemInfo),
    Dir {
        entries: Vec<DirEntry>,
    },
    Stat {
        entry: DirEntry,
    },
    File {
        /// base64 of the (possibly truncated) file bytes.
        data: String,
        bytes: u64,
        truncated: bool,
    },
    Chunk {
        /// base64 of the bytes read; empty at EOF.
        data: String,
        /// True when this read reached end of file.
        eof: bool,
    },
    Written {
        bytes: u64,
    },
    Ok,
    Exec {
        stdout: String,
        stderr: String,
        exit_code: Option<i32>,
        truncated: bool,
        timed_out: bool,
    },
    /// The daemon refused or failed the request. `denied` distinguishes a policy
    /// refusal (the machine's owner disallowed this class of op) from an
    /// operational failure, so the controller can phrase it correctly.
    Error {
        message: String,
        #[serde(default)]
        denied: bool,
    },
}

impl Response {
    /// Convenience for daemon handlers: an operational failure.
    pub fn error(message: impl Into<String>) -> Self {
        Response::Error { message: message.into(), denied: false }
    }

    /// Convenience for daemon handlers: a policy refusal.
    pub fn denied(message: impl Into<String>) -> Self {
        Response::Error { message: message.into(), denied: true }
    }
}

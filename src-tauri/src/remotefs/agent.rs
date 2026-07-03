//! `RemoteFs` over a Faro Agent session — browse and mutate the filesystem of a
//! paired machine through the daemon. Each call is one request/response over the
//! session's encrypted channel; the daemon enforces its own write policy, so a
//! refusal surfaces here as an error the UI shows.

use super::{Capabilities, DirEntry, FileKind, RemoteFs};
use crate::session::AgentSession;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use faro_agent_proto::msg::{
    DirEntry as ProtoEntry, FileKind as ProtoKind, Request, Response,
};
use std::sync::Arc;

pub struct AgentFs {
    session: Arc<AgentSession>,
}

impl AgentFs {
    pub fn new(session: Arc<AgentSession>) -> Self {
        Self { session }
    }
}

/// Map the protocol's `DirEntry`/`FileKind` onto Faro's own (identical shape,
/// distinct types so the wire crate stays Tauri-free).
fn convert(e: ProtoEntry) -> DirEntry {
    let kind = match e.kind {
        ProtoKind::File => FileKind::File,
        ProtoKind::Directory => FileKind::Directory,
        ProtoKind::Symlink => FileKind::Symlink,
        ProtoKind::Other => FileKind::Other,
    };
    DirEntry {
        name: e.name,
        path: e.path,
        kind,
        size: e.size,
        modified: e.modified,
        mode: e.mode,
    }
}

/// Turn a daemon `Error`/unexpected response into an `anyhow` error, mentioning
/// a policy denial explicitly.
fn as_err(resp: Response, ctx: &str) -> anyhow::Error {
    match resp {
        Response::Error { message, denied } => {
            if denied {
                anyhow!("{ctx}: denied by the remote machine's policy — {message}")
            } else {
                anyhow!("{ctx}: {message}")
            }
        }
        other => anyhow!("{ctx}: unexpected reply {other:?}"),
    }
}

#[async_trait]
impl RemoteFs for AgentFs {
    async fn list_dir(&self, path: &str) -> Result<Vec<DirEntry>> {
        match self.session.request(Request::ListDir { path: path.to_string() }).await? {
            Response::Dir { entries } => Ok(entries.into_iter().map(convert).collect()),
            other => Err(as_err(other, &format!("list {path}"))),
        }
    }

    async fn rename(&self, from: &str, to: &str) -> Result<()> {
        match self
            .session
            .request(Request::Rename { from: from.to_string(), to: to.to_string() })
            .await?
        {
            Response::Ok => Ok(()),
            other => Err(as_err(other, &format!("rename {from} -> {to}"))),
        }
    }

    async fn delete(&self, path: &str, recursive: bool) -> Result<()> {
        match self
            .session
            .request(Request::Delete { path: path.to_string(), recursive })
            .await?
        {
            Response::Ok => Ok(()),
            other => Err(as_err(other, &format!("delete {path}"))),
        }
    }

    async fn create_dir(&self, path: &str) -> Result<()> {
        match self.session.request(Request::CreateDir { path: path.to_string() }).await? {
            Response::Ok => Ok(()),
            other => Err(as_err(other, &format!("mkdir {path}"))),
        }
    }

    async fn chmod(&self, path: &str, mode: u32) -> Result<()> {
        match self.session.request(Request::Chmod { path: path.to_string(), mode }).await? {
            Response::Ok => Ok(()),
            other => Err(as_err(other, &format!("chmod {path}"))),
        }
    }

    fn capabilities(&self) -> Capabilities {
        // A Faro Agent runs native shell commands — that's the whole point — so
        // `has_shell` is true. chmod only means something on a POSIX daemon.
        let is_unix = self.session.system_info.os != "windows";
        Capabilities {
            can_chmod: is_unix,
            can_symlink: false,
            can_rename: true,
            has_directories: true,
            has_shell: true,
        }
    }
}

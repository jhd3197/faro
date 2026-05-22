pub mod ftp;
pub mod object;
pub use ftp::{ftp_connect, FtpSession};
pub use object::{object_connect, ObjectSession};

use crate::known_hosts;
use crate::profiles::{AuthMethod, ConnectionProfile};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use russh::client;
use russh_keys::key;
use russh_sftp::client::SftpSession;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::sync::{oneshot, Mutex, OnceCell};
use uuid::Uuid;

// ---- Host-key verification plumbing ----
//
// The russh `Handler::check_server_key` callback runs on the connect path.
// When we encounter an unknown host we suspend that callback on a oneshot
// channel, emit `host://prompt` to the frontend, and resume when the user
// answers via the `respond_to_host_prompt` command.

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HostPromptKind {
    Unknown,
    Mismatch,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostPromptEvent {
    pub request_id: String,
    pub host: String,
    pub port: u16,
    pub key_type: String,
    pub fingerprint: String,
    pub stored_fingerprint: Option<String>,
    pub kind: HostPromptKind,
}

#[derive(Debug, Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HostDecision {
    /// Accept this session only. Do not persist to known_hosts.
    Accept,
    /// Accept and append to ~/.ssh/known_hosts.
    Trust,
    /// Refuse the connection.
    Reject,
}

#[derive(Default)]
pub struct HostPromptRegistry {
    pending: Mutex<HashMap<String, oneshot::Sender<HostDecision>>>,
}

impl HostPromptRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    async fn register(&self) -> (String, oneshot::Receiver<HostDecision>) {
        let id = Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id.clone(), tx);
        (id, rx)
    }

    pub async fn resolve(&self, request_id: &str, decision: HostDecision) -> Result<()> {
        let tx = self
            .pending
            .lock()
            .await
            .remove(request_id)
            .ok_or_else(|| anyhow!("no pending host prompt {request_id}"))?;
        tx.send(decision)
            .map_err(|_| anyhow!("host prompt receiver dropped"))?;
        Ok(())
    }
}

/// Decides what to do with a server key on connect. Implementations: the
/// GUI's `TauriHostKeyVerifier` (emits a Tauri event + awaits the user's
/// click via oneshot), and the CLI's `CliHostKeyVerifier` (prompts on
/// stdin/stdout). A no-op `AutoTrustVerifier` exists for scripted scenarios
/// where the operator has explicitly opted out of prompting.
#[async_trait]
pub trait HostKeyVerifier: Send + Sync {
    async fn decide(
        &self,
        host: &str,
        port: u16,
        key_type: &str,
        fingerprint: &str,
        stored_fingerprint: Option<&str>,
        kind: HostPromptKind,
    ) -> Result<HostDecision, russh::Error>;
}

/// GUI verifier. Emits `host://prompt` and waits for the
/// `respond_to_host_prompt` command to fire a oneshot.
pub struct TauriHostKeyVerifier {
    app: AppHandle,
    prompts: Arc<HostPromptRegistry>,
}

impl TauriHostKeyVerifier {
    pub fn new(app: AppHandle, prompts: Arc<HostPromptRegistry>) -> Self {
        Self { app, prompts }
    }
}

#[async_trait]
impl HostKeyVerifier for TauriHostKeyVerifier {
    async fn decide(
        &self,
        host: &str,
        port: u16,
        key_type: &str,
        fingerprint: &str,
        stored_fingerprint: Option<&str>,
        kind: HostPromptKind,
    ) -> Result<HostDecision, russh::Error> {
        let (request_id, rx) = self.prompts.register().await;
        let event = HostPromptEvent {
            request_id,
            host: host.to_string(),
            port,
            key_type: key_type.to_string(),
            fingerprint: fingerprint.to_string(),
            stored_fingerprint: stored_fingerprint.map(|s| s.to_string()),
            kind,
        };
        if self.app.emit("host://prompt", event).is_err() {
            return Err(russh::Error::HUP);
        }
        rx.await.map_err(|_| russh::Error::HUP)
    }
}

pub struct ClientHandler {
    host: String,
    port: u16,
    verifier: Arc<dyn HostKeyVerifier>,
}

#[async_trait]
impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &key::PublicKey,
    ) -> Result<bool, Self::Error> {
        let status = known_hosts::check(&self.host, self.port, server_public_key);
        let fingerprint = known_hosts::fingerprint(server_public_key);
        let key_type = server_public_key.name().to_string();

        let (kind, stored) = match status {
            known_hosts::HostKeyStatus::Match => return Ok(true),
            known_hosts::HostKeyStatus::Unknown => (HostPromptKind::Unknown, None),
            known_hosts::HostKeyStatus::Mismatch { stored_fingerprint } => {
                (HostPromptKind::Mismatch, Some(stored_fingerprint))
            }
        };

        let decision = self
            .verifier
            .decide(
                &self.host,
                self.port,
                &key_type,
                &fingerprint,
                stored.as_deref(),
                kind,
            )
            .await?;

        match decision {
            HostDecision::Accept => Ok(true),
            HostDecision::Trust => {
                if let Err(e) = known_hosts::append(&self.host, self.port, server_public_key) {
                    tracing::warn!(?e, "failed to persist host key");
                }
                Ok(true)
            }
            HostDecision::Reject => Ok(false),
        }
    }
}

// ---- SshSession ----
//
// One TCP/SSH connection, shared between the SFTP browser and any number of
// terminal channels. This is the killer feature: single auth, two surfaces.
pub struct SshSession {
    pub id: String,
    pub profile: ConnectionProfile,
    pub handle: Mutex<client::Handle<ClientHandler>>,
    sftp: OnceCell<Mutex<SftpSession>>,
}

impl SshSession {
    /// Wrap an already-opened russh client handle and a profile into a
    /// freshly-uuid'd session. Used by `SessionManager` and the CLI binary
    /// (where there's no manager — the CLI builds one session per command
    /// and drops it on exit).
    pub fn new(profile: ConnectionProfile, handle: client::Handle<ClientHandler>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            profile,
            handle: Mutex::new(handle),
            sftp: OnceCell::new(),
        }
    }

    /// Lazily open the SFTP subsystem on first use; the channel sticks around
    /// for the life of the session.
    pub async fn ensure_sftp(&self) -> Result<&Mutex<SftpSession>> {
        self.sftp
            .get_or_try_init(|| async {
                let channel = {
                    let h = self.handle.lock().await;
                    h.channel_open_session().await?
                };
                channel.request_subsystem(true, "sftp").await?;
                let sftp = SftpSession::new(channel.into_stream()).await?;
                Ok::<Mutex<SftpSession>, anyhow::Error>(Mutex::new(sftp))
            })
            .await
    }
}

/// Open a Session for any supported protocol given a profile and a verifier.
/// Used by the CLI and (transitively) the GUI's `SessionManager`. Doesn't
/// register the session anywhere — the caller owns its lifetime.
pub async fn open_session(
    profile: &ConnectionProfile,
    verifier: Arc<dyn HostKeyVerifier>,
) -> Result<Session> {
    match profile.protocol.as_str() {
        "sftp" | "ssh" | "" => {
            let handle = ssh_connect(profile, verifier).await?;
            let ssh = Arc::new(SshSession::new(profile.clone(), handle));
            Ok(Session::Ssh(ssh))
        }
        "ftp" | "ftps" => {
            let ftp = ftp_connect(profile).await?;
            Ok(Session::Ftp(Arc::new(ftp)))
        }
        "s3" | "azure" => {
            let obj = object_connect(profile).await?;
            Ok(Session::Object(Arc::new(obj)))
        }
        other => Err(anyhow!("unsupported protocol: {other}")),
    }
}

// ---- Connect ----

#[cfg(unix)]
async fn open_agent() -> Result<russh_keys::agent::client::AgentClient<tokio::net::UnixStream>> {
    russh_keys::agent::client::AgentClient::connect_env()
        .await
        .context("connecting to ssh-agent via $SSH_AUTH_SOCK")
}

#[cfg(windows)]
async fn open_agent(
) -> Result<russh_keys::agent::client::AgentClient<tokio::net::windows::named_pipe::NamedPipeClient>>
{
    // First try OpenSSH-for-Windows (shipped with Win10/11) which listens
    // on a fixed pipe when the 'OpenSSH Authentication Agent' service runs.
    const OPENSSH_PIPE: &str = r"\\.\pipe\openssh-ssh-agent";
    if let Ok(stream) =
        tokio::net::windows::named_pipe::ClientOptions::new().open(OPENSSH_PIPE)
    {
        return Ok(russh_keys::agent::client::AgentClient::connect(stream));
    }

    // Fall back to PuTTY Pageant 0.78+, which uses pipes like
    // `\\.\pipe\pageant.<user>.<random>`. The random suffix is per-launch,
    // so we enumerate `\\.\pipe\` and try each pageant.* entry. Older
    // Pageant (file-mapping IPC) is not supported.
    if let Some(pipe) = find_pageant_pipe() {
        if let Ok(stream) =
            tokio::net::windows::named_pipe::ClientOptions::new().open(&pipe)
        {
            return Ok(russh_keys::agent::client::AgentClient::connect(stream));
        }
    }

    Err(anyhow!(
        "no ssh-agent found on Windows. Tried OpenSSH ({OPENSSH_PIPE}) and Pageant pipes. \
         Start the 'OpenSSH Authentication Agent' service or launch Pageant (PuTTY 0.78+)."
    ))
}

#[cfg(windows)]
fn find_pageant_pipe() -> Option<std::ffi::OsString> {
    // Listing `\\.\pipe\` returns the names of every named pipe on the system.
    // The OS exposes them as a directory since Windows 7 / 10.
    let entries = std::fs::read_dir(r"\\.\pipe\").ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let s = name.to_string_lossy();
        if s.starts_with("pageant.") {
            // The full pipe path is `\\.\pipe\<name>`.
            let mut p = std::ffi::OsString::from(r"\\.\pipe\");
            p.push(&name);
            return Some(p);
        }
    }
    None
}

async fn authenticate_with_agent(
    session: &mut client::Handle<ClientHandler>,
    username: &str,
) -> Result<bool> {
    let mut agent = open_agent().await?;

    let identities = agent
        .request_identities()
        .await
        .context("listing agent identities")?;
    if identities.is_empty() {
        return Err(anyhow!(
            "ssh-agent has no identities loaded (run `ssh-add`)"
        ));
    }
    for id in identities {
        let (next_agent, authed) = session.authenticate_future(username, id, agent).await;
        agent = next_agent;
        match authed {
            Ok(true) => return Ok(true),
            Ok(false) => continue,
            Err(e) => {
                tracing::debug!(?e, "agent identity failed, trying next");
                continue;
            }
        }
    }
    Ok(false)
}

pub async fn ssh_connect(
    profile: &ConnectionProfile,
    verifier: Arc<dyn HostKeyVerifier>,
) -> Result<client::Handle<ClientHandler>> {
    let config = Arc::new(client::Config {
        inactivity_timeout: Some(std::time::Duration::from_secs(60 * 30)),
        ..Default::default()
    });
    let addr = (profile.host.as_str(), profile.port);
    let handler = ClientHandler {
        host: profile.host.clone(),
        port: profile.port,
        verifier,
    };
    let mut session = client::connect(config, addr, handler)
        .await
        .with_context(|| format!("connect to {}:{}", profile.host, profile.port))?;

    let authed = match &profile.auth {
        AuthMethod::Password { password } => session
            .authenticate_password(&profile.username, password)
            .await
            .context("password auth")?,
        AuthMethod::Key { path, passphrase } => {
            let key = russh_keys::load_secret_key(path, passphrase.as_deref())
                .with_context(|| format!("load key {path}"))?;
            session
                .authenticate_publickey(&profile.username, Arc::new(key))
                .await
                .context("publickey auth")?
        }
        AuthMethod::Agent => authenticate_with_agent(&mut session, &profile.username).await?,
    };

    if !authed {
        return Err(anyhow!("Authentication failed"));
    }
    Ok(session)
}

// ---- Session enum + SessionManager ----
//
// SessionManager keeps a single map of remote sessions regardless of
// protocol. Most commands speak through the `RemoteFs` trait so they don't
// care which variant they're talking to; commands that DO care (terminal
// open, host-key prompts) match explicitly.

pub enum Session {
    Ssh(Arc<SshSession>),
    Ftp(Arc<FtpSession>),
    Object(Arc<ObjectSession>),
}

impl Session {
    pub fn profile(&self) -> &ConnectionProfile {
        match self {
            Self::Ssh(s) => &s.profile,
            Self::Ftp(s) => &s.profile,
            Self::Object(s) => &s.profile,
        }
    }

    pub fn protocol(&self) -> &str {
        match self {
            Self::Ssh(_) => "sftp",
            Self::Ftp(_) => "ftp",
            Self::Object(s) => s.profile.protocol.as_str(),
        }
    }
}

pub struct SessionManager {
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    pub prompts: Arc<HostPromptRegistry>,
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            prompts: Arc::new(HostPromptRegistry::new()),
        }
    }

    pub async fn connect(
        &self,
        profile: ConnectionProfile,
        app: AppHandle,
    ) -> Result<String> {
        let verifier: Arc<dyn HostKeyVerifier> =
            Arc::new(TauriHostKeyVerifier::new(app.clone(), self.prompts.clone()));
        self.connect_with_verifier(profile, app, verifier).await
    }

    /// Used by the CLI binary, which plugs in its own stdin-based verifier
    /// instead of the Tauri event one. The `app` parameter is still required
    /// because the FTP/S3 connect paths don't use it, but the SSH path
    /// embeds the AppHandle elsewhere via `_ = app`.
    pub async fn connect_with_verifier(
        &self,
        profile: ConnectionProfile,
        app: AppHandle,
        verifier: Arc<dyn HostKeyVerifier>,
    ) -> Result<String> {
        let _ = app;
        let session = match profile.protocol.as_str() {
            "sftp" | "ssh" | "" => {
                let handle = ssh_connect(&profile, verifier).await?;
                let id = Uuid::new_v4().to_string();
                let ssh = Arc::new(SshSession {
                    id: id.clone(),
                    profile,
                    handle: Mutex::new(handle),
                    sftp: OnceCell::new(),
                });
                (id, Session::Ssh(ssh))
            }
            "ftp" | "ftps" => {
                let ftp = ftp_connect(&profile).await?;
                let id = ftp.id.clone();
                (id, Session::Ftp(Arc::new(ftp)))
            }
            "s3" | "azure" => {
                let _ = app; // Object stores have no per-connect UI prompts.
                let obj = object_connect(&profile).await?;
                let id = obj.id.clone();
                (id, Session::Object(Arc::new(obj)))
            }
            other => return Err(anyhow!("unsupported protocol: {other}")),
        };
        let (id, sess) = session;
        self.sessions
            .lock()
            .await
            .insert(id.clone(), Arc::new(sess));
        Ok(id)
    }

    pub async fn get(&self, id: &str) -> Option<Arc<Session>> {
        self.sessions.lock().await.get(id).cloned()
    }

    /// Convenience accessor when the caller specifically needs an SSH session
    /// (e.g. opening a PTY). Returns None if the session is FTP or missing.
    pub async fn get_ssh(&self, id: &str) -> Option<Arc<SshSession>> {
        match self.sessions.lock().await.get(id) {
            Some(s) => match &**s {
                Session::Ssh(ssh) => Some(ssh.clone()),
                _ => None,
            },
            None => None,
        }
    }

    pub async fn disconnect(&self, id: &str) -> Result<()> {
        let s = self.sessions.lock().await.remove(id);
        if let Some(s) = s {
            match &*s {
                Session::Ssh(ssh) => {
                    let h = ssh.handle.lock().await;
                    let _ = h
                        .disconnect(russh::Disconnect::ByApplication, "bye", "en")
                        .await;
                }
                Session::Ftp(ftp) => {
                    let _ = ftp
                        .with_stream(|s| {
                            s.quit();
                            Ok(())
                        })
                        .await;
                }
                Session::Object(_) => {
                    // Object stores are stateless HTTP — nothing to close.
                }
            }
        }
        Ok(())
    }
}

use crate::profiles::{AuthMethod, ConnectionProfile};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use russh::client;
use russh_keys::key;
use russh_sftp::client::SftpSession;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, OnceCell};
use uuid::Uuid;

// ---- russh client handler ----
//
// For v0.1 we accept any server key. v0.2 will add known_hosts verification.
pub struct ClientHandler;

#[async_trait]
impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &key::PublicKey,
    ) -> Result<bool, Self::Error> {
        Ok(true)
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

// ---- Connect ----

pub async fn ssh_connect(
    profile: &ConnectionProfile,
) -> Result<client::Handle<ClientHandler>> {
    let config = Arc::new(client::Config {
        inactivity_timeout: Some(std::time::Duration::from_secs(60 * 30)),
        ..Default::default()
    });
    let addr = (profile.host.as_str(), profile.port);
    let mut session = client::connect(config, addr, ClientHandler)
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
        AuthMethod::Agent => {
            return Err(anyhow!("SSH agent auth not implemented in v0.1"));
        }
    };

    if !authed {
        return Err(anyhow!("Authentication failed"));
    }
    Ok(session)
}

// ---- SessionManager ----

pub struct SessionManager {
    sessions: Mutex<HashMap<String, Arc<SshSession>>>,
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
        }
    }

    pub async fn connect(&self, profile: ConnectionProfile) -> Result<String> {
        let handle = ssh_connect(&profile).await?;
        let id = Uuid::new_v4().to_string();
        let session = Arc::new(SshSession {
            id: id.clone(),
            profile,
            handle: Mutex::new(handle),
            sftp: OnceCell::new(),
        });
        self.sessions.lock().await.insert(id.clone(), session);
        Ok(id)
    }

    pub async fn get(&self, id: &str) -> Option<Arc<SshSession>> {
        self.sessions.lock().await.get(id).cloned()
    }

    pub async fn disconnect(&self, id: &str) -> Result<()> {
        let s = self.sessions.lock().await.remove(id);
        if let Some(s) = s {
            let h = s.handle.lock().await;
            let _ = h
                .disconnect(russh::Disconnect::ByApplication, "bye", "en")
                .await;
        }
        Ok(())
    }
}

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Manager};
use tokio::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum AuthMethod {
    Password { password: String },
    Key {
        path: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        passphrase: Option<String>,
    },
    Agent,
    /// A reference to private-key material stored in the OS keychain (under
    /// `key_ref`, e.g. `grant-key:<profile-id>`), never on disk. The connect
    /// path resolves it via `credentials::get_secret` at connect time.
    #[serde(rename_all = "camelCase")]
    KeyRef { key_ref: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionProfile {
    pub id: String,
    pub name: String,
    pub protocol: String, // "sftp" | "ftp" | "ftps" | "s3"
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth: AuthMethod,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_remote_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    // Connect automatically on app launch. Optional so existing profile JSON
    // files (written before this field existed) keep loading as `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_connect: Option<bool>,
    // Object-store-specific fields. Unused for sftp/ftp protocols. We keep
    // them optional so existing profile JSON files keep loading.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bucket: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    // Azure Blob specific. `account` is the Azure storage account name,
    // and `bucket` doubles as the container. For S3, both stay absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    // Faro Agent (protocol "faro-agent") specific: the daemon's pinned static
    // public key (base64), learned at pairing time. Its presence means this
    // profile is paired; absence means the user still needs to pair with a code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_key: Option<String>,
    // Rail organisation. `group` is a free-form folder name (absent = ungrouped);
    // `sort_order` is the manual drag-and-drop position. Profiles without one
    // sort after ordered ones, by protocol/name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort_order: Option<u32>,
    // Single-hop bastion (ProxyJump). When `jump_host` is set, the SSH connect
    // path first connects there with the SAME auth material, then tunnels to
    // `host:port` over a direct-tcpip channel. `jump_port` defaults to 22,
    // `jump_username` defaults to `username`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jump_host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jump_port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jump_username: Option<String>,
}

// Plain JSON file in the app data dir. v0.2 moves secrets into the OS
// keychain and leaves only references here.
pub struct ProfileStore {
    path: PathBuf,
    inner: Arc<Mutex<Vec<ConnectionProfile>>>,
}

impl ProfileStore {
    pub fn load_or_create(app: &AppHandle) -> Result<Self> {
        let dir = app
            .path()
            .app_data_dir()
            .context("resolving app_data_dir")?;
        Self::from_dir(&dir)
    }

    /// Load (or initialise) the profile store from a known directory. Used by
    /// the CLI binary, which doesn't have a Tauri AppHandle. Resolves to the
    /// same on-disk path the GUI uses — see `default_data_dir`.
    pub fn from_dir(dir: &std::path::Path) -> Result<Self> {
        std::fs::create_dir_all(dir).ok();
        let path = dir.join("profiles.json");
        let profiles: Vec<ConnectionProfile> = if path.exists() {
            let bytes = std::fs::read(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            serde_json::from_slice(&bytes).unwrap_or_default()
        } else {
            Vec::new()
        };
        Ok(Self {
            path: path.clone(),
            inner: Arc::new(Mutex::new(profiles)),
        })
    }

    pub async fn list(&self) -> Result<Vec<ConnectionProfile>> {
        Ok(self.inner.lock().await.clone())
    }

    pub async fn get(&self, id: &str) -> Result<Option<ConnectionProfile>> {
        Ok(self.inner.lock().await.iter().find(|p| p.id == id).cloned())
    }

    pub async fn upsert(&self, profile: ConnectionProfile) -> Result<()> {
        let mut g = self.inner.lock().await;
        if let Some(existing) = g.iter_mut().find(|p| p.id == profile.id) {
            *existing = profile;
        } else {
            g.push(profile);
        }
        self.write(&g)?;
        Ok(())
    }

    /// Persist a manual rail order: each listed profile gets `sort_order` =
    /// its index. Ids not in the store are skipped; one write for the batch.
    pub async fn reorder(&self, ids: &[String]) -> Result<()> {
        let mut g = self.inner.lock().await;
        for (i, id) in ids.iter().enumerate() {
            if let Some(p) = g.iter_mut().find(|p| &p.id == id) {
                p.sort_order = Some(i as u32);
            }
        }
        self.write(&g)?;
        Ok(())
    }

    pub async fn delete(&self, id: &str) -> Result<()> {
        let mut g = self.inner.lock().await;
        g.retain(|p| p.id != id);
        self.write(&g)?;
        Ok(())
    }

    fn write(&self, profiles: &[ConnectionProfile]) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(profiles)?;
        std::fs::write(&self.path, bytes)
            .with_context(|| format!("writing {}", self.path.display()))?;
        Ok(())
    }
}

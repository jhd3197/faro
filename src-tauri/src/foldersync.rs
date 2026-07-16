//! Continuous one-way folder sync — the "attach a local folder to a remote and
//! keep it mirrored" feature (see `docs/plans/2_continuous-folder-sync.md`).
//!
//! Each **pair** binds a local folder to a remote root on a saved connection.
//! While enabled, a background task re-reconciles the pair whenever the local
//! folder changes (a `notify` watcher) or a poll interval elapses (the backstop
//! for remote-side drift, and the only trigger for `RemoteToLocal`). Because
//! object stores, SFTP, FTP, and the Faro Agent have no change feed, remote
//! detection is polling by design.
//!
//! The reconciler is deliberately thin: it reuses the existing stateless
//! [`crate::sync::plan`] (files-only diff by size + mtime) and
//! [`crate::commands::execute_sync_plan`] (chunked transfers via
//! `TransferManager`). One-way sync needs no persistent per-file index —
//! the source side is authoritative, so `plan` + `Mirror` deletes fully
//! describe the work. (A persistent index is only needed for bidirectional
//! conflict detection and same-size-edit detection; both are future work.)
//!
//! Structure mirrors `agent_host.rs`: a `Mutex`-guarded settings blob persisted
//! as JSON under the app data dir, a map of running tasks, `load` /
//! `auto_start_if_enabled` / per-pair `start` / `stop` / `status`, and
//! `foldersync://changed` events so the UI can refresh.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use notify::{RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::{mpsc, Mutex};

use crate::sync::{SyncDirection, SyncStrategy};
use crate::transfer::TransferStatus;
use crate::AppState;

/// Backstop poll cadence when a pair doesn't specify one.
const DEFAULT_POLL_SECS: u64 = 60;
/// Coalesce a burst of local FS events into one reconcile.
const DEBOUNCE: Duration = Duration::from_millis(700);
/// Give up waiting on a single reconcile's transfers after this long (they keep
/// running in the TransferManager; we just stop blocking the pair's status on
/// them so the next tick can proceed).
const TRANSFER_WAIT_CAP: Duration = Duration::from_secs(30 * 60);

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ---------- persisted config ----------

/// One configured sync pair. Persisted verbatim in `foldersync.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncPair {
    pub id: String,
    pub name: String,
    pub local_root: String,
    /// The saved connection this pair syncs against (`ConnectionProfile.id`).
    pub profile_id: String,
    pub remote_root: String,
    pub direction: SyncDirection,
    pub strategy: SyncStrategy,
    pub enabled: bool,
    #[serde(default = "default_poll_secs")]
    pub poll_interval_secs: u64,
}

fn default_poll_secs() -> u64 {
    DEFAULT_POLL_SECS
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Settings {
    pairs: Vec<SyncPair>,
}

// ---------- live status ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PairState {
    Idle,
    Scanning,
    Syncing,
    Error,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeStatus {
    state: PairState,
    /// Files still transferring in the current reconcile.
    in_flight: u32,
    /// ms epoch of the last successful reconcile, if any.
    last_synced: Option<i64>,
    last_error: Option<String>,
}

impl Default for RuntimeStatus {
    fn default() -> Self {
        Self { state: PairState::Idle, in_flight: 0, last_synced: None, last_error: None }
    }
}

/// A pair merged with its live runtime status — the shape the UI consumes.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairView {
    #[serde(flatten)]
    pair: SyncPair,
    running: bool,
    state: PairState,
    in_flight: u32,
    last_synced: Option<i64>,
    last_error: Option<String>,
}

// ---------- running task handle ----------

struct Running {
    task: tauri::async_runtime::JoinHandle<()>,
    /// Manual/`notify` reconcile trigger. Dropping every clone ends the loop.
    trigger: mpsc::UnboundedSender<()>,
    /// Kept alive so the filesystem watch stays registered.
    _watcher: Option<notify::RecommendedWatcher>,
    status: Arc<Mutex<RuntimeStatus>>,
}

pub struct FolderSync {
    settings_path: PathBuf,
    settings: Mutex<Settings>,
    running: Mutex<HashMap<String, Running>>,
}

impl FolderSync {
    pub fn load(app: &AppHandle) -> Result<Self> {
        let dir = app.path().app_data_dir().context("resolving app_data_dir")?;
        std::fs::create_dir_all(&dir).ok();
        let settings_path = dir.join("foldersync.json");
        let settings: Settings = std::fs::read(&settings_path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Ok(Self {
            settings_path,
            settings: Mutex::new(settings),
            running: Mutex::new(HashMap::new()),
        })
    }

    async fn persist(&self) -> Result<()> {
        let settings = self.settings.lock().await.clone();
        std::fs::write(&self.settings_path, serde_json::to_vec_pretty(&settings)?)
            .with_context(|| format!("write {}", self.settings_path.display()))?;
        Ok(())
    }

    /// Bring up every pair the user left enabled.
    pub async fn auto_start_if_enabled(&self, app: AppHandle) {
        let pairs = self.settings.lock().await.pairs.clone();
        for p in pairs.into_iter().filter(|p| p.enabled) {
            if let Err(e) = self.start_pair(&app, &p).await {
                tracing::warn!("folder sync '{}' failed to auto-start: {e:#}", p.name);
            }
        }
    }

    /// Merge config with live status for the UI.
    pub async fn views(&self) -> Vec<PairView> {
        let pairs = self.settings.lock().await.pairs.clone();
        let running = self.running.lock().await;
        let mut out = Vec::with_capacity(pairs.len());
        for pair in pairs {
            let (is_running, rt) = match running.get(&pair.id) {
                Some(r) => (true, r.status.lock().await.clone()),
                None => (false, RuntimeStatus::default()),
            };
            out.push(PairView {
                running: is_running,
                state: rt.state,
                in_flight: rt.in_flight,
                last_synced: rt.last_synced,
                last_error: rt.last_error,
                pair,
            });
        }
        out
    }

    /// Add or replace a pair. If it's enabled, (re)start it.
    pub async fn upsert(&self, app: &AppHandle, mut pair: SyncPair) -> Result<()> {
        if pair.id.is_empty() {
            pair.id = uuid::Uuid::new_v4().to_string();
        }
        if pair.poll_interval_secs == 0 {
            pair.poll_interval_secs = DEFAULT_POLL_SECS;
        }
        {
            let mut s = self.settings.lock().await;
            match s.pairs.iter_mut().find(|p| p.id == pair.id) {
                Some(existing) => *existing = pair.clone(),
                None => s.pairs.push(pair.clone()),
            }
        }
        self.persist().await?;
        self.stop_pair(&pair.id).await;
        if pair.enabled {
            self.start_pair(app, &pair).await?;
        }
        Ok(())
    }

    pub async fn remove(&self, id: &str) -> Result<()> {
        self.stop_pair(id).await;
        {
            let mut s = self.settings.lock().await;
            s.pairs.retain(|p| p.id != id);
        }
        self.persist().await
    }

    pub async fn set_enabled(&self, app: &AppHandle, id: &str, enabled: bool) -> Result<()> {
        let pair = {
            let mut s = self.settings.lock().await;
            let p = s.pairs.iter_mut().find(|p| p.id == id);
            match p {
                Some(p) => {
                    p.enabled = enabled;
                    p.clone()
                }
                None => anyhow::bail!("no such sync pair"),
            }
        };
        self.persist().await?;
        if enabled {
            self.stop_pair(id).await;
            self.start_pair(app, &pair).await?;
        } else {
            self.stop_pair(id).await;
        }
        Ok(())
    }

    /// Fire an immediate reconcile for a running pair.
    pub async fn sync_now(&self, id: &str) -> Result<()> {
        let running = self.running.lock().await;
        match running.get(id) {
            Some(r) => {
                let _ = r.trigger.send(());
                Ok(())
            }
            None => anyhow::bail!("sync pair is not enabled"),
        }
    }

    async fn stop_pair(&self, id: &str) {
        if let Some(r) = self.running.lock().await.remove(id) {
            r.task.abort();
            // Dropping `_watcher` unregisters the filesystem watch.
        }
    }

    async fn start_pair(&self, app: &AppHandle, pair: &SyncPair) -> Result<()> {
        let mut running = self.running.lock().await;
        if running.contains_key(&pair.id) {
            return Ok(());
        }

        let (trigger, mut trigger_rx) = mpsc::unbounded_channel::<()>();
        let status = Arc::new(Mutex::new(RuntimeStatus::default()));

        // Watch the local folder for LocalToRemote so edits sync promptly.
        // RemoteToLocal relies on the poll backstop (no remote change feed).
        let watcher = if matches!(pair.direction, SyncDirection::LocalToRemote) {
            match make_watcher(&pair.local_root, trigger.clone()) {
                Ok(w) => Some(w),
                Err(e) => {
                    tracing::warn!("folder sync '{}': watcher unavailable: {e:#}", pair.name);
                    None
                }
            }
        } else {
            None
        };

        let task_app = app.clone();
        let task_pair = pair.clone();
        let task_status = status.clone();
        let poll = Duration::from_secs(pair.poll_interval_secs.max(1));
        let task = tauri::async_runtime::spawn(async move {
            // A live session for this pair, connected lazily and rebuilt on error.
            let mut session_id: Option<String> = None;
            // Reconcile once immediately on start.
            let mut pending = true;
            loop {
                if !pending {
                    tokio::select! {
                        _ = tokio::time::sleep(poll) => {}
                        r = trigger_rx.recv() => {
                            if r.is_none() { break; } // all senders dropped → stop
                            // debounce the burst
                            loop {
                                tokio::select! {
                                    _ = tokio::time::sleep(DEBOUNCE) => break,
                                    r2 = trigger_rx.recv() => { if r2.is_none() { return; } }
                                }
                            }
                        }
                    }
                }
                pending = false;
                reconcile(&task_app, &task_pair, &task_status, &mut session_id).await;
            }
        });

        running.insert(
            pair.id.clone(),
            Running { task, trigger, _watcher: watcher, status },
        );
        Ok(())
    }
}

/// Build a recursive filesystem watcher that pings `trigger` on any change.
fn make_watcher(
    root: &str,
    trigger: mpsc::UnboundedSender<()>,
) -> Result<notify::RecommendedWatcher> {
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res {
            use notify::EventKind;
            if matches!(
                ev.kind,
                EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
            ) {
                let _ = trigger.send(());
            }
        }
    })?;
    watcher.watch(Path::new(root), RecursiveMode::Recursive)?;
    Ok(watcher)
}

/// One reconcile pass for a pair: (re)connect, plan, execute, wait, update status.
async fn reconcile(
    app: &AppHandle,
    pair: &SyncPair,
    status: &Arc<Mutex<RuntimeStatus>>,
    session_id: &mut Option<String>,
) {
    let state = app.state::<AppState>();
    set_state(app, status, PairState::Scanning, |_| {}).await;

    let result = reconcile_inner(app, &state, pair, session_id).await;
    match result {
        Ok(()) => {
            set_state(app, status, PairState::Idle, |s| {
                s.in_flight = 0;
                s.last_synced = Some(now_ms());
                s.last_error = None;
            })
            .await;
        }
        Err(e) => {
            // Drop the session so the next tick reconnects from scratch.
            *session_id = None;
            let msg = format!("{e:#}");
            tracing::warn!("folder sync '{}': {msg}", pair.name);
            set_state(app, status, PairState::Error, move |s| {
                s.in_flight = 0;
                s.last_error = Some(msg);
            })
            .await;
        }
    }
}

async fn reconcile_inner(
    app: &AppHandle,
    state: &AppState,
    pair: &SyncPair,
    session_id: &mut Option<String>,
) -> Result<()> {
    // Ensure a live session for this pair's connection.
    let session = ensure_session(app, state, pair, session_id).await?;

    let local_fs: Box<dyn crate::remotefs::RemoteFs> =
        Box::new(crate::remotefs::local::LocalFs);
    let remote_fs = crate::commands::fs_for_session(&session);

    let plan = crate::sync::plan(
        local_fs.as_ref(),
        remote_fs.as_ref(),
        &pair.local_root,
        &pair.remote_root,
        pair.direction,
        pair.strategy,
    )
    .await
    .context("planning sync")?;

    if plan.copies.is_empty() && plan.deletes.is_empty() {
        return Ok(());
    }

    let ids = crate::commands::execute_sync_plan(session, plan, &state.transfers, app)
        .await
        .context("executing sync")?;

    wait_for_transfers(state, ids).await;
    Ok(())
}

/// Return a live `Arc<Session>` for the pair, reusing the cached session id when
/// it's still valid, otherwise connecting the profile fresh.
async fn ensure_session(
    app: &AppHandle,
    state: &AppState,
    pair: &SyncPair,
    session_id: &mut Option<String>,
) -> Result<Arc<crate::session::Session>> {
    if let Some(id) = session_id.as_deref() {
        if let Some(s) = state.sessions.get(id).await {
            return Ok(s);
        }
    }
    let profile = state
        .profiles
        .get(&pair.profile_id)
        .await
        .context("loading the sync target's connection")?
        .ok_or_else(|| anyhow::anyhow!("connection for this sync pair no longer exists"))?;
    let id = state
        .sessions
        .connect(profile, app.clone())
        .await
        .context("connecting to the sync target")?;
    let session = state
        .sessions
        .get(&id)
        .await
        .ok_or_else(|| anyhow::anyhow!("session vanished right after connect"))?;
    *session_id = Some(id);
    Ok(session)
}

/// Block until every queued transfer reaches a terminal state (or the cap).
async fn wait_for_transfers(state: &AppState, ids: Vec<String>) {
    if ids.is_empty() {
        return;
    }
    let start = std::time::Instant::now();
    loop {
        let mut remaining = 0u32;
        for id in &ids {
            if let Some(t) = state.transfers.snapshot(id).await {
                match t.status {
                    TransferStatus::Queued | TransferStatus::Transferring => remaining += 1,
                    _ => {}
                }
            }
        }
        if remaining == 0 || start.elapsed() > TRANSFER_WAIT_CAP {
            return;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

/// Update status, run a mutator, and emit a change event so the UI refreshes.
async fn set_state(
    app: &AppHandle,
    status: &Arc<Mutex<RuntimeStatus>>,
    state: PairState,
    mutate: impl FnOnce(&mut RuntimeStatus),
) {
    {
        let mut s = status.lock().await;
        s.state = state;
        mutate(&mut s);
    }
    let _ = app.emit("foldersync://changed", ());
}

// ---------- Tauri commands ----------

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

#[tauri::command]
pub async fn foldersync_list(state: State<'_, AppState>) -> Result<Vec<PairView>, String> {
    Ok(state.foldersync.views().await)
}

#[tauri::command]
pub async fn foldersync_upsert(
    pair: SyncPair,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<PairView>, String> {
    state.foldersync.upsert(&app, pair).await.map_err(err)?;
    Ok(state.foldersync.views().await)
}

#[tauri::command]
pub async fn foldersync_remove(
    id: String,
    state: State<'_, AppState>,
) -> Result<Vec<PairView>, String> {
    state.foldersync.remove(&id).await.map_err(err)?;
    Ok(state.foldersync.views().await)
}

#[tauri::command]
pub async fn foldersync_set_enabled(
    id: String,
    enabled: bool,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<PairView>, String> {
    state.foldersync.set_enabled(&app, &id, enabled).await.map_err(err)?;
    Ok(state.foldersync.views().await)
}

#[tauri::command]
pub async fn foldersync_sync_now(
    id: String,
    state: State<'_, AppState>,
) -> Result<Vec<PairView>, String> {
    state.foldersync.sync_now(&id).await.map_err(err)?;
    Ok(state.foldersync.views().await)
}

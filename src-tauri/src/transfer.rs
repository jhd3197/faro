use crate::session::{
    BoxSession, DropboxSession, DynamicsSession, FtpSession, GDriveSession, HttpSession,
    HubSpotSession, ObjectSession, OneDriveSession, Session, ShopifySession, SshSession,
    WebdavSession,
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{watch, Mutex, OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinHandle;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Deserialize, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum OverwritePolicy {
    #[default]
    Overwrite,
    Skip,
    Rename,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TransferKind {
    Download,
    Upload,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum TransferStatus {
    Queued,
    Transferring,
    Paused,
    Done,
    Skipped,
    Error,
    Canceled,
}

/// Marker error: a paused transfer was resumed — the copy loop unwinds with
/// this and the runner re-runs the file from byte 0 (Plan 17 Phase 2).
/// Honest on every backend: no per-backend seek support needed.
#[derive(Debug)]
struct RestartFromPause;

impl std::fmt::Display for RestartFromPause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("transfer resumed after pause; restarting file from the beginning")
    }
}

impl std::error::Error for RestartFromPause {}

/// Payload of the `transfer://queue` event: the FIFO of waiting transfer ids
/// plus the manager-level state the panel header renders (Plan 17).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueState {
    pub waiting: Vec<String>,
    pub paused_all: bool,
    pub concurrency: usize,
    pub throttle_kbps: u64,
}

/// A pause gate shared by the scheduler (pause-all) and individual transfers
/// (Phase 2). watch-channel based so waiters never miss a wakeup.
#[derive(Debug, Clone)]
pub struct PauseGate {
    tx: watch::Sender<bool>,
}

impl PauseGate {
    fn new() -> Self {
        let (tx, _) = watch::channel(false);
        Self { tx }
    }
    fn is_paused(&self) -> bool {
        *self.tx.borrow()
    }
    fn set(&self, paused: bool) {
        let _ = self.tx.send(paused);
    }
    /// Park until the gate opens. Returns immediately if already open.
    async fn wait_open(&self) {
        let mut rx = self.tx.subscribe();
        while *rx.borrow_and_update() {
            if rx.changed().await.is_err() {
                break;
            }
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Transfer {
    pub id: String,
    pub kind: TransferKind,
    pub source: String,
    pub destination: String,
    pub size: u64,
    pub transferred: u64,
    pub status: TransferStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub started_at: i64,
}

/// Default bound on concurrently running transfers (Plan 17); the rest wait
/// in the FIFO as `Queued`. Overridden by the `transferConcurrency` setting.
const DEFAULT_CONCURRENCY: usize = 3;

pub struct TransferManager {
    transfers: Mutex<HashMap<String, Transfer>>,
    tasks: Mutex<HashMap<String, JoinHandle<()>>>,
    /// FIFO of transfer ids waiting for a slot (Plan 17). An id is popped only
    /// once it is at the front, the pause-all gate is open, and a concurrency
    /// permit is available — until then it waits as `Queued`.
    waiting: Mutex<VecDeque<String>>,
    semaphore: Arc<Semaphore>,
    concurrency: AtomicUsize,
    /// Manager-level pause gate. Admission checks it; Phase 2 checkpoints do too.
    pause_all: PauseGate,
    /// Per-transfer pause gates, created at enqueue time (Plan 17 Phase 2).
    pauses: Mutex<HashMap<String, PauseGate>>,
    /// Bumped on every queue change so admission waiters re-check their turn.
    queue_gen: watch::Sender<u64>,
}

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn basename(path: &str) -> String {
    path.rsplit(|c| c == '/' || c == '\\')
        .next()
        .unwrap_or(path)
        .to_string()
}

/// Append _1, _2, … to the stem until a free local path is found.
fn resolve_local_rename(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let parent = path.parent().unwrap_or(Path::new("."));
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    for i in 1..=999 {
        let candidate = parent.join(format!("{stem}_{i}{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    path.to_path_buf()
}

/// Same idea for a remote path. Uses sftp.metadata to probe existence.
async fn resolve_remote_rename(
    sftp: &russh_sftp::client::SftpSession,
    path: &str,
) -> String {
    if sftp.metadata(path).await.is_err() {
        return path.to_string();
    }
    let (stem, ext) = match path.rfind('.') {
        Some(dot) if dot > path.rfind('/').unwrap_or(0) => {
            (&path[..dot], &path[dot..])
        }
        _ => (path, ""),
    };
    for i in 1..=999 {
        let candidate = format!("{stem}_{i}{ext}");
        if sftp.metadata(&candidate).await.is_err() {
            return candidate;
        }
    }
    path.to_string()
}

impl TransferManager {
    pub fn new() -> Self {
        Self {
            transfers: Mutex::new(HashMap::new()),
            tasks: Mutex::new(HashMap::new()),
            waiting: Mutex::new(VecDeque::new()),
            semaphore: Arc::new(Semaphore::new(DEFAULT_CONCURRENCY)),
            concurrency: AtomicUsize::new(DEFAULT_CONCURRENCY),
            pause_all: PauseGate::new(),
            pauses: Mutex::new(HashMap::new()),
            queue_gen: watch::channel(0).0,
        }
    }

    pub async fn list(&self) -> Vec<Transfer> {
        let mut v: Vec<Transfer> = self.transfers.lock().await.values().cloned().collect();
        v.sort_by_key(|t| t.started_at);
        v
    }

    async fn get(&self, id: &str) -> Option<Transfer> {
        self.transfers.lock().await.get(id).cloned()
    }

    /// Public snapshot of a single transfer by id (used by the Agent Bridge so
    /// an agent can poll whether a download/upload it started has finished).
    pub async fn snapshot(&self, id: &str) -> Option<Transfer> {
        self.transfers.lock().await.get(id).cloned()
    }

    async fn update<F: FnOnce(&mut Transfer)>(&self, id: &str, f: F) {
        if let Some(t) = self.transfers.lock().await.get_mut(id) {
            f(t);
        }
    }

    async fn insert(&self, t: Transfer) {
        self.transfers.lock().await.insert(t.id.clone(), t);
    }

    // ---------- Queue scheduling (Plan 17) ----------

    fn build_queue_state(&self, waiting: &VecDeque<String>) -> QueueState {
        QueueState {
            waiting: waiting.iter().cloned().collect(),
            paused_all: self.pause_all.is_paused(),
            concurrency: self.concurrency.load(Ordering::Relaxed),
            throttle_kbps: 0, // wired up in Phase 4
        }
    }

    /// Current queue snapshot for the panel's initial load.
    pub async fn queue_state(&self) -> QueueState {
        let w = self.waiting.lock().await;
        self.build_queue_state(&w)
    }

    /// Emit `transfer://queue` and bump the generation so admission waiters
    /// re-check whether it is their turn.
    async fn bump_queue(&self, app: &AppHandle) {
        let state = {
            let w = self.waiting.lock().await;
            self.build_queue_state(&w)
        };
        self.queue_gen.send_modify(|g| *g += 1);
        let _ = app.emit("transfer://queue", &state);
    }

    /// Wait until this transfer is at the front of the FIFO with the pause-all
    /// gate open, take a concurrency permit, and pop it from the queue.
    /// Returns `None` if the id left the queue (canceled while waiting).
    /// The permit is returned so the caller holds it for the transfer's life.
    async fn admit(&self, id: &str) -> Option<OwnedSemaphorePermit> {
        let mut rx = self.queue_gen.subscribe();
        loop {
            {
                let w = self.waiting.lock().await;
                if !w.iter().any(|x| x == id) {
                    return None;
                }
                if !self.is_my_turn(&w, id).await {
                    drop(w);
                    if rx.changed().await.is_err() {
                        return None;
                    }
                    continue;
                }
            }
            // My turn: take a permit, then pop.
            let permit = self.semaphore.clone().acquire_owned().await.ok()?;
            let mut w = self.waiting.lock().await;
            if self.is_my_turn(&w, id).await {
                if let Some(pos) = w.iter().position(|x| x == id) {
                    w.remove(pos);
                }
                return Some(permit);
            }
            // Lost a race (pause engaged mid-acquire): release, re-evaluate.
            drop(permit);
            if !w.iter().any(|x| x == id) {
                return None;
            }
            drop(w);
        }
    }

    /// Is `id` the first waiting transfer allowed to run? Strict FIFO except
    /// that per-transfer-paused rows are skipped (a paused row must not
    /// head-of-line block the queue); pause-all blocks everyone. Caller must
    /// hold the `waiting` lock; lock order is waiting → pauses.
    async fn is_my_turn(&self, w: &VecDeque<String>, id: &str) -> bool {
        if self.pause_all.is_paused() {
            return false;
        }
        let pauses = self.pauses.lock().await;
        let first_open = w
            .iter()
            .position(|x| pauses.get(x).map_or(true, |g| !g.is_paused()));
        first_open == w.iter().position(|x| x == id)
    }

    /// Reorder a waiting transfer (active transfers are untouched).
    pub async fn move_in_queue(&self, id: &str, up: bool, app: &AppHandle) -> Result<()> {
        {
            let mut w = self.waiting.lock().await;
            let Some(pos) = w.iter().position(|x| x == id) else {
                anyhow::bail!("transfer {id} is not waiting in the queue");
            };
            let swap_with = if up {
                pos.checked_sub(1)
            } else if pos + 1 < w.len() {
                Some(pos + 1)
            } else {
                None
            };
            if let Some(other) = swap_with {
                w.swap(pos, other);
            }
        }
        self.bump_queue(app).await;
        Ok(())
    }

    /// Pause admission of new transfers (running ones keep going until Phase
    /// 2's chunk checkpoints let them park too).
    pub async fn pause_all(&self, app: &AppHandle) {
        self.pause_all.set(true);
        self.bump_queue(app).await;
    }

    pub async fn resume_all(&self, app: &AppHandle) {
        self.pause_all.set(false);
        self.bump_queue(app).await;
    }

    pub fn is_paused_all(&self) -> bool {
        self.pause_all.is_paused()
    }

    /// Chunk-boundary checkpoint shared by every copy loop (Plan 17). Phase 4
    /// draws `_bytes` from the bandwidth bucket here. When the transfer (or
    /// the whole manager) is paused, this parks until resumed, then returns
    /// `RestartFromPause` so the runner re-runs the file from byte 0.
    async fn checkpoint(&self, id: &str, _bytes: u64) -> Result<()> {
        let gate = self.pauses.lock().await.get(id).cloned();
        let parked = self.pause_all.is_paused() || gate.as_ref().is_some_and(|g| g.is_paused());
        if !parked {
            return Ok(());
        }
        // Park until BOTH gates are open (resume requires both).
        loop {
            self.pause_all.wait_open().await;
            if let Some(g) = &gate {
                g.wait_open().await;
            }
            if !self.pause_all.is_paused() && gate.as_ref().is_none_or(|g| !g.is_paused()) {
                break;
            }
        }
        Err(RestartFromPause.into())
    }

    /// Pause a queued or transferring transfer. A running one parks at the
    /// next chunk boundary; a queued one is skipped by admission until resumed.
    pub async fn pause(&self, id: &str, app: &AppHandle) -> Result<()> {
        match self.get(id).await.map(|t| t.status) {
            Some(TransferStatus::Transferring) | Some(TransferStatus::Queued) => {}
            _ => anyhow::bail!("transfer {id} is not running or queued"),
        }
        let gate = self
            .pauses
            .lock()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("transfer {id} not found"))?;
        gate.set(true);
        self.update(id, |t| t.status = TransferStatus::Paused).await;
        if let Some(t) = self.get(id).await {
            let _ = app.emit("transfer://updated", &t);
        }
        Ok(())
    }

    /// Resume a paused transfer. A parked one re-runs its file from byte 0;
    /// a queued one re-enters FIFO admission.
    pub async fn resume(&self, id: &str, app: &AppHandle) -> Result<()> {
        if self.get(id).await.map(|t| t.status) != Some(TransferStatus::Paused) {
            anyhow::bail!("transfer {id} is not paused");
        }
        let still_queued = self.waiting.lock().await.iter().any(|x| x == id);
        let gate = self
            .pauses
            .lock()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("transfer {id} not found"))?;
        self.update(id, |t| {
            t.status = if still_queued {
                TransferStatus::Queued
            } else {
                TransferStatus::Transferring
            };
            t.transferred = 0;
        })
        .await;
        gate.set(false);
        if let Some(t) = self.get(id).await {
            let _ = app.emit("transfer://updated", &t);
        }
        // Wake admission waiters: a queued row may have become runnable.
        self.bump_queue(app).await;
        Ok(())
    }

    /// Live-adjust the concurrency bound. Growing adds permits at once;
    /// shrinking forgets permits as running transfers release them, so
    /// in-flight transfers are never killed to satisfy the new bound.
    pub fn set_concurrency(&self, n: usize) {
        let n = n.clamp(1, 32);
        let old = self.concurrency.swap(n, Ordering::Relaxed);
        if n > old {
            self.semaphore.add_permits(n - old);
        } else if n < old {
            let sem = Arc::clone(&self.semaphore);
            tokio::spawn(async move {
                if let Ok(p) = sem.acquire_many((old - n) as u32).await {
                    p.forget();
                }
            });
        }
    }

    pub async fn cancel(&self, id: &str, app: &AppHandle) -> Result<()> {
        {
            let mut w = self.waiting.lock().await;
            if let Some(pos) = w.iter().position(|x| x == id) {
                w.remove(pos);
            }
        }
        if let Some(h) = self.tasks.lock().await.remove(id) {
            h.abort();
        }
        self.update(id, |t| {
            if matches!(
                t.status,
                TransferStatus::Transferring | TransferStatus::Queued | TransferStatus::Paused
            ) {
                t.status = TransferStatus::Canceled;
            }
        })
        .await;
        // There is no `transfer://canceled` event — `updated` carries the row.
        if let Some(t) = self.get(id).await {
            let _ = app.emit("transfer://updated", &t);
        }
        self.bump_queue(app).await;
        Ok(())
    }

    pub async fn start_download(
        self: &Arc<Self>,
        session: Arc<Session>,
        remote_path: String,
        local_dir: String,
        policy: OverwritePolicy,
        app: AppHandle,
    ) -> Result<String> {
        let size = remote_size(&session, &remote_path).await.unwrap_or(0);

        let initial = PathBuf::from(&local_dir).join(basename(&remote_path));
        let (final_path, skip) = match policy {
            OverwritePolicy::Overwrite => (initial, false),
            OverwritePolicy::Skip => {
                let exists = initial.exists();
                (initial, exists)
            }
            OverwritePolicy::Rename => (resolve_local_rename(&initial), false),
        };

        let id = Uuid::new_v4().to_string();
        let transfer = Transfer {
            id: id.clone(),
            kind: TransferKind::Download,
            source: remote_path.clone(),
            destination: final_path.to_string_lossy().into_owned(),
            size,
            transferred: 0,
            status: if skip {
                TransferStatus::Skipped
            } else {
                TransferStatus::Queued
            },
            error: None,
            started_at: now_ts(),
        };
        self.insert(transfer.clone()).await;
        let _ = app.emit("transfer://added", &transfer);

        if skip {
            let _ = app.emit("transfer://done", &transfer);
            return Ok(id);
        }

        self.waiting.lock().await.push_back(id.clone());
        self.pauses.lock().await.insert(id.clone(), PauseGate::new());
        self.bump_queue(&app).await;

        let mgr = Arc::clone(self);
        let id_for_task = id.clone();
        let task = tokio::spawn(async move {
            let Some(_permit) = mgr.admit(&id_for_task).await else {
                return;
            };
            // A resume-after-pause unwinds the copy loop with RestartFromPause;
            // re-run the file from byte 0 (Plan 17 Phase 2).
            let res = loop {
            let attempt = match &*session {
                Session::Ssh(ssh) => {
                    mgr.run_ssh_download(
                        &id_for_task,
                        ssh.clone(),
                        &remote_path,
                        &final_path,
                        &app,
                    )
                    .await
                }
                Session::Ftp(ftp) => {
                    mgr.run_ftp_download(
                        &id_for_task,
                        ftp.clone(),
                        &remote_path,
                        &final_path,
                        &app,
                    )
                    .await
                }
                Session::Object(obj) => {
                    mgr.run_object_download(
                        &id_for_task,
                        obj.clone(),
                        &remote_path,
                        &final_path,
                        &app,
                    )
                    .await
                }
                Session::Webdav(dav) => {
                    mgr.run_webdav_download(
                        &id_for_task,
                        dav.clone(),
                        &remote_path,
                        &final_path,
                        &app,
                    )
                    .await
                }
                Session::Http(http) => {
                    mgr.run_http_download(
                        &id_for_task,
                        http.clone(),
                        &remote_path,
                        &final_path,
                        &app,
                    )
                    .await
                }
                Session::Dropbox(dbx) => {
                    mgr.run_dropbox_download(
                        &id_for_task,
                        dbx.clone(),
                        &remote_path,
                        &final_path,
                        &app,
                    )
                    .await
                }
                Session::OneDrive(od) => {
                    mgr.run_onedrive_download(
                        &id_for_task,
                        od.clone(),
                        &remote_path,
                        &final_path,
                        &app,
                    )
                    .await
                }
                Session::GDrive(gd) => {
                    mgr.run_gdrive_download(
                        &id_for_task,
                        gd.clone(),
                        &remote_path,
                        &final_path,
                        &app,
                    )
                    .await
                }
                Session::Box(bx) => {
                    mgr.run_box_download(
                        &id_for_task,
                        bx.clone(),
                        &remote_path,
                        &final_path,
                        &app,
                    )
                    .await
                }
                Session::Shopify(sh) => {
                    mgr.run_shopify_download(
                        &id_for_task,
                        sh.clone(),
                        &remote_path,
                        &final_path,
                        &app,
                    )
                    .await
                }
                Session::HubSpot(hs) => {
                    mgr.run_hubspot_download(
                        &id_for_task,
                        hs.clone(),
                        &remote_path,
                        &final_path,
                        &app,
                    )
                    .await
                }
                Session::Dynamics(dynm) => {
                    mgr.run_dynamics_download(
                        &id_for_task,
                        dynm.clone(),
                        &remote_path,
                        &final_path,
                        &app,
                    )
                    .await
                }
                Session::Agent(agent) => {
                    mgr.run_agent_download(
                        &id_for_task,
                        agent.clone(),
                        &remote_path,
                        &final_path,
                        &app,
                    )
                    .await
                }
            };
            match attempt {
                Err(e) if e.downcast_ref::<RestartFromPause>().is_some() => {
                    mgr.update(&id_for_task, |t| t.transferred = 0).await;
                    continue;
                }
                other => break other,
            }
            };
            finalize(&mgr, &id_for_task, &app, res).await;
            mgr.bump_queue(&app).await;
        });
        self.tasks.lock().await.insert(id.clone(), task);
        Ok(id)
    }

    /// Stream a download from a Faro Agent daemon by ranged `ReadChunk`s. The
    /// daemon caps each chunk; we advance the offset until it reports EOF.
    async fn run_agent_download(
        &self,
        id: &str,
        session: Arc<crate::session::AgentSession>,
        remote_path: &str,
        local_path: &Path,
        app: &AppHandle,
    ) -> Result<()> {
        use base64::Engine as _;
        use faro_agent_proto::msg::{Request, Response};
        self.update(id, |t| t.status = TransferStatus::Transferring).await;

        let mut local_file = tokio::fs::File::create(local_path)
            .await
            .with_context(|| format!("create {}", local_path.display()))?;
        let mut offset: u64 = 0;
        let mut last_emit = Instant::now();
        loop {
            let resp = session
                .request(Request::ReadChunk { path: remote_path.to_string(), offset, len: 0 })
                .await?;
            let (data_b64, eof) = match resp {
                Response::Chunk { data, eof } => (data, eof),
                Response::Error { message, .. } => {
                    return Err(anyhow::anyhow!("download {remote_path}: {message}"))
                }
                other => return Err(anyhow::anyhow!("download {remote_path}: unexpected {other:?}")),
            };
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&data_b64)
                .context("decode chunk")?;
            self.checkpoint(id, bytes.len() as u64).await?;
            if !bytes.is_empty() {
                local_file.write_all(&bytes).await?;
                offset += bytes.len() as u64;
                if last_emit.elapsed() > Duration::from_millis(100) {
                    self.update(id, |t| t.transferred = offset).await;
                    if let Some(t) = self.get(id).await {
                        let _ = app.emit("transfer://progress", &t);
                    }
                    last_emit = Instant::now();
                }
            }
            if eof {
                break;
            }
        }
        local_file.flush().await?;
        self.update(id, |t| t.transferred = offset).await;
        Ok(())
    }

    async fn run_ssh_download(
        &self,
        id: &str,
        session: Arc<SshSession>,
        remote_path: &str,
        local_path: &Path,
        app: &AppHandle,
    ) -> Result<()> {
        self.update(id, |t| t.status = TransferStatus::Transferring)
            .await;

        let sftp_cell = session.ensure_sftp().await?;
        let mut remote_file = {
            let sftp = sftp_cell.lock().await;
            sftp.open(remote_path)
                .await
                .with_context(|| format!("open remote {remote_path}"))?
        };
        let mut local_file = tokio::fs::File::create(local_path)
            .await
            .with_context(|| format!("create {}", local_path.display()))?;

        let mut buf = vec![0u8; 64 * 1024];
        let mut transferred: u64 = 0;
        let mut last_emit = Instant::now();
        loop {
            let n = remote_file.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            self.checkpoint(id, n as u64).await?;
            local_file.write_all(&buf[..n]).await?;
            transferred += n as u64;
            if last_emit.elapsed() > Duration::from_millis(100) {
                self.update(id, |t| t.transferred = transferred).await;
                if let Some(t) = self.get(id).await {
                    let _ = app.emit("transfer://progress", &t);
                }
                last_emit = Instant::now();
            }
        }
        local_file.flush().await?;
        self.update(id, |t| t.transferred = transferred).await;
        Ok(())
    }

    pub async fn start_upload(
        self: &Arc<Self>,
        session: Arc<Session>,
        local_path: String,
        remote_dir: String,
        policy: OverwritePolicy,
        app: AppHandle,
    ) -> Result<String> {
        let local = PathBuf::from(&local_path);
        let size = tokio::fs::metadata(&local)
            .await
            .with_context(|| format!("stat {}", local.display()))?
            .len();

        let initial_remote = if remote_dir.ends_with('/') {
            format!("{remote_dir}{}", basename(&local_path))
        } else {
            format!("{remote_dir}/{}", basename(&local_path))
        };

        let (final_remote, skip) = remote_resolve(&session, &initial_remote, policy).await?;

        let id = Uuid::new_v4().to_string();
        let transfer = Transfer {
            id: id.clone(),
            kind: TransferKind::Upload,
            source: local_path.clone(),
            destination: final_remote.clone(),
            size,
            transferred: 0,
            status: if skip {
                TransferStatus::Skipped
            } else {
                TransferStatus::Queued
            },
            error: None,
            started_at: now_ts(),
        };
        self.insert(transfer.clone()).await;
        let _ = app.emit("transfer://added", &transfer);

        if skip {
            let _ = app.emit("transfer://done", &transfer);
            return Ok(id);
        }

        self.waiting.lock().await.push_back(id.clone());
        self.pauses.lock().await.insert(id.clone(), PauseGate::new());
        self.bump_queue(&app).await;

        let mgr = Arc::clone(self);
        let id_for_task = id.clone();
        let task = tokio::spawn(async move {
            let Some(_permit) = mgr.admit(&id_for_task).await else {
                return;
            };
            // A resume-after-pause unwinds the copy loop with RestartFromPause;
            // re-run the file from byte 0 (Plan 17 Phase 2).
            let res = loop {
            let attempt = match &*session {
                Session::Ssh(ssh) => {
                    mgr.run_ssh_upload(
                        &id_for_task,
                        ssh.clone(),
                        &local,
                        &final_remote,
                        &app,
                    )
                    .await
                }
                Session::Ftp(ftp) => {
                    mgr.run_ftp_upload(
                        &id_for_task,
                        ftp.clone(),
                        &local,
                        &final_remote,
                        &app,
                    )
                    .await
                }
                Session::Object(obj) => {
                    mgr.run_object_upload(
                        &id_for_task,
                        obj.clone(),
                        &local,
                        &final_remote,
                        &app,
                    )
                    .await
                }
                Session::Webdav(dav) => {
                    mgr.run_webdav_upload(
                        &id_for_task,
                        dav.clone(),
                        &local,
                        &final_remote,
                        &app,
                    )
                    .await
                }
                Session::Http(_) => {
                    Err(anyhow::anyhow!("HTTP source is read-only — upload not supported"))
                }
                Session::Dropbox(dbx) => {
                    mgr.run_dropbox_upload(
                        &id_for_task,
                        dbx.clone(),
                        &local,
                        &final_remote,
                        &app,
                    )
                    .await
                }
                Session::OneDrive(od) => {
                    mgr.run_onedrive_upload(
                        &id_for_task,
                        od.clone(),
                        &local,
                        &final_remote,
                        &app,
                    )
                    .await
                }
                Session::GDrive(gd) => {
                    mgr.run_gdrive_upload(
                        &id_for_task,
                        gd.clone(),
                        &local,
                        &final_remote,
                        &app,
                    )
                    .await
                }
                Session::Box(bx) => {
                    mgr.run_box_upload(
                        &id_for_task,
                        bx.clone(),
                        &local,
                        &final_remote,
                        &app,
                    )
                    .await
                }
                Session::Shopify(sh) => {
                    mgr.run_shopify_upload(
                        &id_for_task,
                        sh.clone(),
                        &local,
                        &final_remote,
                        &app,
                    )
                    .await
                }
                Session::HubSpot(hs) => {
                    mgr.run_hubspot_upload(
                        &id_for_task,
                        hs.clone(),
                        &local,
                        &final_remote,
                        &app,
                    )
                    .await
                }
                Session::Dynamics(dynm) => {
                    mgr.run_dynamics_upload(
                        &id_for_task,
                        dynm.clone(),
                        &local,
                        &final_remote,
                        &app,
                    )
                    .await
                }
                Session::Agent(agent) => {
                    mgr.run_agent_upload(
                        &id_for_task,
                        agent.clone(),
                        &local,
                        &final_remote,
                        &app,
                    )
                    .await
                }
            };
            match attempt {
                Err(e) if e.downcast_ref::<RestartFromPause>().is_some() => {
                    mgr.update(&id_for_task, |t| t.transferred = 0).await;
                    continue;
                }
                other => break other,
            }
            };
            finalize(&mgr, &id_for_task, &app, res).await;
            mgr.bump_queue(&app).await;
        });
        self.tasks.lock().await.insert(id.clone(), task);
        Ok(id)
    }

    /// Stream an upload to a Faro Agent daemon by ranged `WriteChunk`s. The first
    /// chunk truncates/creates the file; subsequent chunks append at the offset.
    async fn run_agent_upload(
        &self,
        id: &str,
        session: Arc<crate::session::AgentSession>,
        local_path: &Path,
        remote_path: &str,
        app: &AppHandle,
    ) -> Result<()> {
        use base64::Engine as _;
        use faro_agent_proto::msg::{Request, Response};
        self.update(id, |t| t.status = TransferStatus::Transferring).await;

        let mut local_file = tokio::fs::File::open(local_path)
            .await
            .with_context(|| format!("open {}", local_path.display()))?;
        // 128 KiB plaintext keeps each request comfortably under the daemon's
        // per-chunk cap while amortising the round-trip.
        let mut buf = vec![0u8; 128 * 1024];
        let mut offset: u64 = 0;
        let mut first = true;
        let mut last_emit = Instant::now();
        loop {
            let n = local_file.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            self.checkpoint(id, n as u64).await?;
            let data = base64::engine::general_purpose::STANDARD.encode(&buf[..n]);
            let resp = session
                .request(Request::WriteChunk {
                    path: remote_path.to_string(),
                    offset,
                    data,
                    truncate: first,
                    done: false,
                })
                .await?;
            match resp {
                Response::Written { .. } => {}
                Response::Error { message, .. } => {
                    return Err(anyhow::anyhow!("upload {remote_path}: {message}"))
                }
                other => return Err(anyhow::anyhow!("upload {remote_path}: unexpected {other:?}")),
            }
            offset += n as u64;
            first = false;
            if last_emit.elapsed() > Duration::from_millis(100) {
                self.update(id, |t| t.transferred = offset).await;
                if let Some(t) = self.get(id).await {
                    let _ = app.emit("transfer://progress", &t);
                }
                last_emit = Instant::now();
            }
        }
        self.update(id, |t| t.transferred = offset).await;
        Ok(())
    }

    async fn run_ssh_upload(
        &self,
        id: &str,
        session: Arc<SshSession>,
        local_path: &Path,
        remote_path: &str,
        app: &AppHandle,
    ) -> Result<()> {
        self.update(id, |t| t.status = TransferStatus::Transferring)
            .await;

        let sftp_cell = session.ensure_sftp().await?;
        let mut remote_file = {
            let sftp = sftp_cell.lock().await;
            sftp.create(remote_path)
                .await
                .with_context(|| format!("create remote {remote_path}"))?
        };
        let mut local_file = tokio::fs::File::open(local_path)
            .await
            .with_context(|| format!("open {}", local_path.display()))?;

        let mut buf = vec![0u8; 64 * 1024];
        let mut transferred: u64 = 0;
        let mut last_emit = Instant::now();
        loop {
            let n = local_file.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            self.checkpoint(id, n as u64).await?;
            remote_file.write_all(&buf[..n]).await?;
            transferred += n as u64;
            if last_emit.elapsed() > Duration::from_millis(100) {
                self.update(id, |t| t.transferred = transferred).await;
                if let Some(t) = self.get(id).await {
                    let _ = app.emit("transfer://progress", &t);
                }
                last_emit = Instant::now();
            }
        }
        remote_file.flush().await?;
        self.update(id, |t| t.transferred = transferred).await;
        Ok(())
    }
}

impl TransferManager {
    /// Walk a remote directory tree and queue a transfer per file. Uses the
    /// RemoteFs trait so it works for both SFTP and FTP.
    pub async fn start_directory_download(
        self: &Arc<Self>,
        session: Arc<Session>,
        remote_root: String,
        local_dir: String,
        policy: OverwritePolicy,
        app: AppHandle,
    ) -> Result<Vec<String>> {
        let root_name = basename(&remote_root);
        let local_root = PathBuf::from(&local_dir).join(&root_name);
        tokio::fs::create_dir_all(&local_root)
            .await
            .with_context(|| format!("mkdir -p {}", local_root.display()))?;

        let fs = fs_for_session(&session);

        let mut dirs_to_visit: Vec<String> = vec![remote_root.clone()];
        let mut files: Vec<(String, PathBuf)> = Vec::new();

        while let Some(d) = dirs_to_visit.pop() {
            let entries = fs.list_dir(&d).await.with_context(|| format!("read_dir {d}"))?;
            for entry in entries {
                let remote_child = entry.path.clone();
                let rel = remote_child
                    .strip_prefix(&remote_root)
                    .unwrap_or(&remote_child)
                    .trim_start_matches('/');
                let local_child = local_root.join(rel);
                match entry.kind {
                    crate::remotefs::FileKind::Directory => {
                        tokio::fs::create_dir_all(&local_child).await.ok();
                        dirs_to_visit.push(remote_child);
                    }
                    crate::remotefs::FileKind::File => {
                        files.push((remote_child, local_child));
                    }
                    _ => {}
                }
            }
        }

        let mut ids = Vec::with_capacity(files.len());
        for (remote_path, local_path) in files {
            let parent_dir = local_path
                .parent()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|| ".".into());
            let id = self
                .start_download(
                    Arc::clone(&session),
                    remote_path,
                    parent_dir,
                    policy,
                    app.clone(),
                )
                .await?;
            ids.push(id);
        }
        Ok(ids)
    }

    pub async fn start_directory_upload(
        self: &Arc<Self>,
        session: Arc<Session>,
        local_root: String,
        remote_dir: String,
        policy: OverwritePolicy,
        app: AppHandle,
    ) -> Result<Vec<String>> {
        let local_root_path = PathBuf::from(&local_root);
        let root_name = local_root_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "upload".into());
        let remote_root = if remote_dir.ends_with('/') {
            format!("{remote_dir}{root_name}")
        } else {
            format!("{remote_dir}/{root_name}")
        };

        let fs = fs_for_session(&session);

        // Best-effort: create the remote root.
        let _ = fs.create_dir(&remote_root).await;

        let mut dirs_to_visit: Vec<PathBuf> = vec![local_root_path.clone()];
        let mut files: Vec<(PathBuf, String)> = Vec::new();
        let mut subdirs: Vec<String> = Vec::new();
        while let Some(d) = dirs_to_visit.pop() {
            let mut rd = tokio::fs::read_dir(&d)
                .await
                .with_context(|| format!("read_dir {}", d.display()))?;
            while let Some(entry) = rd.next_entry().await? {
                let p = entry.path();
                let meta = match entry.metadata().await {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                let rel = p
                    .strip_prefix(&local_root_path)
                    .unwrap_or(&p)
                    .to_string_lossy()
                    .replace('\\', "/");
                let remote_child = format!("{remote_root}/{rel}");
                if meta.is_dir() {
                    subdirs.push(remote_child);
                    dirs_to_visit.push(p);
                } else if meta.is_file() {
                    files.push((p, remote_child));
                }
            }
        }

        subdirs.sort_by_key(|s| s.matches('/').count());
        for sd in subdirs {
            let _ = fs.create_dir(&sd).await;
        }

        let mut ids = Vec::with_capacity(files.len());
        for (local_path, remote_path) in files {
            let parent = remote_path
                .rsplit_once('/')
                .map(|(p, _)| p.to_string())
                .unwrap_or_else(|| remote_root.clone());
            let id = self
                .start_upload(
                    Arc::clone(&session),
                    local_path.to_string_lossy().into_owned(),
                    parent,
                    policy,
                    app.clone(),
                )
                .await?;
            ids.push(id);
        }
        Ok(ids)
    }

    async fn run_ftp_download(
        &self,
        id: &str,
        session: Arc<FtpSession>,
        remote_path: &str,
        local_path: &Path,
        app: &AppHandle,
    ) -> Result<()> {
        self.update(id, |t| t.status = TransferStatus::Transferring)
            .await;

        // FTP's data channel doesn't surface incremental progress easily without
        // an extra control round-trip. We can still report mid-transfer by
        // calling .size() up front, then bumping `transferred` to size on
        // completion. For now we keep it simple: 0 -> size on done.
        let final_path = local_path.to_path_buf();
        let path = remote_path.to_string();
        let id_for_emit = id.to_string();
        let app_for_emit = app.clone();
        let mgr_for_emit: *const TransferManager = self;
        // The pointer cast keeps the closure 'static — we re-form an Arc via
        // the field on the manager's containing Arc inside the blocking task.
        // Since the blocking task is awaited (not detached), the manager
        // outlives the borrow. We update progress after the task returns.

        let _ = (mgr_for_emit, id_for_emit, app_for_emit);

        self.checkpoint(id, 0).await?;
        let res: Result<u64> = session
            .with_stream(move |stream| {
                let file = std::fs::File::create(&final_path)
                    .with_context(|| format!("create {}", final_path.display()))?;
                let written = stream
                    .retr_to_writer(&path, std::io::BufWriter::new(file))?;
                Ok(written)
            })
            .await;

        let written = res?;
        self.update(id, |t| t.transferred = written).await;
        Ok(())
    }

    async fn run_ftp_upload(
        &self,
        id: &str,
        session: Arc<FtpSession>,
        local_path: &Path,
        remote_path: &str,
        app: &AppHandle,
    ) -> Result<()> {
        self.update(id, |t| t.status = TransferStatus::Transferring)
            .await;
        let _ = app; // progress events come at completion for FTP

        self.checkpoint(id, 0).await?;
        let local = local_path.to_path_buf();
        let remote = remote_path.to_string();
        let res: Result<u64> = session
            .with_stream(move |stream| {
                let file = std::fs::File::open(&local)
                    .with_context(|| format!("open {}", local.display()))?;
                let mut reader = std::io::BufReader::new(file);
                let written = stream.put_from_reader(&remote, &mut reader)?;
                Ok(written)
            })
            .await;
        let written = res?;
        self.update(id, |t| t.transferred = written).await;
        Ok(())
    }

    async fn run_object_download(
        &self,
        id: &str,
        session: Arc<ObjectSession>,
        remote_path: &str,
        local_path: &Path,
        app: &AppHandle,
    ) -> Result<()> {
        use futures::StreamExt;
        use tokio::io::AsyncWriteExt;

        self.update(id, |t| t.status = TransferStatus::Transferring)
            .await;

        let key = remote_path.trim_start_matches('/');
        let p = object_store::path::Path::from(key);
        let get = session
            .store
            .get(&p)
            .await
            .with_context(|| format!("s3 get {key}"))?;

        let mut file = tokio::fs::File::create(local_path)
            .await
            .with_context(|| format!("create {}", local_path.display()))?;
        let mut stream = get.into_stream();
        let mut transferred: u64 = 0;
        let mut last_emit = Instant::now();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.with_context(|| format!("s3 chunk for {key}"))?;
            self.checkpoint(id, chunk.len() as u64).await?;
            file.write_all(&chunk).await?;
            transferred += chunk.len() as u64;
            if last_emit.elapsed() > Duration::from_millis(100) {
                self.update(id, |t| t.transferred = transferred).await;
                if let Some(t) = self.get(id).await {
                    let _ = app.emit("transfer://progress", &t);
                }
                last_emit = Instant::now();
            }
        }
        file.flush().await?;
        self.update(id, |t| t.transferred = transferred).await;
        Ok(())
    }

    async fn run_object_upload(
        &self,
        id: &str,
        session: Arc<ObjectSession>,
        local_path: &Path,
        remote_path: &str,
        app: &AppHandle,
    ) -> Result<()> {
        use tokio::io::AsyncReadExt;

        self.update(id, |t| t.status = TransferStatus::Transferring)
            .await;

        let key = remote_path.trim_start_matches('/');
        let p = object_store::path::Path::from(key);

        // For files under ~16 MB we put in one shot; larger files go through
        // a multipart upload so we get streaming + parallelism without buffering
        // the whole body in memory.
        let meta = tokio::fs::metadata(local_path)
            .await
            .with_context(|| format!("stat {}", local_path.display()))?;
        let size = meta.len();
        let mut file = tokio::fs::File::open(local_path)
            .await
            .with_context(|| format!("open {}", local_path.display()))?;

        if size <= 16 * 1024 * 1024 {
            self.checkpoint(id, size).await?;
            let mut buf = Vec::with_capacity(size as usize);
            file.read_to_end(&mut buf).await?;
            session
                .store
                .put(&p, bytes::Bytes::from(buf).into())
                .await
                .with_context(|| format!("s3 put {key}"))?;
            self.update(id, |t| t.transferred = size).await;
            if let Some(t) = self.get(id).await {
                let _ = app.emit("transfer://progress", &t);
            }
            return Ok(());
        }

        // Multipart path. object_store wants a `MultipartUpload` for which we
        // push parts and call complete() at the end.
        let mut upload = session
            .store
            .put_multipart(&p)
            .await
            .with_context(|| format!("s3 begin multipart {key}"))?;

        const PART: usize = 8 * 1024 * 1024;
        let mut transferred: u64 = 0;
        let mut last_emit = Instant::now();
        let mut buf = vec![0u8; PART];
        loop {
            let mut filled = 0;
            while filled < buf.len() {
                let n = file.read(&mut buf[filled..]).await?;
                if n == 0 {
                    break;
                }
                filled += n;
            }
            if filled == 0 {
                break;
            }
            self.checkpoint(id, filled as u64).await?;
            let chunk = bytes::Bytes::copy_from_slice(&buf[..filled]);
            upload
                .put_part(chunk.into())
                .await
                .with_context(|| format!("s3 put_part {key}"))?;
            transferred += filled as u64;
            if last_emit.elapsed() > Duration::from_millis(100) {
                self.update(id, |t| t.transferred = transferred).await;
                if let Some(t) = self.get(id).await {
                    let _ = app.emit("transfer://progress", &t);
                }
                last_emit = Instant::now();
            }
            if filled < buf.len() {
                break;
            }
        }
        upload
            .complete()
            .await
            .with_context(|| format!("s3 complete multipart {key}"))?;
        self.update(id, |t| t.transferred = transferred).await;
        Ok(())
    }

    /// Stream a WebDAV download: a single ranged-capable `GET`, written to the
    /// local file chunk by chunk so progress is real (not 0→size at the end).
    async fn run_webdav_download(
        &self,
        id: &str,
        session: Arc<WebdavSession>,
        remote_path: &str,
        local_path: &Path,
        app: &AppHandle,
    ) -> Result<()> {
        use futures::StreamExt;

        self.update(id, |t| t.status = TransferStatus::Transferring)
            .await;

        let url = session.url_for(remote_path, false);
        let resp = session
            .request(reqwest::Method::GET, url)
            .send()
            .await
            .with_context(|| format!("GET {remote_path}"))?;
        if !resp.status().is_success() {
            return Err(anyhow::anyhow!(
                "download {remote_path} failed: HTTP {}",
                resp.status().as_u16()
            ));
        }

        let mut file = tokio::fs::File::create(local_path)
            .await
            .with_context(|| format!("create {}", local_path.display()))?;
        let mut stream = resp.bytes_stream();
        let mut transferred: u64 = 0;
        let mut last_emit = Instant::now();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.with_context(|| format!("webdav chunk for {remote_path}"))?;
            self.checkpoint(id, chunk.len() as u64).await?;
            file.write_all(&chunk).await?;
            transferred += chunk.len() as u64;
            if last_emit.elapsed() > Duration::from_millis(100) {
                self.update(id, |t| t.transferred = transferred).await;
                if let Some(t) = self.get(id).await {
                    let _ = app.emit("transfer://progress", &t);
                }
                last_emit = Instant::now();
            }
        }
        file.flush().await?;
        self.update(id, |t| t.transferred = transferred).await;
        Ok(())
    }

    /// Upload via WebDAV `PUT`, streaming the file body straight off disk (no
    /// full-file buffering) with an explicit Content-Length so servers accept it.
    async fn run_webdav_upload(
        &self,
        id: &str,
        session: Arc<WebdavSession>,
        local_path: &Path,
        remote_path: &str,
        app: &AppHandle,
    ) -> Result<()> {
        use tokio_util::io::ReaderStream;

        self.update(id, |t| t.status = TransferStatus::Transferring)
            .await;
        let _ = app; // WebDAV PUT reports at completion, like the FTP path.

        let size = tokio::fs::metadata(local_path)
            .await
            .with_context(|| format!("stat {}", local_path.display()))?
            .len();
        let file = tokio::fs::File::open(local_path)
            .await
            .with_context(|| format!("open {}", local_path.display()))?;
        let body = reqwest::Body::wrap_stream(ReaderStream::new(file));
        self.checkpoint(id, size).await?;

        let url = session.url_for(remote_path, false);
        let resp = session
            .request(reqwest::Method::PUT, url)
            .header(reqwest::header::CONTENT_LENGTH, size)
            .body(body)
            .send()
            .await
            .with_context(|| format!("PUT {remote_path}"))?;
        if !resp.status().is_success() {
            return Err(anyhow::anyhow!(
                "upload {remote_path} failed: HTTP {}",
                resp.status().as_u16()
            ));
        }
        self.update(id, |t| t.transferred = size).await;
        Ok(())
    }

    /// Stream a read-only HTTP download: a single `GET`, written chunk by chunk.
    async fn run_http_download(
        &self,
        id: &str,
        session: Arc<HttpSession>,
        remote_path: &str,
        local_path: &Path,
        app: &AppHandle,
    ) -> Result<()> {
        use futures::StreamExt;

        self.update(id, |t| t.status = TransferStatus::Transferring)
            .await;

        let url = session.url_for(remote_path, false);
        let resp = session
            .request(reqwest::Method::GET, url)
            .send()
            .await
            .with_context(|| format!("GET {remote_path}"))?;
        if !resp.status().is_success() {
            return Err(anyhow::anyhow!(
                "download {remote_path} failed: HTTP {}",
                resp.status().as_u16()
            ));
        }

        let mut file = tokio::fs::File::create(local_path)
            .await
            .with_context(|| format!("create {}", local_path.display()))?;
        let mut stream = resp.bytes_stream();
        let mut transferred: u64 = 0;
        let mut last_emit = Instant::now();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.with_context(|| format!("http chunk for {remote_path}"))?;
            self.checkpoint(id, chunk.len() as u64).await?;
            file.write_all(&chunk).await?;
            transferred += chunk.len() as u64;
            if last_emit.elapsed() > Duration::from_millis(100) {
                self.update(id, |t| t.transferred = transferred).await;
                if let Some(t) = self.get(id).await {
                    let _ = app.emit("transfer://progress", &t);
                }
                last_emit = Instant::now();
            }
        }
        file.flush().await?;
        self.update(id, |t| t.transferred = transferred).await;
        Ok(())
    }

    /// Stream a Dropbox download: POST `/2/files/download` (path in the
    /// `Dropbox-API-Arg` header), written chunk by chunk.
    async fn run_dropbox_download(
        &self,
        id: &str,
        session: Arc<DropboxSession>,
        remote_path: &str,
        local_path: &Path,
        app: &AppHandle,
    ) -> Result<()> {
        use futures::StreamExt;

        self.update(id, |t| t.status = TransferStatus::Transferring)
            .await;

        let dbx = crate::remotefs::dropbox::dropbox_api_path(remote_path);
        let arg = serde_json::json!({ "path": dbx }).to_string();
        let resp = session.content_get("/2/files/download", &arg).await?;

        let mut file = tokio::fs::File::create(local_path)
            .await
            .with_context(|| format!("create {}", local_path.display()))?;
        let mut stream = resp.bytes_stream();
        let mut transferred: u64 = 0;
        let mut last_emit = Instant::now();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.with_context(|| format!("dropbox chunk for {remote_path}"))?;
            self.checkpoint(id, chunk.len() as u64).await?;
            file.write_all(&chunk).await?;
            transferred += chunk.len() as u64;
            if last_emit.elapsed() > Duration::from_millis(100) {
                self.update(id, |t| t.transferred = transferred).await;
                if let Some(t) = self.get(id).await {
                    let _ = app.emit("transfer://progress", &t);
                }
                last_emit = Instant::now();
            }
        }
        file.flush().await?;
        self.update(id, |t| t.transferred = transferred).await;
        Ok(())
    }

    /// Upload a file to Dropbox via `/2/files/upload` (overwrite mode). Simple
    /// single-shot upload; Dropbox caps that at 150 MB, so larger files are
    /// refused with a clear message (chunked upload_session is a follow-up).
    async fn run_dropbox_upload(
        &self,
        id: &str,
        session: Arc<DropboxSession>,
        local_path: &Path,
        remote_path: &str,
        app: &AppHandle,
    ) -> Result<()> {
        use tokio_util::io::ReaderStream;

        self.update(id, |t| t.status = TransferStatus::Transferring)
            .await;
        let _ = app; // Dropbox reports at completion, like the FTP/WebDAV paths.

        let size = tokio::fs::metadata(local_path)
            .await
            .with_context(|| format!("stat {}", local_path.display()))?
            .len();
        const SIMPLE_UPLOAD_MAX: u64 = 150 * 1024 * 1024;
        if size > SIMPLE_UPLOAD_MAX {
            return Err(anyhow::anyhow!(
                "{} exceeds Dropbox's 150 MB single-request upload limit \
                 (chunked upload not yet implemented)",
                local_path.display()
            ));
        }

        let dbx = crate::remotefs::dropbox::dropbox_api_path(remote_path);
        let arg = serde_json::json!({
            "path": dbx, "mode": "overwrite", "autorename": false, "mute": true
        })
        .to_string();
        let url = format!("{}/2/files/upload", session.content_base);

        // Proactive refresh covers the common case; on a hard 401 we refresh and
        // retry once, re-opening the file for a fresh streamed body.
        self.checkpoint(id, size).await?;
        let mut attempt = 0;
        loop {
            let token = session.access_token().await?;
            let file = tokio::fs::File::open(local_path)
                .await
                .with_context(|| format!("open {}", local_path.display()))?;
            let body = reqwest::Body::wrap_stream(ReaderStream::new(file));
            let resp = session
                .client
                .post(&url)
                .bearer_auth(&token)
                .header("Dropbox-API-Arg", &arg)
                .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
                .body(body)
                .send()
                .await
                .with_context(|| format!("PUT {remote_path}"))?;
            if resp.status() == reqwest::StatusCode::UNAUTHORIZED && attempt == 0 {
                attempt += 1;
                session.force_refresh().await?;
                continue;
            }
            if !resp.status().is_success() {
                let code = resp.status().as_u16();
                let text = resp.text().await.unwrap_or_default();
                return Err(anyhow::anyhow!("upload {remote_path} failed ({code}): {text}"));
            }
            break;
        }
        self.update(id, |t| t.transferred = size).await;
        Ok(())
    }

    /// Download a Shopify theme asset. The Assets API answers with the whole
    /// file as JSON (`value`/base64 `attachment`), so there is nothing to
    /// stream — read it, write it out, report at completion like the FTP path.
    async fn run_shopify_download(
        &self,
        id: &str,
        session: Arc<ShopifySession>,
        remote_path: &str,
        local_path: &Path,
        app: &AppHandle,
    ) -> Result<()> {
        self.update(id, |t| t.status = TransferStatus::Transferring)
            .await;
        let _ = app; // single-shot API: progress is reported at completion.

        self.checkpoint(id, 0).await?;
        let data = crate::remotefs::shopify::read_asset(&session, remote_path).await?;
        let mut file = tokio::fs::File::create(local_path)
            .await
            .with_context(|| format!("create {}", local_path.display()))?;
        file.write_all(&data).await?;
        file.flush().await?;
        self.update(id, |t| t.transferred = data.len() as u64).await;
        Ok(())
    }

    /// Upload a file as a Shopify theme asset (create and update are the same
    /// PUT; theme files are small, so a single-shot write is the whole story).
    async fn run_shopify_upload(
        &self,
        id: &str,
        session: Arc<ShopifySession>,
        local_path: &Path,
        remote_path: &str,
        app: &AppHandle,
    ) -> Result<()> {
        self.update(id, |t| t.status = TransferStatus::Transferring)
            .await;
        let _ = app; // Shopify reports at completion, like the Dropbox path.

        let data = tokio::fs::read(local_path)
            .await
            .with_context(|| format!("read {}", local_path.display()))?;
        let size = data.len() as u64;
        self.checkpoint(id, size).await?;
        crate::remotefs::shopify::write_asset(&session, remote_path, &data).await?;
        self.update(id, |t| t.transferred = size).await;
        Ok(())
    }

    /// Download a HubSpot Design Manager file. The Source Code API answers
    /// with the whole file as an octet-stream, so there is nothing to stream —
    /// read it, write it out, report at completion like the Shopify path.
    async fn run_hubspot_download(
        &self,
        id: &str,
        session: Arc<HubSpotSession>,
        remote_path: &str,
        local_path: &Path,
        app: &AppHandle,
    ) -> Result<()> {
        self.update(id, |t| t.status = TransferStatus::Transferring)
            .await;
        let _ = app; // single-shot API: progress is reported at completion.

        self.checkpoint(id, 0).await?;
        let data = crate::remotefs::hubspot::read_file(&session, remote_path).await?;
        let mut file = tokio::fs::File::create(local_path)
            .await
            .with_context(|| format!("create {}", local_path.display()))?;
        file.write_all(&data).await?;
        file.flush().await?;
        self.update(id, |t| t.transferred = data.len() as u64).await;
        Ok(())
    }

    /// Upload a file to the HubSpot Design Manager (create and update are the
    /// same multipart PUT; theme files are small, so a single-shot write is
    /// the whole story).
    async fn run_hubspot_upload(
        &self,
        id: &str,
        session: Arc<HubSpotSession>,
        local_path: &Path,
        remote_path: &str,
        app: &AppHandle,
    ) -> Result<()> {
        self.update(id, |t| t.status = TransferStatus::Transferring)
            .await;
        let _ = app; // HubSpot reports at completion, like the Shopify path.

        let data = tokio::fs::read(local_path)
            .await
            .with_context(|| format!("read {}", local_path.display()))?;
        let size = data.len() as u64;
        self.checkpoint(id, size).await?;
        crate::remotefs::hubspot::write_file(&session, remote_path, &data).await?;
        self.update(id, |t| t.transferred = size).await;
        Ok(())
    }

    /// Download a Dataverse web resource. The Web API answers with the whole
    /// file as base64 `content`, so there is nothing to stream — read it,
    /// write it out, report at completion like the HubSpot path.
    async fn run_dynamics_download(
        &self,
        id: &str,
        session: Arc<DynamicsSession>,
        remote_path: &str,
        local_path: &Path,
        app: &AppHandle,
    ) -> Result<()> {
        self.update(id, |t| t.status = TransferStatus::Transferring)
            .await;
        let _ = app; // single-shot API: progress is reported at completion.

        self.checkpoint(id, 0).await?;
        let data = crate::remotefs::dynamics::read_file(&session, remote_path).await?;
        let mut file = tokio::fs::File::create(local_path)
            .await
            .with_context(|| format!("create {}", local_path.display()))?;
        file.write_all(&data).await?;
        file.flush().await?;
        self.update(id, |t| t.transferred = data.len() as u64).await;
        Ok(())
    }

    /// Upload a file as a Dataverse web resource (create or update by name
    /// lookup, then publish — save = deployed).
    async fn run_dynamics_upload(
        &self,
        id: &str,
        session: Arc<DynamicsSession>,
        local_path: &Path,
        remote_path: &str,
        app: &AppHandle,
    ) -> Result<()> {
        self.update(id, |t| t.status = TransferStatus::Transferring)
            .await;
        let _ = app; // Dynamics reports at completion, like the HubSpot path.

        let data = tokio::fs::read(local_path)
            .await
            .with_context(|| format!("read {}", local_path.display()))?;
        let size = data.len() as u64;
        self.checkpoint(id, size).await?;
        crate::remotefs::dynamics::write_file(&session, remote_path, &data).await?;
        self.update(id, |t| t.transferred = size).await;
        Ok(())
    }

    /// Stream a OneDrive download: GET the item's `/content` (Graph 302s to a
    /// pre-authorized URL, which reqwest follows), written chunk by chunk.
    async fn run_onedrive_download(
        &self,
        id: &str,
        session: Arc<OneDriveSession>,
        remote_path: &str,
        local_path: &Path,
        app: &AppHandle,
    ) -> Result<()> {
        use futures::StreamExt;

        self.update(id, |t| t.status = TransferStatus::Transferring)
            .await;

        let content = crate::remotefs::onedrive::content_ref(remote_path);
        let resp = session.get_stream(&content).await?;

        let mut file = tokio::fs::File::create(local_path)
            .await
            .with_context(|| format!("create {}", local_path.display()))?;
        let mut stream = resp.bytes_stream();
        let mut transferred: u64 = 0;
        let mut last_emit = Instant::now();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.with_context(|| format!("onedrive chunk for {remote_path}"))?;
            self.checkpoint(id, chunk.len() as u64).await?;
            file.write_all(&chunk).await?;
            transferred += chunk.len() as u64;
            if last_emit.elapsed() > Duration::from_millis(100) {
                self.update(id, |t| t.transferred = transferred).await;
                if let Some(t) = self.get(id).await {
                    let _ = app.emit("transfer://progress", &t);
                }
                last_emit = Instant::now();
            }
        }
        file.flush().await?;
        self.update(id, |t| t.transferred = transferred).await;
        Ok(())
    }

    /// Upload to OneDrive: a single `PUT …/content` for small files, or a
    /// chunked upload session for larger ones (Graph caps simple PUT at 4 MB).
    async fn run_onedrive_upload(
        &self,
        id: &str,
        session: Arc<OneDriveSession>,
        local_path: &Path,
        remote_path: &str,
        app: &AppHandle,
    ) -> Result<()> {
        self.update(id, |t| t.status = TransferStatus::Transferring)
            .await;

        let size = tokio::fs::metadata(local_path)
            .await
            .with_context(|| format!("stat {}", local_path.display()))?
            .len();
        // Graph's simple-upload ceiling is 4 MB; overridable for tests.
        let simple_max: u64 = std::env::var("FARO_ONEDRIVE_SIMPLE_MAX")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(4 * 1024 * 1024);

        if size <= simple_max {
            self.checkpoint(id, size).await?;
            self.onedrive_simple_upload(id, &session, local_path, remote_path)
                .await?;
        } else {
            self.onedrive_session_upload(id, &session, local_path, remote_path, size, app)
                .await?;
        }
        self.update(id, |t| t.transferred = size).await;
        Ok(())
    }

    async fn onedrive_simple_upload(
        &self,
        _id: &str,
        session: &Arc<OneDriveSession>,
        local_path: &Path,
        remote_path: &str,
    ) -> Result<()> {
        use tokio_util::io::ReaderStream;
        let content = crate::remotefs::onedrive::content_ref(remote_path);
        let url = format!("{}{content}", session.graph_base);
        let mut attempt = 0;
        loop {
            let token = session.access_token().await?;
            let file = tokio::fs::File::open(local_path)
                .await
                .with_context(|| format!("open {}", local_path.display()))?;
            let body = reqwest::Body::wrap_stream(ReaderStream::new(file));
            let resp = session
                .client
                .put(&url)
                .bearer_auth(&token)
                .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
                .body(body)
                .send()
                .await
                .with_context(|| format!("PUT {remote_path}"))?;
            if resp.status() == reqwest::StatusCode::UNAUTHORIZED && attempt == 0 {
                attempt += 1;
                session.force_refresh().await?;
                continue;
            }
            if !resp.status().is_success() {
                let code = resp.status().as_u16();
                let text = resp.text().await.unwrap_or_default();
                return Err(anyhow::anyhow!("upload {remote_path} failed ({code}): {text}"));
            }
            return Ok(());
        }
    }

    async fn onedrive_session_upload(
        &self,
        id: &str,
        session: &Arc<OneDriveSession>,
        local_path: &Path,
        remote_path: &str,
        size: u64,
        app: &AppHandle,
    ) -> Result<()> {
        // Create the upload session.
        let item = crate::remotefs::onedrive::item_ref(remote_path);
        let create = format!("{item}/createUploadSession");
        let body = serde_json::json!({
            "item": { "@microsoft.graph.conflictBehavior": "replace" }
        });
        let sess = session
            .rpc(reqwest::Method::POST, &create, Some(&body))
            .await?;
        let upload_url = sess
            .get("uploadUrl")
            .and_then(|u| u.as_str())
            .ok_or_else(|| anyhow::anyhow!("createUploadSession returned no uploadUrl"))?
            .to_string();

        // Chunks must be a multiple of 320 KiB (except the last). ~6 MiB.
        const CHUNK: usize = 320 * 1024 * 20;
        let mut file = tokio::fs::File::open(local_path)
            .await
            .with_context(|| format!("open {}", local_path.display()))?;
        let mut buf = vec![0u8; CHUNK];
        let mut offset: u64 = 0;
        let mut last_emit = Instant::now();
        while offset < size {
            let mut filled = 0;
            while filled < buf.len() {
                let n = file.read(&mut buf[filled..]).await?;
                if n == 0 {
                    break;
                }
                filled += n;
            }
            if filled == 0 {
                break;
            }
            self.checkpoint(id, filled as u64).await?;
            let start = offset;
            let end = offset + filled as u64 - 1;
            let range = format!("bytes {start}-{end}/{size}");
            let resp = session
                .client
                .put(&upload_url)
                .header(reqwest::header::CONTENT_LENGTH, filled as u64)
                .header("Content-Range", range)
                .body(bytes::Bytes::copy_from_slice(&buf[..filled]))
                .send()
                .await
                .with_context(|| format!("upload chunk for {remote_path}"))?;
            if !resp.status().is_success() {
                let code = resp.status().as_u16();
                let text = resp.text().await.unwrap_or_default();
                return Err(anyhow::anyhow!("upload {remote_path} chunk failed ({code}): {text}"));
            }
            offset += filled as u64;
            if last_emit.elapsed() > Duration::from_millis(100) {
                self.update(id, |t| t.transferred = offset).await;
                if let Some(t) = self.get(id).await {
                    let _ = app.emit("transfer://progress", &t);
                }
                last_emit = Instant::now();
            }
        }
        Ok(())
    }

    /// Stream a Google Drive download: resolve the path to a file id, then GET
    /// `/files/{id}?alt=media`.
    async fn run_gdrive_download(
        &self,
        id: &str,
        session: Arc<GDriveSession>,
        remote_path: &str,
        local_path: &Path,
        app: &AppHandle,
    ) -> Result<()> {
        use futures::StreamExt;

        self.update(id, |t| t.status = TransferStatus::Transferring)
            .await;

        let (file_id, _) = session
            .resolve_item(remote_path)
            .await?
            .ok_or_else(|| anyhow::anyhow!("{remote_path}: not found"))?;
        let resp = session
            .get_stream(&format!("/files/{file_id}?alt=media"))
            .await?;

        let mut file = tokio::fs::File::create(local_path)
            .await
            .with_context(|| format!("create {}", local_path.display()))?;
        let mut stream = resp.bytes_stream();
        let mut transferred: u64 = 0;
        let mut last_emit = Instant::now();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.with_context(|| format!("drive chunk for {remote_path}"))?;
            self.checkpoint(id, chunk.len() as u64).await?;
            file.write_all(&chunk).await?;
            transferred += chunk.len() as u64;
            if last_emit.elapsed() > Duration::from_millis(100) {
                self.update(id, |t| t.transferred = transferred).await;
                if let Some(t) = self.get(id).await {
                    let _ = app.emit("transfer://progress", &t);
                }
                last_emit = Instant::now();
            }
        }
        file.flush().await?;
        self.update(id, |t| t.transferred = transferred).await;
        Ok(())
    }

    /// Upload to Google Drive: update the existing file's media if a same-named
    /// child exists, else create a new file via a multipart/related request.
    async fn run_gdrive_upload(
        &self,
        id: &str,
        session: Arc<GDriveSession>,
        local_path: &Path,
        remote_path: &str,
        _app: &AppHandle,
    ) -> Result<()> {
        use crate::session::gdrive::{basename, normalize, parent_of};

        self.update(id, |t| t.status = TransferStatus::Transferring)
            .await;

        let size = tokio::fs::metadata(local_path)
            .await
            .with_context(|| format!("stat {}", local_path.display()))?
            .len();
        let norm = normalize(remote_path);
        let name = basename(&norm).to_string();
        let parent_id = session.folder_id(&parent_of(&norm)).await?;
        let existing = session.find_child(&parent_id, &name).await?;
        let bytes = tokio::fs::read(local_path)
            .await
            .with_context(|| format!("read {}", local_path.display()))?;
        let token = session.access_token().await?;
        self.checkpoint(id, size).await?;

        let resp = if let Some((file_id, _)) = existing {
            // Update the existing file's content in place.
            session
                .client
                .patch(format!(
                    "{}/files/{file_id}?uploadType=media",
                    session.upload_base
                ))
                .bearer_auth(&token)
                .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
                .body(bytes)
                .send()
                .await
                .with_context(|| format!("update {remote_path}"))?
        } else {
            // Create a new file: multipart/related metadata + media.
            let boundary = format!("faro{}", Uuid::new_v4().simple());
            let meta = serde_json::json!({ "name": name, "parents": [parent_id] });
            let mut body: Vec<u8> = Vec::with_capacity(bytes.len() + 256);
            body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
            body.extend_from_slice(b"Content-Type: application/json; charset=UTF-8\r\n\r\n");
            body.extend_from_slice(meta.to_string().as_bytes());
            body.extend_from_slice(format!("\r\n--{boundary}\r\n").as_bytes());
            body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
            body.extend_from_slice(&bytes);
            body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
            session
                .client
                .post(format!(
                    "{}/files?uploadType=multipart&fields=id",
                    session.upload_base
                ))
                .bearer_auth(&token)
                .header(
                    reqwest::header::CONTENT_TYPE,
                    format!("multipart/related; boundary={boundary}"),
                )
                .body(body)
                .send()
                .await
                .with_context(|| format!("create {remote_path}"))?
        };

        if !resp.status().is_success() {
            let code = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("upload {remote_path} failed ({code}): {text}"));
        }
        session.clear_cache();
        self.update(id, |t| t.transferred = size).await;
        Ok(())
    }

    /// Stream a Box download: resolve the path to a file id, GET `/files/{id}/content`.
    async fn run_box_download(
        &self,
        id: &str,
        session: Arc<BoxSession>,
        remote_path: &str,
        local_path: &Path,
        app: &AppHandle,
    ) -> Result<()> {
        use futures::StreamExt;

        self.update(id, |t| t.status = TransferStatus::Transferring)
            .await;

        let (file_id, _) = session
            .resolve_item(remote_path)
            .await?
            .ok_or_else(|| anyhow::anyhow!("{remote_path}: not found"))?;
        let resp = session
            .get_stream(&format!("/files/{file_id}/content"))
            .await?;

        let mut file = tokio::fs::File::create(local_path)
            .await
            .with_context(|| format!("create {}", local_path.display()))?;
        let mut stream = resp.bytes_stream();
        let mut transferred: u64 = 0;
        let mut last_emit = Instant::now();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.with_context(|| format!("box chunk for {remote_path}"))?;
            self.checkpoint(id, chunk.len() as u64).await?;
            file.write_all(&chunk).await?;
            transferred += chunk.len() as u64;
            if last_emit.elapsed() > Duration::from_millis(100) {
                self.update(id, |t| t.transferred = transferred).await;
                if let Some(t) = self.get(id).await {
                    let _ = app.emit("transfer://progress", &t);
                }
                last_emit = Instant::now();
            }
        }
        file.flush().await?;
        self.update(id, |t| t.transferred = transferred).await;
        Ok(())
    }

    /// Upload to Box via multipart/form-data: a new file (`/files/content` with
    /// attributes) or a new version of an existing one (`/files/{id}/content`).
    async fn run_box_upload(
        &self,
        id: &str,
        session: Arc<BoxSession>,
        local_path: &Path,
        remote_path: &str,
        _app: &AppHandle,
    ) -> Result<()> {
        use crate::session::boxdrive::{basename, normalize, parent_of};

        self.update(id, |t| t.status = TransferStatus::Transferring)
            .await;

        let size = tokio::fs::metadata(local_path)
            .await
            .with_context(|| format!("stat {}", local_path.display()))?
            .len();
        let norm = normalize(remote_path);
        let name = basename(&norm).to_string();
        let parent_id = session.folder_id(&parent_of(&norm)).await?;
        let existing = session.find_child(&parent_id, &name).await?;
        let bytes = tokio::fs::read(local_path)
            .await
            .with_context(|| format!("read {}", local_path.display()))?;
        let token = session.access_token().await?;
        self.checkpoint(id, size).await?;

        let file_part = reqwest::multipart::Part::bytes(bytes).file_name(name.clone());
        let (url, form) = match existing {
            Some((file_id, false)) => (
                format!("{}/files/{file_id}/content", session.upload_base),
                reqwest::multipart::Form::new().part("file", file_part),
            ),
            _ => {
                let attrs = serde_json::json!({ "name": name, "parent": { "id": parent_id } });
                (
                    format!("{}/files/content", session.upload_base),
                    reqwest::multipart::Form::new()
                        .text("attributes", attrs.to_string())
                        .part("file", file_part),
                )
            }
        };
        let resp = session
            .client
            .post(&url)
            .bearer_auth(&token)
            .multipart(form)
            .send()
            .await
            .with_context(|| format!("upload {remote_path}"))?;
        if !resp.status().is_success() {
            let code = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("upload {remote_path} failed ({code}): {text}"));
        }
        session.clear_cache();
        self.update(id, |t| t.transferred = size).await;
        Ok(())
    }
}

/// Build a RemoteFs handle for the right backend.
fn fs_for_session(session: &Arc<Session>) -> Box<dyn crate::remotefs::RemoteFs> {
    match &**session {
        Session::Ssh(ssh) => Box::new(crate::remotefs::sftp::SftpFs::new(ssh.clone())),
        Session::Ftp(ftp) => Box::new(crate::remotefs::ftp::FtpFs::new(ftp.clone())),
        Session::Object(obj) => {
            Box::new(crate::remotefs::object::ObjectFs::new(obj.clone()))
        }
        Session::Webdav(dav) => Box::new(crate::remotefs::webdav::WebdavFs::new(dav.clone())),
        Session::Http(http) => Box::new(crate::remotefs::http::HttpFs::new(http.clone())),
        Session::Dropbox(dbx) => Box::new(crate::remotefs::dropbox::DropboxFs::new(dbx.clone())),
        Session::OneDrive(od) => Box::new(crate::remotefs::onedrive::OneDriveFs::new(od.clone())),
        Session::GDrive(gd) => Box::new(crate::remotefs::gdrive::GDriveFs::new(gd.clone())),
        Session::Box(bx) => Box::new(crate::remotefs::boxdrive::BoxFs::new(bx.clone())),
        Session::Shopify(sh) => Box::new(crate::remotefs::shopify::ShopifyFs::new(sh.clone())),
        Session::HubSpot(hs) => Box::new(crate::remotefs::hubspot::HubSpotFs::new(hs.clone())),
        Session::Dynamics(dynm) => Box::new(crate::remotefs::dynamics::DynamicsFs::new(dynm.clone())),
        Session::Agent(agent) => Box::new(crate::remotefs::agent::AgentFs::new(agent.clone())),
    }
}

/// HEAD a WebDAV resource, returning its size (from Content-Length) and whether
/// it exists. Best-effort: a server that rejects HEAD reports (0, false).
async fn webdav_head(session: &Arc<WebdavSession>, path: &str) -> (u64, bool) {
    let url = session.url_for(path, false);
    match session.request(reqwest::Method::HEAD, url).send().await {
        Ok(resp) if resp.status().is_success() => {
            let size = resp
                .headers()
                .get(reqwest::header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0);
            (size, true)
        }
        _ => (0, false),
    }
}

/// HEAD an HTTP-source file for its size. Best-effort (0 on any failure).
async fn http_size(session: &Arc<HttpSession>, path: &str) -> u64 {
    let url = session.url_for(path, false);
    match session.request(reqwest::Method::HEAD, url).send().await {
        Ok(resp) if resp.status().is_success() => resp
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0),
        _ => 0,
    }
}

/// Stat a path on a Faro Agent daemon, returning its size and whether it exists.
async fn agent_stat(
    session: &Arc<crate::session::AgentSession>,
    path: &str,
) -> (u64, bool) {
    use faro_agent_proto::msg::{Request, Response};
    match session.request(Request::Stat { path: path.to_string() }).await {
        Ok(Response::Stat { entry }) => (entry.size, true),
        _ => (0, false),
    }
}

/// Lookup the size of a remote file using each backend's native API.
async fn remote_size(session: &Arc<Session>, path: &str) -> Result<u64> {
    match &**session {
        Session::Ssh(ssh) => {
            let cell = ssh.ensure_sftp().await?;
            let sftp = cell.lock().await;
            Ok(sftp
                .metadata(path)
                .await
                .with_context(|| format!("stat {path}"))?
                .size
                .unwrap_or(0))
        }
        Session::Ftp(ftp) => {
            let path = path.to_string();
            let sz = ftp.with_stream(move |s| s.size(&path)).await?;
            Ok(sz as u64)
        }
        Session::Object(obj) => {
            let key = path.trim_start_matches('/').to_string();
            let p = object_store::path::Path::from(key.as_str());
            let meta = obj
                .store
                .head(&p)
                .await
                .with_context(|| format!("object head {key}"))?;
            Ok(meta.size as u64)
        }
        Session::Webdav(dav) => Ok(webdav_head(dav, path).await.0),
        Session::Http(http) => Ok(http_size(http, path).await),
        Session::Dropbox(dbx) => {
            Ok(dbx.size(&crate::remotefs::dropbox::dropbox_api_path(path)).await)
        }
        Session::OneDrive(od) => Ok(od.size(&crate::remotefs::onedrive::item_ref(path)).await),
        Session::GDrive(gd) => Ok(gd.size(path).await),
        Session::Box(bx) => Ok(bx.size(path).await),
        Session::Shopify(sh) => Ok(crate::remotefs::shopify::asset_size(sh, path).await),
        Session::HubSpot(hs) => Ok(crate::remotefs::hubspot::file_size(hs, path).await),
        Session::Dynamics(dynm) => Ok(crate::remotefs::dynamics::file_size(dynm, path).await),
        Session::Agent(agent) => Ok(agent_stat(agent, path).await.0),
    }
}

/// Apply the overwrite policy on the remote side; returns (final_path, skip).
async fn remote_resolve(
    session: &Arc<Session>,
    initial_remote: &str,
    policy: OverwritePolicy,
) -> Result<(String, bool)> {
    match &**session {
        Session::Ssh(ssh) => {
            let cell = ssh.ensure_sftp().await?;
            let sftp = cell.lock().await;
            Ok(match policy {
                OverwritePolicy::Overwrite => (initial_remote.to_string(), false),
                OverwritePolicy::Skip => {
                    let exists = sftp.metadata(initial_remote).await.is_ok();
                    (initial_remote.to_string(), exists)
                }
                OverwritePolicy::Rename => {
                    let renamed = resolve_remote_rename(&sftp, initial_remote).await;
                    (renamed, false)
                }
            })
        }
        Session::Ftp(ftp) => {
            let probe = initial_remote.to_string();
            let exists = ftp.with_stream(move |s| Ok(s.size(&probe).is_ok())).await?;
            Ok(match policy {
                OverwritePolicy::Overwrite => (initial_remote.to_string(), false),
                OverwritePolicy::Skip => (initial_remote.to_string(), exists),
                OverwritePolicy::Rename if !exists => (initial_remote.to_string(), false),
                OverwritePolicy::Rename => {
                    let mut candidate = initial_remote.to_string();
                    let session = ftp.clone();
                    for i in 1..=999 {
                        let (stem, ext) = split_ext(initial_remote);
                        candidate = format!("{stem}_{i}{ext}");
                        let probe = candidate.clone();
                        let found = session
                            .with_stream(move |s| Ok(s.size(&probe).is_ok()))
                            .await?;
                        if !found {
                            break;
                        }
                    }
                    (candidate, false)
                }
            })
        }
        Session::Object(obj) => {
            let key = initial_remote.trim_start_matches('/').to_string();
            let probe = object_store::path::Path::from(key.as_str());
            let exists = obj.store.head(&probe).await.is_ok();
            Ok(match policy {
                OverwritePolicy::Overwrite => (initial_remote.to_string(), false),
                OverwritePolicy::Skip => (initial_remote.to_string(), exists),
                OverwritePolicy::Rename if !exists => (initial_remote.to_string(), false),
                OverwritePolicy::Rename => {
                    let mut candidate = initial_remote.to_string();
                    for i in 1..=999 {
                        let (stem, ext) = split_ext(initial_remote);
                        candidate = format!("{stem}_{i}{ext}");
                        let key = candidate.trim_start_matches('/');
                        let p = object_store::path::Path::from(key);
                        if obj.store.head(&p).await.is_err() {
                            break;
                        }
                    }
                    (candidate, false)
                }
            })
        }
        Session::Webdav(dav) => {
            let (_, exists) = webdav_head(dav, initial_remote).await;
            Ok(match policy {
                OverwritePolicy::Overwrite => (initial_remote.to_string(), false),
                OverwritePolicy::Skip => (initial_remote.to_string(), exists),
                OverwritePolicy::Rename if !exists => (initial_remote.to_string(), false),
                OverwritePolicy::Rename => {
                    let mut candidate = initial_remote.to_string();
                    for i in 1..=999 {
                        let (stem, ext) = split_ext(initial_remote);
                        candidate = format!("{stem}_{i}{ext}");
                        if !webdav_head(dav, &candidate).await.1 {
                            break;
                        }
                    }
                    (candidate, false)
                }
            })
        }
        Session::Http(_) => {
            // Read-only: no upload will actually run (run_http_upload errors), so
            // resolution is a no-op that just echoes the target back.
            Ok((initial_remote.to_string(), false))
        }
        Session::Dropbox(dbx) => {
            let dbx_path = crate::remotefs::dropbox::dropbox_api_path(initial_remote);
            let exists = dbx.exists(&dbx_path).await;
            Ok(match policy {
                OverwritePolicy::Overwrite => (initial_remote.to_string(), false),
                OverwritePolicy::Skip => (initial_remote.to_string(), exists),
                OverwritePolicy::Rename if !exists => (initial_remote.to_string(), false),
                OverwritePolicy::Rename => {
                    let mut candidate = initial_remote.to_string();
                    for i in 1..=999 {
                        let (stem, ext) = split_ext(initial_remote);
                        candidate = format!("{stem}_{i}{ext}");
                        let p = crate::remotefs::dropbox::dropbox_api_path(&candidate);
                        if !dbx.exists(&p).await {
                            break;
                        }
                    }
                    (candidate, false)
                }
            })
        }
        Session::OneDrive(od) => {
            let exists = od.exists(&crate::remotefs::onedrive::item_ref(initial_remote)).await;
            Ok(match policy {
                OverwritePolicy::Overwrite => (initial_remote.to_string(), false),
                OverwritePolicy::Skip => (initial_remote.to_string(), exists),
                OverwritePolicy::Rename if !exists => (initial_remote.to_string(), false),
                OverwritePolicy::Rename => {
                    let mut candidate = initial_remote.to_string();
                    for i in 1..=999 {
                        let (stem, ext) = split_ext(initial_remote);
                        candidate = format!("{stem}_{i}{ext}");
                        if !od
                            .exists(&crate::remotefs::onedrive::item_ref(&candidate))
                            .await
                        {
                            break;
                        }
                    }
                    (candidate, false)
                }
            })
        }
        Session::GDrive(gd) => {
            let exists = gd.exists(initial_remote).await;
            Ok(match policy {
                OverwritePolicy::Overwrite => (initial_remote.to_string(), false),
                OverwritePolicy::Skip => (initial_remote.to_string(), exists),
                OverwritePolicy::Rename if !exists => (initial_remote.to_string(), false),
                OverwritePolicy::Rename => {
                    let mut candidate = initial_remote.to_string();
                    for i in 1..=999 {
                        let (stem, ext) = split_ext(initial_remote);
                        candidate = format!("{stem}_{i}{ext}");
                        if !gd.exists(&candidate).await {
                            break;
                        }
                    }
                    (candidate, false)
                }
            })
        }
        Session::Box(bx) => {
            let exists = bx.exists(initial_remote).await;
            Ok(match policy {
                OverwritePolicy::Overwrite => (initial_remote.to_string(), false),
                OverwritePolicy::Skip => (initial_remote.to_string(), exists),
                OverwritePolicy::Rename if !exists => (initial_remote.to_string(), false),
                OverwritePolicy::Rename => {
                    let mut candidate = initial_remote.to_string();
                    for i in 1..=999 {
                        let (stem, ext) = split_ext(initial_remote);
                        candidate = format!("{stem}_{i}{ext}");
                        if !bx.exists(&candidate).await {
                            break;
                        }
                    }
                    (candidate, false)
                }
            })
        }
        Session::Shopify(sh) => {
            let exists = crate::remotefs::shopify::asset_exists(sh, initial_remote).await;
            Ok(match policy {
                OverwritePolicy::Overwrite => (initial_remote.to_string(), false),
                OverwritePolicy::Skip => (initial_remote.to_string(), exists),
                OverwritePolicy::Rename if !exists => (initial_remote.to_string(), false),
                OverwritePolicy::Rename => {
                    let mut candidate = initial_remote.to_string();
                    for i in 1..=999 {
                        let (stem, ext) = split_ext(initial_remote);
                        candidate = format!("{stem}_{i}{ext}");
                        if !crate::remotefs::shopify::asset_exists(sh, &candidate).await {
                            break;
                        }
                    }
                    (candidate, false)
                }
            })
        }
        Session::HubSpot(hs) => {
            let exists = crate::remotefs::hubspot::file_exists(hs, initial_remote).await;
            Ok(match policy {
                OverwritePolicy::Overwrite => (initial_remote.to_string(), false),
                OverwritePolicy::Skip => (initial_remote.to_string(), exists),
                OverwritePolicy::Rename if !exists => (initial_remote.to_string(), false),
                OverwritePolicy::Rename => {
                    let mut candidate = initial_remote.to_string();
                    for i in 1..=999 {
                        let (stem, ext) = split_ext(initial_remote);
                        candidate = format!("{stem}_{i}{ext}");
                        if !crate::remotefs::hubspot::file_exists(hs, &candidate).await {
                            break;
                        }
                    }
                    (candidate, false)
                }
            })
        }
        Session::Dynamics(dynm) => {
            let exists = crate::remotefs::dynamics::file_exists(dynm, initial_remote).await;
            Ok(match policy {
                OverwritePolicy::Overwrite => (initial_remote.to_string(), false),
                OverwritePolicy::Skip => (initial_remote.to_string(), exists),
                OverwritePolicy::Rename if !exists => (initial_remote.to_string(), false),
                OverwritePolicy::Rename => {
                    let mut candidate = initial_remote.to_string();
                    for i in 1..=999 {
                        let (stem, ext) = split_ext(initial_remote);
                        candidate = format!("{stem}_{i}{ext}");
                        if !crate::remotefs::dynamics::file_exists(dynm, &candidate).await {
                            break;
                        }
                    }
                    (candidate, false)
                }
            })
        }
        Session::Agent(agent) => {
            let (_, exists) = agent_stat(agent, initial_remote).await;
            Ok(match policy {
                OverwritePolicy::Overwrite => (initial_remote.to_string(), false),
                OverwritePolicy::Skip => (initial_remote.to_string(), exists),
                OverwritePolicy::Rename if !exists => (initial_remote.to_string(), false),
                OverwritePolicy::Rename => {
                    let mut candidate = initial_remote.to_string();
                    for i in 1..=999 {
                        let (stem, ext) = split_ext(initial_remote);
                        candidate = format!("{stem}_{i}{ext}");
                        if !agent_stat(agent, &candidate).await.1 {
                            break;
                        }
                    }
                    (candidate, false)
                }
            })
        }
    }
}
fn split_ext(path: &str) -> (&str, &str) {
    match path.rfind('.') {
        Some(dot) if dot > path.rfind('/').unwrap_or(0) => (&path[..dot], &path[dot..]),
        _ => (path, ""),
    }
}

async fn finalize(
    mgr: &Arc<TransferManager>,
    id: &str,
    app: &AppHandle,
    result: Result<()>,
) {
    match result {
        Ok(()) => {
            mgr.update(id, |t| {
                t.status = TransferStatus::Done;
                t.transferred = t.size;
            })
            .await;
            if let Some(t) = mgr.get(id).await {
                let _ = app.emit("transfer://done", &t);
            }
        }
        Err(e) => {
            mgr.update(id, |t| {
                t.status = TransferStatus::Error;
                t.error = Some(e.to_string());
            })
            .await;
            if let Some(t) = mgr.get(id).await {
                let _ = app.emit("transfer://error", &t);
            }
        }
    }
    mgr.tasks.lock().await.remove(id);
}

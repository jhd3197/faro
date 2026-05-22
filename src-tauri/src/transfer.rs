use crate::session::{FtpSession, Session, SshSession};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;
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
    Done,
    Skipped,
    Error,
    Canceled,
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

pub struct TransferManager {
    transfers: Mutex<HashMap<String, Transfer>>,
    tasks: Mutex<HashMap<String, JoinHandle<()>>>,
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

    async fn update<F: FnOnce(&mut Transfer)>(&self, id: &str, f: F) {
        if let Some(t) = self.transfers.lock().await.get_mut(id) {
            f(t);
        }
    }

    async fn insert(&self, t: Transfer) {
        self.transfers.lock().await.insert(t.id.clone(), t);
    }

    pub async fn cancel(&self, id: &str) -> Result<()> {
        if let Some(h) = self.tasks.lock().await.remove(id) {
            h.abort();
        }
        self.update(id, |t| {
            if t.status == TransferStatus::Transferring || t.status == TransferStatus::Queued {
                t.status = TransferStatus::Canceled;
            }
        })
        .await;
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

        let mgr = Arc::clone(self);
        let id_for_task = id.clone();
        let task = tokio::spawn(async move {
            let res = match &*session {
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
            };
            finalize(&mgr, &id_for_task, &app, res).await;
        });
        self.tasks.lock().await.insert(id.clone(), task);
        Ok(id)
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

        let mgr = Arc::clone(self);
        let id_for_task = id.clone();
        let task = tokio::spawn(async move {
            let res = match &*session {
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
            };
            finalize(&mgr, &id_for_task, &app, res).await;
        });
        self.tasks.lock().await.insert(id.clone(), task);
        Ok(id)
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
}

/// Build a RemoteFs handle for the right backend.
fn fs_for_session(session: &Arc<Session>) -> Box<dyn crate::remotefs::RemoteFs> {
    match &**session {
        Session::Ssh(ssh) => Box::new(crate::remotefs::sftp::SftpFs::new(ssh.clone())),
        Session::Ftp(ftp) => Box::new(crate::remotefs::ftp::FtpFs::new(ftp.clone())),
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
                    // FTP has no fast stat; probe candidates linearly.
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

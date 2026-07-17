use crate::session::{
    DropboxSession, FtpSession, GDriveSession, HttpSession, ObjectSession, OneDriveSession,
    Session, SshSession, WebdavSession,
};
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
            finalize(&mgr, &id_for_task, &app, res).await;
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
            finalize(&mgr, &id_for_task, &app, res).await;
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

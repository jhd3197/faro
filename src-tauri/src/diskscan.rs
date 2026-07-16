//! Disk Usage Explorer scan engine (see `docs/plans/4_disk-usage-explorer.md`).
//!
//! A WinDirStat/WizTree-style "what's eating the disk?" analysis, but over **any**
//! backend — because every `RemoteFs` already knows how to walk. A [`ScanManager`]
//! (shaped like `TransferManager` / `agent_host`: an `Arc` in `AppState`, running
//! scans in a `Mutex<HashMap<scanId, …>>`) recursively sizes a directory and
//! aggregates the totals into a tree the frontend renders as a treemap + a
//! size-ranked list.
//!
//! Phase 1 is the **generic walk** — [`crate::scan::walk`], the one shared scan
//! engine, summed up the tree. It works on every backend and is the fallback for
//! the protocol fast paths (shell `du` for SSH/agent, object-store flat listing)
//! that layer on in Phase 3 via [`ScanStrategy`].
//!
//! Progress streams over `diskscan://…` events (mirrors `transfer://…`); cancel
//! flips a shared [`CancelToken`]. The aggregated tree is fetched once, on
//! completion, via `diskscan_tree`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::remotefs::{FileKind, RemoteFs};
use crate::scan::{self, CancelToken, ScanOptions, ScanProgress};

/// Which strategy produced a scan. Phase 1 only ever reports `Generic`; the
/// exec (`Shell`) and object-store (`ObjectFlat`) fast paths land in Phase 3 and
/// set this so the UI can show which path fired (and why the generic fallback,
/// if exec was denied).
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ScanStrategy {
    /// Recursive `RemoteFs::list_dir` walk — always available.
    Generic,
    /// One `du -ab` / `find -printf` over the SSH exec channel or a Faro Agent.
    Shell,
    /// A single flat object listing under the prefix (S3/Azure).
    ObjectFlat,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ScanState {
    Scanning,
    Done,
    Error,
    Canceled,
}

/// One node in the aggregated size tree. `size` is the *total* bytes under this
/// node (own bytes for a file; sum of descendants for a directory). Directories
/// carry `children`; files have none. Children are sorted largest-first so the
/// treemap and ranked list render the "biggest offenders" without re-sorting.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuNode {
    pub name: String,
    pub path: String,
    pub kind: FileKind,
    pub size: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<DuNode>,
}

/// A snapshot of a scan for the frontend: live progress while `Scanning`, the
/// aggregated `tree` once `Done`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanSnapshot {
    pub id: String,
    pub session_id: String,
    pub root: String,
    pub state: ScanState,
    pub strategy: ScanStrategy,
    pub dirs_scanned: usize,
    pub files_found: usize,
    pub total_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tree: Option<DuNode>,
    pub started_at: i64,
}

/// The lightweight progress event body streamed over `diskscan://progress`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProgressEvent {
    id: String,
    dirs_scanned: usize,
    files_found: usize,
    total_bytes: u64,
    strategy: ScanStrategy,
}

/// Shared live state for one scan. The task mutates the atomics as it walks; the
/// query commands read them — no lock is held across the walk.
struct ScanInfo {
    id: String,
    session_id: String,
    root: String,
    strategy: StdMutex<ScanStrategy>,
    cancel: CancelToken,
    dirs: AtomicUsize,
    files: AtomicUsize,
    bytes: AtomicU64,
    started_at: i64,
    /// The terminal result — `None` until the scan settles.
    result: StdMutex<RunResult>,
}

enum RunResult {
    Running,
    Done(DuNode),
    Error(String),
    Canceled,
}

impl ScanInfo {
    fn snapshot(&self) -> ScanSnapshot {
        let strategy = *self.strategy.lock().unwrap();
        let (state, error, tree) = match &*self.result.lock().unwrap() {
            RunResult::Running => (ScanState::Scanning, None, None),
            RunResult::Done(tree) => (ScanState::Done, None, Some(tree.clone())),
            RunResult::Error(e) => (ScanState::Error, Some(e.clone()), None),
            RunResult::Canceled => (ScanState::Canceled, None, None),
        };
        ScanSnapshot {
            id: self.id.clone(),
            session_id: self.session_id.clone(),
            root: self.root.clone(),
            state,
            strategy,
            dirs_scanned: self.dirs.load(Ordering::Relaxed),
            files_found: self.files.load(Ordering::Relaxed),
            total_bytes: self.bytes.load(Ordering::Relaxed),
            error,
            tree,
            started_at: self.started_at,
        }
    }
}

struct ScanHandle {
    info: Arc<ScanInfo>,
    task: tauri::async_runtime::JoinHandle<()>,
}

pub struct ScanManager {
    scans: Mutex<HashMap<String, ScanHandle>>,
}

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl ScanManager {
    pub fn new() -> Self {
        Self { scans: Mutex::new(HashMap::new()) }
    }

    /// Kick off a scan of `root` on `fs`. Returns the scan id immediately; the
    /// walk runs in a spawned task streaming `diskscan://progress` and, on
    /// settle, one of `diskscan://done` / `diskscan://error` / `diskscan://canceled`.
    pub async fn start(
        self: &Arc<Self>,
        session_id: String,
        root: String,
        fs: Box<dyn RemoteFs>,
        app: AppHandle,
    ) -> String {
        let id = Uuid::new_v4().to_string();
        let info = Arc::new(ScanInfo {
            id: id.clone(),
            session_id,
            root: root.clone(),
            strategy: StdMutex::new(ScanStrategy::Generic),
            cancel: CancelToken::new(),
            dirs: AtomicUsize::new(0),
            files: AtomicUsize::new(0),
            bytes: AtomicU64::new(0),
            started_at: now_ts(),
            result: StdMutex::new(RunResult::Running),
        });

        let task_info = Arc::clone(&info);
        let mgr = Arc::clone(self);
        let task = tauri::async_runtime::spawn(async move {
            run_scan(task_info, root, fs, app).await;
            // Leave the handle in the map so `diskscan_tree`/`_status` can fetch
            // the settled result; the frontend evicts it when the tab closes.
            let _ = mgr;
        });

        self.scans
            .lock()
            .await
            .insert(id.clone(), ScanHandle { info, task });
        id
    }

    pub async fn snapshot(&self, id: &str) -> Option<ScanSnapshot> {
        self.scans.lock().await.get(id).map(|h| h.info.snapshot())
    }

    /// Flip the cancel flag; the walk stops at its next tick and reports Canceled.
    pub async fn cancel(&self, id: &str) {
        if let Some(h) = self.scans.lock().await.get(id) {
            h.info.cancel.cancel();
        }
    }

    /// Drop a finished (or abandoned) scan — the tab was closed. Aborts the task
    /// if it's somehow still running.
    pub async fn forget(&self, id: &str) {
        if let Some(h) = self.scans.lock().await.remove(id) {
            h.info.cancel.cancel();
            h.task.abort();
        }
    }
}

/// The generic-walk scan body. Streams progress, builds the aggregated tree, and
/// records the terminal result on `info`.
async fn run_scan(info: Arc<ScanInfo>, root: String, fs: Box<dyn RemoteFs>, app: AppHandle) {
    let opts = ScanOptions { concurrency: scan::DEFAULT_CONCURRENCY, cancel: info.cancel.clone() };

    // Throttle progress events to ~10/s so a fast local walk doesn't flood the
    // IPC channel. The atomics are always current for the query commands.
    let mut last_emit = Instant::now();
    let ev_info = Arc::clone(&info);
    let ev_app = app.clone();
    let on_progress = move |p: ScanProgress| {
        ev_info.dirs.store(p.dirs_scanned, Ordering::Relaxed);
        ev_info.files.store(p.files_found, Ordering::Relaxed);
        ev_info.bytes.store(p.bytes_found, Ordering::Relaxed);
        if last_emit.elapsed() >= Duration::from_millis(100) {
            let _ = ev_app.emit(
                "diskscan://progress",
                ProgressEvent {
                    id: ev_info.id.clone(),
                    dirs_scanned: p.dirs_scanned,
                    files_found: p.files_found,
                    total_bytes: p.bytes_found,
                    strategy: *ev_info.strategy.lock().unwrap(),
                },
            );
            last_emit = Instant::now();
        }
    };

    let walked = scan::walk(fs.as_ref(), &root, &opts, on_progress).await;

    // Cancellation wins even if the walk returned an (empty/partial) Ok tree.
    if info.cancel.is_cancelled() {
        *info.result.lock().unwrap() = RunResult::Canceled;
        let _ = app.emit("diskscan://canceled", info.snapshot());
        return;
    }

    match walked {
        Ok(tree) => {
            let node = build_tree(&root, &tree);
            info.bytes.store(node.size, Ordering::Relaxed);
            *info.result.lock().unwrap() = RunResult::Done(node);
            let _ = app.emit("diskscan://done", info.snapshot());
        }
        Err(e) => {
            *info.result.lock().unwrap() = RunResult::Error(format!("{e:#}"));
            let _ = app.emit("diskscan://error", info.snapshot());
        }
    }
}

/// The basename of a POSIX-ish path, tolerating a trailing slash. Empty → "/".
fn basename(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return "/".to_string();
    }
    trimmed
        .rsplit(|c| c == '/' || c == '\\')
        .next()
        .unwrap_or(trimmed)
        .to_string()
}

/// Intermediate builder: a directory-or-file with children keyed by segment name
/// and a running aggregate `size`. Converted to a sorted [`DuNode`] at the end.
#[derive(Default)]
struct Build {
    size: u64,
    is_dir: bool,
    children: HashMap<String, Build>,
}

/// Fold the flat file list into a nested, size-aggregated tree.
fn build_tree(root: &str, tree: &scan::ScanTree) -> DuNode {
    let mut builder = Build { is_dir: true, ..Default::default() };
    for (rel, entry) in &tree.files {
        let segments: Vec<&str> = rel.split('/').filter(|s| !s.is_empty()).collect();
        if segments.is_empty() {
            continue;
        }
        builder.size += entry.size;
        insert_path(&mut builder, &segments, entry.size);
    }
    let root_path = root.trim_end_matches('/');
    let root_path = if root_path.is_empty() { "/" } else { root_path };
    convert(basename(root), root_path.to_string(), FileKind::Directory, builder)
}

fn insert_path(node: &mut Build, segments: &[&str], size: u64) {
    let (first, rest) = segments.split_first().expect("non-empty segments");
    let child = node.children.entry((*first).to_string()).or_default();
    child.size += size;
    if rest.is_empty() {
        // Leaf file (unless a directory of the same name already claimed it).
    } else {
        child.is_dir = true;
        insert_path(child, rest, size);
    }
}

fn convert(name: String, path: String, kind: FileKind, build: Build) -> DuNode {
    let mut children: Vec<DuNode> = build
        .children
        .into_iter()
        .map(|(seg, child)| {
            let child_path = if path == "/" {
                format!("/{seg}")
            } else {
                format!("{path}/{seg}")
            };
            let child_kind = if child.is_dir { FileKind::Directory } else { FileKind::File };
            convert(seg, child_path, child_kind, child)
        })
        .collect();
    // Largest-first: the biggest offenders lead in both the treemap and the list.
    children.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name)));
    DuNode { name, path, kind, size: build.size, children }
}

// ---------- Tauri commands ----------

use tauri::State;

/// Start a disk-usage scan of `path` on `session_id`. Returns the scan id; the
/// frontend then listens for `diskscan://…` and fetches the tree on `done`.
#[tauri::command]
pub async fn diskscan_start(
    session_id: String,
    path: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let fs = crate::commands::fs_for_public(&session_id, &state).await?;
    let mgr = Arc::clone(&state.diskscan);
    Ok(mgr.start(session_id, path, fs, app).await)
}

#[tauri::command]
pub async fn diskscan_status(
    scan_id: String,
    state: State<'_, AppState>,
) -> Result<ScanSnapshot, String> {
    state
        .diskscan
        .snapshot(&scan_id)
        .await
        .ok_or_else(|| format!("scan {scan_id} not found"))
}

/// Full snapshot including the aggregated `tree` (present once the scan is done).
#[tauri::command]
pub async fn diskscan_tree(
    scan_id: String,
    state: State<'_, AppState>,
) -> Result<ScanSnapshot, String> {
    state
        .diskscan
        .snapshot(&scan_id)
        .await
        .ok_or_else(|| format!("scan {scan_id} not found"))
}

#[tauri::command]
pub async fn diskscan_cancel(scan_id: String, state: State<'_, AppState>) -> Result<(), String> {
    state.diskscan.cancel(&scan_id).await;
    Ok(())
}

#[tauri::command]
pub async fn diskscan_forget(scan_id: String, state: State<'_, AppState>) -> Result<(), String> {
    state.diskscan.forget(&scan_id).await;
    Ok(())
}

use crate::AppState;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::{ScanEntry, ScanTree};

    fn entry(size: u64) -> ScanEntry {
        ScanEntry { absolute: String::new(), size, modified: 0, etag: None }
    }

    #[test]
    fn aggregates_sizes_up_the_tree() {
        let mut tree = ScanTree::default();
        tree.files.insert("a/b.txt".into(), entry(10));
        tree.files.insert("a/c.txt".into(), entry(20));
        tree.files.insert("d.txt".into(), entry(5));

        let root = build_tree("/data", &tree);
        assert_eq!(root.size, 35);
        assert_eq!(root.path, "/data");
        // Children sorted largest-first: dir `a` (30) before file `d.txt` (5).
        assert_eq!(root.children.len(), 2);
        assert_eq!(root.children[0].name, "a");
        assert_eq!(root.children[0].size, 30);
        assert_eq!(root.children[0].kind, FileKind::Directory);
        assert_eq!(root.children[0].path, "/data/a");
        assert_eq!(root.children[1].name, "d.txt");
        assert_eq!(root.children[1].size, 5);
        assert_eq!(root.children[1].kind, FileKind::File);

        // The dir's own children are sorted too: c.txt (20) before b.txt (10).
        let a = &root.children[0];
        assert_eq!(a.children[0].name, "c.txt");
        assert_eq!(a.children[0].size, 20);
        assert_eq!(a.children[1].name, "b.txt");
    }

    #[test]
    fn empty_tree_yields_empty_root() {
        let tree = ScanTree::default();
        let root = build_tree("/x/", &tree);
        assert_eq!(root.size, 0);
        assert!(root.children.is_empty());
        assert_eq!(root.path, "/x");
    }

    #[test]
    fn root_slash_child_paths() {
        let mut tree = ScanTree::default();
        tree.files.insert("etc/hosts".into(), entry(1));
        let root = build_tree("/", &tree);
        assert_eq!(root.path, "/");
        assert_eq!(root.children[0].path, "/etc");
        assert_eq!(root.children[0].children[0].path, "/etc/hosts");
    }
}

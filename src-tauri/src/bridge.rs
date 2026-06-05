//! Agent Bridge — a localhost HTTP endpoint that lets a local AI agent (Claude
//! Code, Cursor, …) operate on the user's servers through Faro's already-
//! authenticated sessions, without installing anything on the remote server or
//! handing the agent any credentials.
//!
//! Capabilities exposed to the agent (REST + MCP):
//!   * `exec` — run a shell command (SSH only),
//!   * `list_dir` / `read_file` / `search` — inspect the remote filesystem,
//!   * `download` / `upload` — move files through Faro's transfer engine,
//!   * `server_info` / `list_sessions` — context about what's connected.
//!
//! Security model:
//!   * bound to 127.0.0.1 only,
//!   * guarded by a per-launch bearer token,
//!   * every session must be explicitly opted in ("Allow agent access"),
//!   * each side-effecting request is approved interactively in the Faro UI
//!     (emit event → await oneshot → resolve), UNLESS the user has relaxed the
//!     approval policy (allow-all, auto-approve read-only ops, or auto-approve
//!     read-only shell commands). The policy + the per-profile allow-list are
//!     persisted to `bridge.json` so they survive restarts/reconnects.

use crate::AppState;
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{oneshot, Mutex};
use uuid::Uuid;

use crate::remotefs::FileKind;
use crate::session::{ExecStream, Session};
use crate::transfer::OverwritePolicy;

const APPROVAL_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_ACTIVITY: usize = 200;
const MAX_BODY: usize = 1024 * 1024; // 1 MiB request bodies
const MAX_HEADERS: usize = 64 * 1024;
const MAX_READ_FILE: usize = 256 * 1024; // cap faro_read_file output at 256 KiB
const MAX_EXEC_BYTES: usize = 512 * 1024; // cap exec output at 512 KiB
const EXEC_TIMEOUT: Duration = Duration::from_secs(60); // kill a hung/streaming command
const SEARCH_MAX_RESULTS: usize = 200;
const SEARCH_MAX_DEPTH: usize = 6;
const SEARCH_MAX_DIRS: usize = 4000; // hard ceiling on directories visited

/// Filename of the endpoint discovery file written next to `bridge.json` while
/// the bridge is running. `faro-cli agent …` reads the URL + token from it, so
/// the AI never has to handle either. Deleted when the bridge stops.
const DISCOVERY_FILE: &str = "agent-endpoint.json";

/// How much approval friction the user has opted to remove. Every field
/// defaults to `false` — i.e. the original "approve everything" behaviour.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ApprovalPolicy {
    /// Master switch — approve every agent request on enabled sessions.
    pub allow_all: bool,
    /// Auto-approve read-only operations (list_dir, read_file, search). Note:
    /// download/upload are treated as writes (they touch a local/remote disk).
    pub auto_read: bool,
    /// Auto-approve shell commands that look read-only (best-effort heuristic).
    pub auto_safe_exec: bool,
}

/// On-disk shape of `bridge.json`. Session ids are per-connect UUIDs, so the
/// allow-list is keyed by *profile* id and re-applied when a session connects.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct PersistedConfig {
    /// Master on/off switch. Default false (serde `default` => old configs
    /// without the key stay off), so local work exposes nothing until opted in.
    enabled: bool,
    enabled_profiles: Vec<String>,
    policy: ApprovalPolicy,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeStatus {
    pub running: bool,
    /// Master switch state (persisted). When false nothing listens at all.
    pub enabled: bool,
    pub url: Option<String>,
    pub port: Option<u16>,
    pub token: Option<String>,
    pub enabled_sessions: Vec<String>,
    pub policy: ApprovalPolicy,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityEntry {
    pub id: String,
    pub session_id: String,
    pub kind: String, // "exec" | "read" | "download" | "upload" | "search" | "denied" | "error"
    pub detail: String,
    pub ok: bool,
    pub at: i64, // unix millis
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequest {
    pub request_id: String,
    pub session_id: String,
    pub session_name: String,
    /// Operation kind ("exec", "read", "download", "upload", "search") so the
    /// UI can phrase the prompt appropriately.
    pub kind: String,
    /// Human-readable summary of what the agent wants to do.
    pub command: String,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalDecision {
    Approve,
    Deny,
}

/// Risk class of an operation, used to decide auto-approval.
#[derive(Clone, Copy, PartialEq, Eq)]
enum OpClass {
    Read,
    Write,
    Exec,
}

struct Running {
    port: u16,
    token: String,
    /// Path of the discovery file written on start, removed on stop.
    endpoint_path: PathBuf,
    shutdown: Option<oneshot::Sender<()>>,
}

#[derive(Default)]
pub struct BridgeState {
    running: Mutex<Option<Running>>,
    /// Master on/off switch (the persisted `enabled` key). Default false:
    /// while off the bridge never starts and no token/discovery file exists.
    enabled_master: Mutex<bool>,
    /// Runtime allow-list, keyed by session id (per-connect UUID).
    enabled: Mutex<HashSet<String>>,
    /// Persistent allow-list, keyed by profile id; re-applied on connect.
    enabled_profiles: Mutex<HashSet<String>>,
    policy: Mutex<ApprovalPolicy>,
    approvals: Mutex<HashMap<String, oneshot::Sender<ApprovalDecision>>>,
    activity: Mutex<Vec<ActivityEntry>>,
    config_path: Option<PathBuf>,
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn activity(kind: &str, session_id: &str, detail: String, ok: bool) -> ActivityEntry {
    ActivityEntry {
        id: Uuid::new_v4().to_string(),
        session_id: session_id.to_string(),
        kind: kind.to_string(),
        detail,
        ok,
        at: now_millis(),
    }
}

/// Publish the live endpoint (`{url, port, token, pid, version}`) so the CLI can
/// reach the bridge without the user or the agent ever copying a URL or token.
/// On Unix the file is locked down to the owner (0600); on Windows it inherits
/// the per-user `%APPDATA%` ACL — the same trust boundary `profiles.json`
/// already relies on (any process running as this user can read it).
fn write_discovery_file(path: &PathBuf, port: u16, token: &str) {
    let body = json!({
        "url": format!("http://127.0.0.1:{port}"),
        "port": port,
        "token": token,
        "pid": std::process::id(),
        "version": env!("CARGO_PKG_VERSION"),
    });
    let Ok(bytes) = serde_json::to_vec_pretty(&body) else {
        return;
    };
    if std::fs::write(path, &bytes).is_ok() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
    }
}

/// Best-effort classifier: is this shell command read-only? Conservative by
/// design — anything with shell chaining/redirection, a known command-runner
/// (which could exec an arbitrary mutating command via args alone), or a binary
/// not on the curated safe list, is treated as *not* read-only (so it still
/// needs approval unless allow-all is on). This is a heuristic, never a security
/// boundary — the binary+args space is too large to fully reason about, so we
/// only auto-approve commands we're confident neither mutate state nor launch
/// another program. Anything else falls back to interactive approval.
fn is_read_only_command(cmd: &str) -> bool {
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return false;
    }
    // Reject anything that could chain/redirect/substitute into a mutating command.
    const DANGEROUS: &[&str] = &[
        ";", "&&", "||", "|", ">", "<", "`", "$(", "${", "&", "\n", "\r",
    ];
    for d in DANGEROUS {
        if trimmed.contains(d) {
            return false;
        }
    }
    let first = trimmed.split_whitespace().next().unwrap_or("");
    let bin = first.rsplit('/').next().unwrap_or(first);
    // Binaries that run another program or mutate system/network state using
    // only flags/args (no shell metacharacters) — e.g. `env CMD`, `ip netns
    // exec <ns> CMD`, `date -s`, `dmesg -C`. These can never be auto-approved.
    const NEVER_SAFE: &[&str] = &[
        "env", "sudo", "doas", "su", "xargs", "watch", "timeout", "nohup",
        "nice", "ionice", "time", "ssh", "scp", "rsync", "sh", "bash", "zsh",
        "dash", "ksh", "fish", "perl", "python", "python3", "ruby", "node",
        "php", "lua", "eval", "exec", "find", "ip", "ifconfig", "iptables",
        "nft", "date", "hostname", "hostnamectl", "dmesg", "ss", "systemctl",
        "service", "kill", "pkill", "killall", "mount", "umount", "modprobe",
        "sysctl", "tee", "dd", "crontab",
    ];
    if NEVER_SAFE.contains(&bin) {
        return false;
    }
    // Curated binaries that neither mutate state nor launch other programs with
    // their normal invocations. Deliberately excludes find/sed/awk/sort/uniq
    // (write/exec/-i/-o flags) and anything in NEVER_SAFE.
    const READ_ONLY: &[&str] = &[
        "ls", "cat", "pwd", "whoami", "id", "uname", "uptime", "df", "du",
        "free", "ps", "stat", "head", "tail", "wc", "file", "echo", "printenv",
        "which", "type", "tree", "readlink", "realpath", "dirname", "basename",
        "lsblk", "lscpu", "netstat", "cut", "grep", "egrep", "fgrep", "md5sum",
        "sha1sum", "sha256sum", "groups", "getent", "lsof", "nproc", "vmstat",
        "cal", "arch", "lsusb", "lspci", "uptime",
    ];
    READ_ONLY.contains(&bin)
}

impl BridgeState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Load (or initialise) bridge config from the app data dir. Mirrors
    /// `ProfileStore::load_or_create`. Falls back to defaults on any error.
    pub fn load_or_create(app: &AppHandle) -> Result<Self> {
        let dir = app
            .path()
            .app_data_dir()
            .context("resolving app_data_dir")?;
        std::fs::create_dir_all(&dir).ok();
        let path = dir.join("bridge.json");
        let cfg: PersistedConfig = if path.exists() {
            std::fs::read(&path)
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default()
        } else {
            PersistedConfig::default()
        };
        Ok(Self {
            enabled_master: Mutex::new(cfg.enabled),
            enabled_profiles: Mutex::new(cfg.enabled_profiles.into_iter().collect()),
            policy: Mutex::new(cfg.policy),
            config_path: Some(path),
            ..Default::default()
        })
    }

    async fn persist(&self) {
        let Some(path) = self.config_path.as_ref() else {
            return;
        };
        let cfg = PersistedConfig {
            enabled: *self.enabled_master.lock().await,
            enabled_profiles: self.enabled_profiles.lock().await.iter().cloned().collect(),
            policy: *self.policy.lock().await,
        };
        if let Ok(bytes) = serde_json::to_vec_pretty(&cfg) {
            let _ = std::fs::write(path, bytes);
        }
    }

    pub async fn status(&self) -> BridgeStatus {
        let running = self.running.lock().await;
        let enabled = *self.enabled_master.lock().await;
        let enabled_sessions = self.enabled.lock().await.iter().cloned().collect();
        let policy = *self.policy.lock().await;
        match running.as_ref() {
            Some(r) => BridgeStatus {
                running: true,
                enabled,
                url: Some(format!("http://127.0.0.1:{}", r.port)),
                port: Some(r.port),
                token: Some(r.token.clone()),
                enabled_sessions,
                policy,
            },
            None => BridgeStatus {
                running: false,
                enabled,
                url: None,
                port: None,
                token: None,
                enabled_sessions,
                policy,
            },
        }
    }

    pub async fn start(self: &Arc<Self>, app: AppHandle) -> Result<BridgeStatus> {
        if self.running.lock().await.is_some() {
            return Ok(self.status().await);
        }
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let port = listener.local_addr()?.port();
        let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());

        // Publish the live endpoint so `faro-cli agent …` can find it with no
        // URL/token handling by the user or the agent. Re-resolve the data dir
        // from `app` here (not from config_path) so it works even on the
        // load_or_create default-fallback path where config_path is None.
        let dir = app
            .path()
            .app_data_dir()
            .unwrap_or_else(|_| PathBuf::from("."));
        std::fs::create_dir_all(&dir).ok();
        let endpoint_path = dir.join(DISCOVERY_FILE);
        write_discovery_file(&endpoint_path, port, &token);

        let (tx, rx) = oneshot::channel();

        let state = self.clone();
        let app_for_task = app.clone();
        let token_for_task = token.clone();
        tokio::spawn(async move {
            serve(app_for_task, state, listener, token_for_task, rx).await;
        });

        *self.running.lock().await = Some(Running {
            port,
            token,
            endpoint_path,
            shutdown: Some(tx),
        });
        Ok(self.status().await)
    }

    pub async fn stop(&self) {
        if let Some(mut r) = self.running.lock().await.take() {
            // Pull the published endpoint so the CLI immediately sees "not
            // running" instead of a stale URL/token.
            let _ = std::fs::remove_file(&r.endpoint_path);
            if let Some(tx) = r.shutdown.take() {
                let _ = tx.send(());
            }
        }
    }

    /// The master on/off switch the UI exposes (persisted, default off). Turning
    /// it on starts the bridge (binding a port, minting a token, publishing the
    /// discovery file) and makes it auto-start on the next launch; turning it
    /// off stops the bridge and removes the token/discovery file, so local work
    /// exposes nothing. Per-session grants are left intact across a toggle.
    pub async fn set_enabled(self: &Arc<Self>, app: AppHandle, on: bool) -> Result<BridgeStatus> {
        *self.enabled_master.lock().await = on;
        self.persist().await;
        if on {
            self.start(app).await?;
        } else {
            self.stop().await;
        }
        Ok(self.status().await)
    }

    /// Called once on launch: bring the bridge up only if the user left the
    /// master switch on, so the AI path survives app restarts.
    pub async fn auto_start_if_enabled(self: &Arc<Self>, app: AppHandle) {
        if *self.enabled_master.lock().await {
            if let Err(e) = self.start(app).await {
                tracing::warn!(?e, "agent bridge auto-start failed");
            }
        }
    }

    /// Grant/revoke agent access for a session. Mirrors the grant onto the
    /// persistent, profile-keyed allow-list so it survives reconnects.
    pub async fn set_access(&self, app: &AppHandle, session_id: &str, enabled: bool) {
        {
            let mut set = self.enabled.lock().await;
            if enabled {
                set.insert(session_id.to_string());
            } else {
                set.remove(session_id);
            }
        }
        if let Some(sess) = app.state::<AppState>().sessions.get(session_id).await {
            let profile_id = sess.profile().id.clone();
            let mut profs = self.enabled_profiles.lock().await;
            if enabled {
                profs.insert(profile_id);
            } else {
                profs.remove(&profile_id);
            }
        }
        self.persist().await;
    }

    /// Called when a session connects: auto-enable it if its profile was
    /// previously granted access.
    pub async fn on_session_connected(&self, session_id: &str, profile_id: &str) {
        if self.enabled_profiles.lock().await.contains(profile_id) {
            self.enabled.lock().await.insert(session_id.to_string());
        }
    }

    pub async fn set_policy(&self, policy: ApprovalPolicy) {
        *self.policy.lock().await = policy;
        self.persist().await;
    }

    async fn is_enabled(&self, session_id: &str) -> bool {
        self.enabled.lock().await.contains(session_id)
    }

    /// Decide whether the current policy auto-approves an operation.
    async fn auto_approved(&self, class: OpClass, exec_cmd: Option<&str>) -> bool {
        let p = *self.policy.lock().await;
        if p.allow_all {
            return true;
        }
        match class {
            OpClass::Read => p.auto_read,
            OpClass::Write => false,
            OpClass::Exec => {
                p.auto_safe_exec && exec_cmd.map(is_read_only_command).unwrap_or(false)
            }
        }
    }

    pub async fn resolve_approval(
        &self,
        request_id: &str,
        decision: ApprovalDecision,
    ) -> Result<()> {
        let tx = self
            .approvals
            .lock()
            .await
            .remove(request_id)
            .ok_or_else(|| anyhow!("no pending approval {request_id}"))?;
        tx.send(decision)
            .map_err(|_| anyhow!("approval receiver dropped"))?;
        Ok(())
    }

    /// Emit a `bridge://approval` event and block until the user clicks
    /// Approve/Deny (or it times out). Returns true only on explicit approval.
    async fn request_approval(
        &self,
        app: &AppHandle,
        session_id: &str,
        session_name: &str,
        kind: &str,
        summary: &str,
    ) -> bool {
        let request_id = Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        self.approvals.lock().await.insert(request_id.clone(), tx);
        let _ = app.emit(
            "bridge://approval",
            ApprovalRequest {
                request_id: request_id.clone(),
                session_id: session_id.to_string(),
                session_name: session_name.to_string(),
                kind: kind.to_string(),
                command: summary.to_string(),
            },
        );
        let approved = matches!(
            tokio::time::timeout(APPROVAL_TIMEOUT, rx).await,
            Ok(Ok(ApprovalDecision::Approve))
        );
        // If it timed out, the entry is still in the map — clean it up.
        self.approvals.lock().await.remove(&request_id);
        approved
    }

    async fn log(&self, app: &AppHandle, entry: ActivityEntry) {
        {
            let mut v = self.activity.lock().await;
            v.push(entry.clone());
            let len = v.len();
            if len > MAX_ACTIVITY {
                v.drain(0..len - MAX_ACTIVITY);
            }
        }
        let _ = app.emit("bridge://activity", entry);
    }

    pub async fn recent_activity(&self) -> Vec<ActivityEntry> {
        self.activity.lock().await.clone()
    }
}

/// Opt-in check → policy/approval gate. On success the caller proceeds; on
/// failure it returns the contained `(status, json)` response. Denials are
/// logged here so callers only log their own success/error paths.
async fn gate(
    app: &AppHandle,
    state: &Arc<BridgeState>,
    session_id: &str,
    session_name: &str,
    class: OpClass,
    kind: &str,
    summary: &str,
    exec_cmd: Option<&str>,
) -> Result<(), (u16, Value)> {
    if !state.is_enabled(session_id).await {
        return Err((403, json!({"error": "session has not granted agent access"})));
    }
    if state.auto_approved(class, exec_cmd).await {
        return Ok(());
    }
    if state
        .request_approval(app, session_id, session_name, kind, summary)
        .await
    {
        Ok(())
    } else {
        state
            .log(app, activity("denied", session_id, summary.to_string(), false))
            .await;
        Err((403, json!({"error": "request was denied or timed out"})))
    }
}

// ---- HTTP server (hand-rolled, localhost-only, one request per connection) ----

async fn serve(
    app: AppHandle,
    state: Arc<BridgeState>,
    listener: TcpListener,
    token: String,
    mut shutdown: oneshot::Receiver<()>,
) {
    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            accepted = listener.accept() => {
                let Ok((stream, _peer)) = accepted else { continue };
                let app = app.clone();
                let state = state.clone();
                let token = token.clone();
                tokio::spawn(async move {
                    let _ = handle_conn(app, state, stream, token).await;
                });
            }
        }
    }
}

struct Request {
    method: String,
    path: String,
    auth: Option<String>,
    body: Vec<u8>,
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

async fn read_request(stream: &mut TcpStream) -> Result<Request> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let header_end = loop {
        if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
            break pos + 4;
        }
        if buf.len() > MAX_HEADERS {
            return Err(anyhow!("request headers too large"));
        }
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            return Err(anyhow!("connection closed before headers"));
        }
        buf.extend_from_slice(&tmp[..n]);
    };

    let header_text = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = header_text.split("\r\n");
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();

    let mut content_length = 0usize;
    let mut auth = None;
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim().to_ascii_lowercase();
            let val = v.trim();
            if key == "content-length" {
                content_length = val.parse().unwrap_or(0);
            } else if key == "authorization" {
                auth = Some(val.to_string());
            }
        }
    }
    if content_length > MAX_BODY {
        return Err(anyhow!("request body too large"));
    }

    let mut body = buf[header_end..].to_vec();
    while body.len() < content_length {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    body.truncate(content_length);

    Ok(Request {
        method,
        path,
        auth,
        body,
    })
}

async fn write_response(stream: &mut TcpStream, status: u16, body: &Value) -> Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        _ => "Internal Server Error",
    };
    let payload = serde_json::to_vec(body)?;
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n",
        payload.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(&payload).await?;
    stream.flush().await?;
    Ok(())
}

async fn handle_conn(
    app: AppHandle,
    state: Arc<BridgeState>,
    mut stream: TcpStream,
    token: String,
) -> Result<()> {
    let req = match read_request(&mut stream).await {
        Ok(r) => r,
        Err(_) => {
            return write_response(&mut stream, 400, &json!({"error": "bad request"})).await;
        }
    };

    let authed = req
        .auth
        .as_deref()
        .and_then(|a| a.strip_prefix("Bearer "))
        .map(|t| t == token)
        .unwrap_or(false);
    if !authed {
        return write_response(&mut stream, 401, &json!({"error": "unauthorized"})).await;
    }

    // MCP (Streamable HTTP) lives on /mcp and manages its own JSON-RPC framing.
    if req.path == "/mcp" {
        if req.method == "POST" {
            return handle_mcp(&app, &state, &req.body, &mut stream).await;
        }
        // The optional GET server→client SSE stream is unsupported; clients
        // fall back gracefully when it returns 405.
        return write_empty(&mut stream, 405).await;
    }

    let (status, body) = route(&app, &state, &req).await;
    write_response(&mut stream, status, &body).await
}

async fn route(app: &AppHandle, state: &Arc<BridgeState>, req: &Request) -> (u16, Value) {
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/health") => (
            200,
            json!({"ok": true, "name": "faro-agent-bridge", "version": env!("CARGO_PKG_VERSION")}),
        ),
        ("GET", "/sessions") => handle_sessions(app, state).await,
        ("POST", "/exec") => handle_exec(app, state, &req.body).await,
        ("POST", "/list") => handle_list(app, state, &req.body).await,
        ("POST", "/read") => handle_read(app, state, &req.body).await,
        ("POST", "/download") => handle_download(app, state, &req.body).await,
        ("POST", "/upload") => handle_upload(app, state, &req.body).await,
        ("POST", "/search") => handle_search(app, state, &req.body).await,
        ("POST", "/info") => handle_info(app, state, &req.body).await,
        ("POST", "/transfer") => handle_transfer_status(app, &req.body).await,
        ("POST", "/history") => handle_history(app, state, &req.body).await,
        _ => (404, json!({"error": "not found"})),
    }
}

async fn handle_sessions(app: &AppHandle, state: &Arc<BridgeState>) -> (u16, Value) {
    let manager = app.state::<AppState>().sessions.clone();
    let enabled: Vec<String> = state.enabled.lock().await.iter().cloned().collect();
    let mut out = Vec::new();
    for id in enabled {
        if let Some(sess) = manager.get(&id).await {
            let p = sess.profile();
            out.push(json!({
                "id": id,
                "name": p.name,
                "host": p.host,
                "protocol": sess.protocol(),
                "canExec": matches!(&*sess, Session::Ssh(_)),
            }));
        }
    }
    (200, json!({ "sessions": out }))
}

fn body_str(v: &Value, key: &str) -> String {
    v.get(key).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

fn parse_body(body: &[u8]) -> Result<Value, (u16, Value)> {
    serde_json::from_slice(body).map_err(|_| (400, json!({"error": "invalid JSON body"})))
}

async fn handle_exec(app: &AppHandle, state: &Arc<BridgeState>, body: &[u8]) -> (u16, Value) {
    let parsed = match parse_body(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let session_id = body_str(&parsed, "sessionId");
    let command = body_str(&parsed, "command");
    if session_id.is_empty() || command.is_empty() {
        return (400, json!({"error": "sessionId and command are required"}));
    }
    exec_on(app, state, &session_id, &command).await
}

async fn handle_list(app: &AppHandle, state: &Arc<BridgeState>, body: &[u8]) -> (u16, Value) {
    let parsed = match parse_body(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let session_id = body_str(&parsed, "sessionId");
    if session_id.is_empty() {
        return (400, json!({"error": "sessionId is required"}));
    }
    let path = match body_str(&parsed, "path") {
        p if p.is_empty() => ".".to_string(),
        p => p,
    };
    op_list_dir(app, state, &session_id, &path).await
}

async fn handle_read(app: &AppHandle, state: &Arc<BridgeState>, body: &[u8]) -> (u16, Value) {
    let parsed = match parse_body(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let session_id = body_str(&parsed, "sessionId");
    let path = body_str(&parsed, "path");
    if session_id.is_empty() || path.is_empty() {
        return (400, json!({"error": "sessionId and path are required"}));
    }
    op_read_file(app, state, &session_id, &path).await
}

async fn handle_download(app: &AppHandle, state: &Arc<BridgeState>, body: &[u8]) -> (u16, Value) {
    let parsed = match parse_body(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let session_id = body_str(&parsed, "sessionId");
    let path = body_str(&parsed, "path");
    if session_id.is_empty() || path.is_empty() {
        return (400, json!({"error": "sessionId and path are required"}));
    }
    let local_dir = parsed
        .get("localDir")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    op_download(app, state, &session_id, &path, local_dir).await
}

async fn handle_upload(app: &AppHandle, state: &Arc<BridgeState>, body: &[u8]) -> (u16, Value) {
    let parsed = match parse_body(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let session_id = body_str(&parsed, "sessionId");
    let local_path = body_str(&parsed, "localPath");
    let remote_dir = body_str(&parsed, "remoteDir");
    if session_id.is_empty() || local_path.is_empty() || remote_dir.is_empty() {
        return (
            400,
            json!({"error": "sessionId, localPath and remoteDir are required"}),
        );
    }
    op_upload(app, state, &session_id, &local_path, &remote_dir).await
}

async fn handle_search(app: &AppHandle, state: &Arc<BridgeState>, body: &[u8]) -> (u16, Value) {
    let parsed = match parse_body(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let session_id = body_str(&parsed, "sessionId");
    let query = body_str(&parsed, "query");
    if session_id.is_empty() || query.is_empty() {
        return (400, json!({"error": "sessionId and query are required"}));
    }
    let root = match body_str(&parsed, "path") {
        p if p.is_empty() => ".".to_string(),
        p => p,
    };
    op_search(app, state, &session_id, &root, &query).await
}

async fn handle_info(app: &AppHandle, state: &Arc<BridgeState>, body: &[u8]) -> (u16, Value) {
    let parsed = match parse_body(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let session_id = body_str(&parsed, "sessionId");
    if session_id.is_empty() {
        return (400, json!({"error": "sessionId is required"}));
    }
    op_server_info(app, state, &session_id).await
}

async fn handle_transfer_status(app: &AppHandle, body: &[u8]) -> (u16, Value) {
    let parsed = match parse_body(body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let transfer_id = body_str(&parsed, "transferId");
    if transfer_id.is_empty() {
        return (400, json!({"error": "transferId is required"}));
    }
    op_transfer_status(app, &transfer_id).await
}

async fn handle_history(app: &AppHandle, state: &Arc<BridgeState>, body: &[u8]) -> (u16, Value) {
    // An empty body (or `{}`) returns the whole recent log.
    let parsed = if body.is_empty() {
        json!({})
    } else {
        match parse_body(body) {
            Ok(v) => v,
            Err(e) => return e,
        }
    };
    let session = parsed
        .get("sessionId")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());
    let limit = parsed.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;
    op_history(app, state, session, limit).await
}

// ---- Operation cores (shared by REST routes and MCP tools) ----

/// Run a non-interactive shell command (SSH only).
async fn exec_on(
    app: &AppHandle,
    state: &Arc<BridgeState>,
    session_id: &str,
    command: &str,
) -> (u16, Value) {
    let manager = app.state::<AppState>().sessions.clone();
    let Some(ssh) = manager.get_ssh(session_id).await else {
        return (
            400,
            json!({"error": "session not found or is not an SSH session"}),
        );
    };
    let session_name = ssh.profile.name.clone();

    if let Err(resp) = gate(
        app,
        state,
        session_id,
        &session_name,
        OpClass::Exec,
        "exec",
        command,
        Some(command),
    )
    .await
    {
        return resp;
    }

    // Tag this run so streamed output + the final audit line correlate in the
    // live agent console.
    let op_id = Uuid::new_v4().to_string();
    let _ = app.emit(
        "agent://exec-start",
        json!({
            "opId": op_id,
            "sessionId": session_id,
            "sessionName": session_name,
            "command": command,
        }),
    );
    let stream = ExecStream {
        app: app.clone(),
        op_id: op_id.clone(),
    };

    match ssh
        .exec_bounded(command, MAX_EXEC_BYTES, EXEC_TIMEOUT, Some(&stream))
        .await
    {
        Ok(out) => {
            let ok = !out.timed_out && out.exit_code.unwrap_or(0) == 0;
            let mut detail = command.to_string();
            if out.truncated {
                detail.push_str("  [output truncated]");
            }
            if out.timed_out {
                detail.push_str("  [timed out]");
            }
            state
                .log(
                    app,
                    ActivityEntry {
                        id: op_id,
                        session_id: session_id.to_string(),
                        kind: "exec".into(),
                        detail,
                        ok,
                        at: now_millis(),
                    },
                )
                .await;
            (
                200,
                json!({
                    "stdout": out.stdout,
                    "stderr": out.stderr,
                    "exitCode": out.exit_code,
                    "truncated": out.truncated,
                    "timedOut": out.timed_out,
                }),
            )
        }
        Err(e) => {
            // Reuse op_id so the live console finalizes the same "running" entry
            // emitted by agent://exec-start (instead of leaving a stuck spinner
            // and appending a duplicate row).
            state
                .log(
                    app,
                    ActivityEntry {
                        id: op_id,
                        session_id: session_id.to_string(),
                        kind: "error".into(),
                        detail: format!("{command} — {e}"),
                        ok: false,
                        at: now_millis(),
                    },
                )
                .await;
            (500, json!({"error": e.to_string()}))
        }
    }
}

async fn op_list_dir(
    app: &AppHandle,
    state: &Arc<BridgeState>,
    session_id: &str,
    path: &str,
) -> (u16, Value) {
    let manager = app.state::<AppState>().sessions.clone();
    let Some(sess) = manager.get(session_id).await else {
        return (400, json!({"error": "session not found"}));
    };
    let name = sess.profile().name.clone();
    if let Err(resp) = gate(
        app,
        state,
        session_id,
        &name,
        OpClass::Read,
        "read",
        &format!("list {path}"),
        None,
    )
    .await
    {
        return resp;
    }
    let fs = crate::commands::fs_for_session(&sess);
    match fs.list_dir(path).await {
        Ok(entries) => {
            state
                .log(
                    app,
                    activity(
                        "read",
                        session_id,
                        format!("list {path} ({} items)", entries.len()),
                        true,
                    ),
                )
                .await;
            (200, json!({ "entries": entries }))
        }
        Err(e) => {
            state
                .log(
                    app,
                    activity("error", session_id, format!("list {path} — {e}"), false),
                )
                .await;
            (500, json!({"error": e.to_string()}))
        }
    }
}

async fn op_read_file(
    app: &AppHandle,
    state: &Arc<BridgeState>,
    session_id: &str,
    path: &str,
) -> (u16, Value) {
    let manager = app.state::<AppState>().sessions.clone();
    let Some(ssh) = manager.get_ssh(session_id).await else {
        return (
            400,
            json!({"error": "read_file currently supports SSH/SFTP sessions — use download for other protocols"}),
        );
    };
    let name = ssh.profile.name.clone();
    if let Err(resp) = gate(
        app,
        state,
        session_id,
        &name,
        OpClass::Read,
        "read",
        &format!("read {path}"),
        None,
    )
    .await
    {
        return resp;
    }

    let sftp_cell = match ssh.ensure_sftp().await {
        Ok(c) => c,
        Err(e) => {
            state
                .log(
                    app,
                    activity("error", session_id, format!("read {path} — {e}"), false),
                )
                .await;
            return (500, json!({"error": e.to_string()}));
        }
    };
    // Open under the lock, then read without holding it (the file handle is
    // independent of the SFTP guard — see transfer.rs).
    let mut file = {
        let sftp = sftp_cell.lock().await;
        match sftp.open(path).await {
            Ok(f) => f,
            Err(e) => {
                state
                    .log(
                        app,
                        activity("error", session_id, format!("read {path} — {e}"), false),
                    )
                    .await;
                return (500, json!({"error": format!("open {path}: {e}")}));
            }
        }
    };

    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = vec![0u8; 64 * 1024];
    let mut truncated = false;
    loop {
        let n = match file.read(&mut chunk).await {
            Ok(n) => n,
            Err(e) => {
                state
                    .log(
                        app,
                        activity("error", session_id, format!("read {path} — {e}"), false),
                    )
                    .await;
                return (500, json!({"error": e.to_string()}));
            }
        };
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() >= MAX_READ_FILE {
            buf.truncate(MAX_READ_FILE);
            truncated = true;
            break;
        }
    }

    let bytes = buf.len();
    let content = String::from_utf8_lossy(&buf).to_string();
    state
        .log(
            app,
            activity("read", session_id, format!("read {path} ({bytes} bytes)"), true),
        )
        .await;
    (
        200,
        json!({ "content": content, "bytes": bytes, "truncated": truncated }),
    )
}

fn default_download_dir(app: &AppHandle) -> String {
    app.path()
        .download_dir()
        .or_else(|_| app.path().app_data_dir())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| ".".to_string())
}

async fn op_download(
    app: &AppHandle,
    state: &Arc<BridgeState>,
    session_id: &str,
    remote_path: &str,
    local_dir: Option<String>,
) -> (u16, Value) {
    let manager = app.state::<AppState>().sessions.clone();
    let Some(sess) = manager.get(session_id).await else {
        return (400, json!({"error": "session not found"}));
    };
    let name = sess.profile().name.clone();
    let local_dir = local_dir.unwrap_or_else(|| default_download_dir(app));
    // Download is a *local write* (to an agent-supplied directory), so it's
    // classed Write — only auto-approved under allow-all, never under auto-read.
    if let Err(resp) = gate(
        app,
        state,
        session_id,
        &name,
        OpClass::Write,
        "download",
        &format!("download {remote_path} → {local_dir}"),
        None,
    )
    .await
    {
        return resp;
    }
    let transfers = app.state::<AppState>().transfers.clone();
    match transfers
        .start_download(
            sess.clone(),
            remote_path.to_string(),
            local_dir.clone(),
            OverwritePolicy::Rename,
            app.clone(),
        )
        .await
    {
        Ok(id) => {
            state
                .log(
                    app,
                    activity(
                        "download",
                        session_id,
                        format!("download {remote_path} → {local_dir}"),
                        true,
                    ),
                )
                .await;
            (
                200,
                json!({ "transferId": id, "localDir": local_dir, "status": "started" }),
            )
        }
        Err(e) => {
            state
                .log(
                    app,
                    activity(
                        "error",
                        session_id,
                        format!("download {remote_path} — {e}"),
                        false,
                    ),
                )
                .await;
            (500, json!({"error": e.to_string()}))
        }
    }
}

async fn op_upload(
    app: &AppHandle,
    state: &Arc<BridgeState>,
    session_id: &str,
    local_path: &str,
    remote_dir: &str,
) -> (u16, Value) {
    let manager = app.state::<AppState>().sessions.clone();
    let Some(sess) = manager.get(session_id).await else {
        return (400, json!({"error": "session not found"}));
    };
    let name = sess.profile().name.clone();
    if let Err(resp) = gate(
        app,
        state,
        session_id,
        &name,
        OpClass::Write,
        "upload",
        &format!("upload {local_path} → {remote_dir}"),
        None,
    )
    .await
    {
        return resp;
    }
    let transfers = app.state::<AppState>().transfers.clone();
    match transfers
        .start_upload(
            sess.clone(),
            local_path.to_string(),
            remote_dir.to_string(),
            OverwritePolicy::Rename,
            app.clone(),
        )
        .await
    {
        Ok(id) => {
            state
                .log(
                    app,
                    activity(
                        "upload",
                        session_id,
                        format!("upload {local_path} → {remote_dir}"),
                        true,
                    ),
                )
                .await;
            (200, json!({ "transferId": id, "status": "started" }))
        }
        Err(e) => {
            state
                .log(
                    app,
                    activity(
                        "error",
                        session_id,
                        format!("upload {local_path} — {e}"),
                        false,
                    ),
                )
                .await;
            (500, json!({"error": e.to_string()}))
        }
    }
}

async fn op_search(
    app: &AppHandle,
    state: &Arc<BridgeState>,
    session_id: &str,
    root: &str,
    query: &str,
) -> (u16, Value) {
    let manager = app.state::<AppState>().sessions.clone();
    let Some(sess) = manager.get(session_id).await else {
        return (400, json!({"error": "session not found"}));
    };
    let name = sess.profile().name.clone();
    if let Err(resp) = gate(
        app,
        state,
        session_id,
        &name,
        OpClass::Read,
        "search",
        &format!("search \"{query}\" in {root}"),
        None,
    )
    .await
    {
        return resp;
    }

    let fs = crate::commands::fs_for_session(&sess);
    let needle = query.to_lowercase();
    let mut hits: Vec<Value> = Vec::new();
    let mut stack: Vec<(String, usize)> = vec![(root.to_string(), 0)];
    let mut visited = 0usize;
    while let Some((dir, depth)) = stack.pop() {
        if hits.len() >= SEARCH_MAX_RESULTS || visited >= SEARCH_MAX_DIRS {
            break;
        }
        visited += 1;
        let entries = match fs.list_dir(&dir).await {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries {
            if entry.name.to_lowercase().contains(&needle) {
                hits.push(json!({
                    "name": entry.name,
                    "path": entry.path,
                    "kind": entry.kind,
                    "size": entry.size,
                }));
                if hits.len() >= SEARCH_MAX_RESULTS {
                    break;
                }
            }
            if matches!(entry.kind, FileKind::Directory) && depth < SEARCH_MAX_DEPTH {
                stack.push((entry.path.clone(), depth + 1));
            }
        }
    }
    let truncated = hits.len() >= SEARCH_MAX_RESULTS || visited >= SEARCH_MAX_DIRS;
    state
        .log(
            app,
            activity(
                "search",
                session_id,
                format!("search \"{query}\" in {root} ({} hits)", hits.len()),
                true,
            ),
        )
        .await;
    (200, json!({ "matches": hits, "truncated": truncated }))
}

/// Status of a transfer the agent previously started. No gate — the agent owns
/// the (random) transfer id, and this is local metadata about its own action.
async fn op_transfer_status(app: &AppHandle, transfer_id: &str) -> (u16, Value) {
    let transfers = app.state::<AppState>().transfers.clone();
    match transfers.snapshot(transfer_id).await {
        Some(t) => (
            200,
            json!({
                "transferId": t.id,
                "kind": t.kind,
                "status": t.status,
                "source": t.source,
                "destination": t.destination,
                "size": t.size,
                "transferred": t.transferred,
                "error": t.error,
            }),
        ),
        None => (404, json!({"error": "no transfer with that id (it may have been cleared)"})),
    }
}

/// Newest-first view of the agent's own activity log — the commands, reads,
/// transfers and denials that already ran through Faro. The whole log is
/// readable without per-op approval (it's the agent's own audit trail, already
/// scoped by the bearer token); when a `session` filter is given we require
/// that session to be opted-in, mirroring `op_server_info`.
async fn op_history(
    app: &AppHandle,
    state: &Arc<BridgeState>,
    session_filter: Option<&str>,
    limit: usize,
) -> (u16, Value) {
    let limit = limit.clamp(1, MAX_ACTIVITY);
    let filter_id = if let Some(arg) = session_filter {
        match resolve_session(app, state, Some(arg), false).await {
            Ok(id) => {
                if !state.is_enabled(&id).await {
                    return (403, json!({"error": "session has not granted agent access"}));
                }
                Some(id)
            }
            Err(msg) => return (400, json!({ "error": msg })),
        }
    } else {
        None
    };

    let mut entries = state.recent_activity().await; // oldest-first
    entries.reverse(); // newest-first
    let out: Vec<Value> = entries
        .into_iter()
        .filter(|e| filter_id.as_deref().map_or(true, |id| e.session_id == id))
        .take(limit)
        .map(|e| {
            json!({
                "kind": e.kind,
                "detail": e.detail,
                "ok": e.ok,
                "at": e.at,
                "sessionId": e.session_id,
            })
        })
        .collect();
    (200, json!({ "history": out }))
}

/// Context about a session — Faro-local metadata only, so no remote round-trip
/// and no interactive approval. Still gated on the per-session opt-in: we must
/// not leak host/username/port for sessions the user hasn't granted access to.
async fn op_server_info(
    app: &AppHandle,
    state: &Arc<BridgeState>,
    session_id: &str,
) -> (u16, Value) {
    if !state.is_enabled(session_id).await {
        return (403, json!({"error": "session has not granted agent access"}));
    }
    let manager = app.state::<AppState>().sessions.clone();
    let Some(sess) = manager.get(session_id).await else {
        return (400, json!({"error": "session not found"}));
    };
    let p = sess.profile();
    (
        200,
        json!({
            "id": session_id,
            "name": p.name,
            "protocol": sess.protocol(),
            "host": p.host,
            "port": p.port,
            "username": p.username,
            "canExec": matches!(&*sess, Session::Ssh(_)),
            "defaultRemotePath": p.default_remote_path,
        }),
    )
}

// ---- MCP (Model Context Protocol) over Streamable HTTP ----
//
// One stateless JSON-RPC endpoint. Claude Code connects with:
//   claude mcp add --transport http faro http://127.0.0.1:<port>/mcp \
//     --header "Authorization: Bearer <token>"
// and auto-discovers the faro_* tools below.

async fn handle_mcp(
    app: &AppHandle,
    state: &Arc<BridgeState>,
    body: &[u8],
    stream: &mut TcpStream,
) -> Result<()> {
    let req: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => return write_jsonrpc_error(stream, Value::Null, -32700, "parse error").await,
    };

    let id = req.get("id").cloned();
    let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let params = req.get("params").cloned().unwrap_or(Value::Null);

    // Notifications carry no id and expect no response body.
    let Some(id) = id else {
        return write_empty(stream, 202).await;
    };

    let result: Result<Value, (i64, String)> = match method {
        "initialize" => Ok(mcp_initialize(&params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(mcp_tools_list()),
        "tools/call" => Ok(mcp_tools_call(app, state, &params).await),
        other => Err((-32601, format!("method not found: {other}"))),
    };

    match result {
        Ok(value) => write_jsonrpc_result(stream, id, value).await,
        Err((code, message)) => write_jsonrpc_error(stream, id, code, &message).await,
    }
}

fn mcp_initialize(params: &Value) -> Value {
    let version = params
        .get("protocolVersion")
        .and_then(|v| v.as_str())
        .unwrap_or("2024-11-05");
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": {} },
        "serverInfo": {
            "name": "faro-agent-bridge",
            "version": env!("CARGO_PKG_VERSION"),
        },
    })
}

fn mcp_tools_list() -> Value {
    let session_prop = json!({
        "type": "string",
        "description": "Session id or name. Optional when only one session is available."
    });
    json!({
        "tools": [
            {
                "name": "faro_list_sessions",
                "description": "List the user's own servers that they've connected in Faro (a desktop SSH/SFTP client, like FileZilla + PuTTY) and authorized for administration. Call this first to discover session ids/names and which support shell commands (SSH).",
                "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
            },
            {
                "name": "faro_server_info",
                "description": "Get context about one of the user's connected servers: protocol, host, port, username, default remote path, and whether it supports shell commands.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "session": session_prop },
                    "additionalProperties": false
                }
            },
            {
                "name": "faro_exec",
                "description": "Run a shell command on the server the user currently has open in Faro — their own machine, which they have already authenticated to and authorized for administration. This is equivalent to the user typing the command into Faro's built-in terminal: routine server administration on their own infrastructure. The user approves the command in Faro (or has pre-approved this kind in their settings). Returns stdout, stderr and exit code. SSH sessions only.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "The shell command to run. Keep it non-interactive (no pagers/prompts)." },
                        "session": session_prop
                    },
                    "required": ["command"],
                    "additionalProperties": false
                }
            },
            {
                "name": "faro_list_dir",
                "description": "List a directory on a server the user has open in Faro (their own machine; SFTP, FTP or S3). Returns entries with name, path, kind, size and modified time.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Remote directory path. Defaults to \".\"." },
                        "session": session_prop
                    },
                    "additionalProperties": false
                }
            },
            {
                "name": "faro_read_file",
                "description": "Read a text file on the user's connected server (SSH/SFTP only). Output is capped at 256 KiB; check the `truncated` flag.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Remote file path to read." },
                        "session": session_prop
                    },
                    "required": ["path"],
                    "additionalProperties": false
                }
            },
            {
                "name": "faro_search",
                "description": "Find files/directories on the user's connected server whose name contains a substring (case-insensitive), recursively under a root path. Works for all protocols. Bounded in depth and results.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Case-insensitive substring to match against entry names." },
                        "path": { "type": "string", "description": "Root directory to search under. Defaults to \".\"." },
                        "session": session_prop
                    },
                    "required": ["query"],
                    "additionalProperties": false
                }
            },
            {
                "name": "faro_download",
                "description": "Download a file from the user's connected server to their own machine via Faro's transfer engine. Returns a transferId; the transfer also appears in Faro's transfer panel.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Remote file path to download." },
                        "localDir": { "type": "string", "description": "Local destination directory. Defaults to the user's Downloads folder." },
                        "session": session_prop
                    },
                    "required": ["path"],
                    "additionalProperties": false
                }
            },
            {
                "name": "faro_upload",
                "description": "Upload a local file to a directory on the user's connected server via Faro's transfer engine. Returns a transferId.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "localPath": { "type": "string", "description": "Path to the local file to upload." },
                        "remoteDir": { "type": "string", "description": "Remote destination directory." },
                        "session": session_prop
                    },
                    "required": ["localPath", "remoteDir"],
                    "additionalProperties": false
                }
            },
            {
                "name": "faro_transfer_status",
                "description": "Check whether a download/upload (started via faro_download/faro_upload) has finished. Returns status (queued|transferring|done|skipped|error|canceled), bytes transferred, and any error. Poll this after starting a transfer to confirm success.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "transferId": { "type": "string", "description": "The transferId returned by faro_download or faro_upload." }
                    },
                    "required": ["transferId"],
                    "additionalProperties": false
                }
            },
            {
                "name": "faro_history",
                "description": "Review the recent Agent Bridge activity log — the commands, reads, transfers and denials that have already run through Faro on the user's servers, newest first. Use it to recall what you did earlier in this session or to confirm an action was approved. Optionally filter by `session`.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "session": session_prop,
                        "limit": { "type": "integer", "description": "Max entries to return (default 50, max 200)." }
                    },
                    "additionalProperties": false
                }
            }
        ]
    })
}

fn arg_str(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

/// Wrap an op's `(status, json)` result as an MCP tool response.
fn mcp_wrap(res: (u16, Value)) -> Value {
    let (status, body) = res;
    if status == 200 {
        tool_text(serde_json::to_string_pretty(&body).unwrap_or_default())
    } else {
        tool_error(body.get("error").and_then(|v| v.as_str()).unwrap_or("error"))
    }
}

async fn mcp_tools_call(app: &AppHandle, state: &Arc<BridgeState>, params: &Value) -> Value {
    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
    let session_arg = args.get("session").and_then(|v| v.as_str());

    // Tools that don't need a session.
    if name == "faro_list_sessions" {
        let (_, body) = handle_sessions(app, state).await;
        return tool_text(serde_json::to_string_pretty(&body).unwrap_or_default());
    }

    match name {
        "faro_exec" => {
            let Some(command) = arg_str(&args, "command") else {
                return tool_error("`command` is required");
            };
            let session_id = match resolve_session(app, state, session_arg, true).await {
                Ok(id) => id,
                Err(msg) => return tool_error(&msg),
            };
            let (status, body) = exec_on(app, state, &session_id, &command).await;
            if status == 200 {
                tool_text(format_exec_result(&body))
            } else {
                tool_error(body.get("error").and_then(|v| v.as_str()).unwrap_or("error"))
            }
        }
        "faro_server_info" => match resolve_session(app, state, session_arg, false).await {
            Ok(id) => mcp_wrap(op_server_info(app, state, &id).await),
            Err(msg) => tool_error(&msg),
        },
        "faro_list_dir" => {
            let path = arg_str(&args, "path").unwrap_or_else(|| ".".to_string());
            match resolve_session(app, state, session_arg, false).await {
                Ok(id) => mcp_wrap(op_list_dir(app, state, &id, &path).await),
                Err(msg) => tool_error(&msg),
            }
        }
        "faro_read_file" => {
            let Some(path) = arg_str(&args, "path") else {
                return tool_error("`path` is required");
            };
            match resolve_session(app, state, session_arg, true).await {
                Ok(id) => mcp_wrap(op_read_file(app, state, &id, &path).await),
                Err(msg) => tool_error(&msg),
            }
        }
        "faro_search" => {
            let Some(query) = arg_str(&args, "query") else {
                return tool_error("`query` is required");
            };
            let root = arg_str(&args, "path").unwrap_or_else(|| ".".to_string());
            match resolve_session(app, state, session_arg, false).await {
                Ok(id) => mcp_wrap(op_search(app, state, &id, &root, &query).await),
                Err(msg) => tool_error(&msg),
            }
        }
        "faro_download" => {
            let Some(path) = arg_str(&args, "path") else {
                return tool_error("`path` is required");
            };
            let local_dir = arg_str(&args, "localDir");
            match resolve_session(app, state, session_arg, false).await {
                Ok(id) => mcp_wrap(op_download(app, state, &id, &path, local_dir).await),
                Err(msg) => tool_error(&msg),
            }
        }
        "faro_upload" => {
            let (Some(local_path), Some(remote_dir)) =
                (arg_str(&args, "localPath"), arg_str(&args, "remoteDir"))
            else {
                return tool_error("`localPath` and `remoteDir` are required");
            };
            match resolve_session(app, state, session_arg, false).await {
                Ok(id) => mcp_wrap(op_upload(app, state, &id, &local_path, &remote_dir).await),
                Err(msg) => tool_error(&msg),
            }
        }
        "faro_transfer_status" => {
            let Some(transfer_id) = arg_str(&args, "transferId") else {
                return tool_error("`transferId` is required");
            };
            mcp_wrap(op_transfer_status(app, &transfer_id).await)
        }
        "faro_history" => {
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;
            // session_arg is resolved + gated inside op_history only when present.
            mcp_wrap(op_history(app, state, session_arg, limit).await)
        }
        other => tool_error(&format!("unknown tool: {other}")),
    }
}

/// All enabled sessions as (id, name, is_ssh).
async fn enabled_sessions(app: &AppHandle, state: &Arc<BridgeState>) -> Vec<(String, String, bool)> {
    let manager = app.state::<AppState>().sessions.clone();
    let ids: Vec<String> = state.enabled.lock().await.iter().cloned().collect();
    let mut out = Vec::new();
    for id in ids {
        if let Some(sess) = manager.get(&id).await {
            let is_ssh = matches!(&*sess, Session::Ssh(_));
            out.push((id, sess.profile().name.clone(), is_ssh));
        }
    }
    out
}

/// Resolve the `session` argument to a concrete session id. When `require_exec`
/// is set, only SSH sessions are valid (exec/read_file need a shell/SFTP) — but
/// we resolve against ALL enabled sessions first so a non-SSH match yields an
/// accurate "that's not SSH" error rather than a misleading "no match".
async fn resolve_session(
    app: &AppHandle,
    state: &Arc<BridgeState>,
    arg: Option<&str>,
    require_exec: bool,
) -> Result<String, String> {
    let all = enabled_sessions(app, state).await; // (id, name, is_ssh)

    if let Some(a) = arg {
        return match all
            .iter()
            .find(|(id, name, _)| id == a || name.eq_ignore_ascii_case(a))
        {
            Some((id, _, is_ssh)) => {
                if require_exec && !*is_ssh {
                    Err(format!(
                        "session \"{a}\" is not an SSH session — exec and read_file need SSH; use list_dir/download/upload for this server"
                    ))
                } else {
                    Ok(id.clone())
                }
            }
            None => Err(format!("no enabled session matches \"{a}\"")),
        };
    }

    let candidates: Vec<&(String, String, bool)> = all
        .iter()
        .filter(|(_, _, is_ssh)| !require_exec || *is_ssh)
        .collect();
    match candidates.len() {
        0 if require_exec && !all.is_empty() => Err(
            "the enabled session(s) are not SSH — exec and read_file need SSH; use list_dir/download/upload instead"
                .into(),
        ),
        0 => Err(
            "no server has granted agent access — ask the user to enable it in Faro's Agent Bridge panel"
                .into(),
        ),
        1 => Ok(candidates[0].0.clone()),
        _ => {
            let names: Vec<&str> = candidates.iter().map(|(_, n, _)| n.as_str()).collect();
            Err(format!(
                "multiple sessions are available ({}); pass the `session` argument",
                names.join(", ")
            ))
        }
    }
}

fn tool_text(text: String) -> Value {
    json!({ "content": [ { "type": "text", "text": text } ] })
}

fn tool_error(msg: &str) -> Value {
    json!({ "content": [ { "type": "text", "text": format!("Error: {msg}") } ], "isError": true })
}

fn format_exec_result(body: &Value) -> String {
    let stdout = body.get("stdout").and_then(|v| v.as_str()).unwrap_or("");
    let stderr = body.get("stderr").and_then(|v| v.as_str()).unwrap_or("");
    let code = body.get("exitCode").and_then(|v| v.as_i64());
    let truncated = body.get("truncated").and_then(|v| v.as_bool()).unwrap_or(false);
    let timed_out = body.get("timedOut").and_then(|v| v.as_bool()).unwrap_or(false);
    let mut out = format!(
        "exit code: {}\n",
        code.map(|c| c.to_string())
            .unwrap_or_else(|| "unknown".into())
    );
    if timed_out {
        out.push_str("NOTE: command timed out before finishing — output is partial and the exit code is unknown.\n");
    }
    if truncated {
        out.push_str("NOTE: output was truncated (exceeded the size cap).\n");
    }
    if !stdout.is_empty() {
        out.push_str("\n--- stdout ---\n");
        out.push_str(stdout);
    }
    if !stderr.is_empty() {
        out.push_str("\n--- stderr ---\n");
        out.push_str(stderr);
    }
    out
}

async fn write_raw(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
) -> Result<()> {
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        405 => "Method Not Allowed",
        _ => "OK",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    if !body.is_empty() {
        stream.write_all(body).await?;
    }
    stream.flush().await?;
    Ok(())
}

async fn write_empty(stream: &mut TcpStream, status: u16) -> Result<()> {
    write_raw(stream, status, "application/json", b"").await
}

async fn write_jsonrpc_result(stream: &mut TcpStream, id: Value, result: Value) -> Result<()> {
    let env = json!({ "jsonrpc": "2.0", "id": id, "result": result });
    write_raw(stream, 200, "application/json", &serde_json::to_vec(&env)?).await
}

async fn write_jsonrpc_error(
    stream: &mut TcpStream,
    id: Value,
    code: i64,
    message: &str,
) -> Result<()> {
    let env = json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } });
    write_raw(stream, 200, "application/json", &serde_json::to_vec(&env)?).await
}

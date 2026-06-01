//! Agent Bridge — a localhost HTTP endpoint that lets a local AI agent (Claude
//! Code, Cursor, …) run commands through Faro's already-authenticated SSH
//! sessions, without installing anything on the remote server or handing the
//! agent any credentials.
//!
//! Security model:
//!   * bound to 127.0.0.1 only,
//!   * guarded by a per-launch bearer token,
//!   * every session must be explicitly opted in ("Allow agent access"),
//!   * every `exec` requires interactive approval in the Faro UI — mirroring
//!     the host-key prompt flow (emit event → await oneshot → resolve command).

use crate::AppState;
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{oneshot, Mutex};
use uuid::Uuid;

const APPROVAL_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_ACTIVITY: usize = 200;
const MAX_BODY: usize = 1024 * 1024; // 1 MiB request bodies
const MAX_HEADERS: usize = 64 * 1024;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeStatus {
    pub running: bool,
    pub url: Option<String>,
    pub port: Option<u16>,
    pub token: Option<String>,
    pub enabled_sessions: Vec<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityEntry {
    pub id: String,
    pub session_id: String,
    pub kind: String, // "exec" | "denied" | "error"
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
    pub command: String,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalDecision {
    Approve,
    Deny,
}

struct Running {
    port: u16,
    token: String,
    shutdown: Option<oneshot::Sender<()>>,
}

#[derive(Default)]
pub struct BridgeState {
    running: Mutex<Option<Running>>,
    enabled: Mutex<HashSet<String>>,
    approvals: Mutex<HashMap<String, oneshot::Sender<ApprovalDecision>>>,
    activity: Mutex<Vec<ActivityEntry>>,
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl BridgeState {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn status(&self) -> BridgeStatus {
        let running = self.running.lock().await;
        let enabled_sessions = self.enabled.lock().await.iter().cloned().collect();
        match running.as_ref() {
            Some(r) => BridgeStatus {
                running: true,
                url: Some(format!("http://127.0.0.1:{}", r.port)),
                port: Some(r.port),
                token: Some(r.token.clone()),
                enabled_sessions,
            },
            None => BridgeStatus {
                running: false,
                url: None,
                port: None,
                token: None,
                enabled_sessions,
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
            shutdown: Some(tx),
        });
        Ok(self.status().await)
    }

    pub async fn stop(&self) {
        if let Some(mut r) = self.running.lock().await.take() {
            if let Some(tx) = r.shutdown.take() {
                let _ = tx.send(());
            }
        }
    }

    pub async fn set_access(&self, session_id: &str, enabled: bool) {
        let mut set = self.enabled.lock().await;
        if enabled {
            set.insert(session_id.to_string());
        } else {
            set.remove(session_id);
        }
    }

    async fn is_enabled(&self, session_id: &str) -> bool {
        self.enabled.lock().await.contains(session_id)
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
        command: &str,
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
                command: command.to_string(),
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
                "canExec": matches!(&*sess, crate::session::Session::Ssh(_)),
            }));
        }
    }
    (200, json!({ "sessions": out }))
}

async fn handle_exec(app: &AppHandle, state: &Arc<BridgeState>, body: &[u8]) -> (u16, Value) {
    let parsed: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => return (400, json!({"error": "invalid JSON body"})),
    };
    let session_id = parsed
        .get("sessionId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let command = parsed
        .get("command")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if session_id.is_empty() || command.is_empty() {
        return (400, json!({"error": "sessionId and command are required"}));
    }
    if !state.is_enabled(&session_id).await {
        return (403, json!({"error": "session has not granted agent access"}));
    }

    let manager = app.state::<AppState>().sessions.clone();
    let Some(ssh) = manager.get_ssh(&session_id).await else {
        return (
            400,
            json!({"error": "session not found or is not an SSH session"}),
        );
    };
    let session_name = ssh.profile.name.clone();

    if !state
        .request_approval(app, &session_id, &session_name, &command)
        .await
    {
        state
            .log(
                app,
                ActivityEntry {
                    id: Uuid::new_v4().to_string(),
                    session_id: session_id.clone(),
                    kind: "denied".into(),
                    detail: command.clone(),
                    ok: false,
                    at: now_millis(),
                },
            )
            .await;
        return (403, json!({"error": "command was denied or timed out"}));
    }

    match ssh.exec(&command).await {
        Ok(out) => {
            state
                .log(
                    app,
                    ActivityEntry {
                        id: Uuid::new_v4().to_string(),
                        session_id: session_id.clone(),
                        kind: "exec".into(),
                        detail: command.clone(),
                        ok: out.exit_code.unwrap_or(0) == 0,
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
                }),
            )
        }
        Err(e) => {
            state
                .log(
                    app,
                    ActivityEntry {
                        id: Uuid::new_v4().to_string(),
                        session_id: session_id.clone(),
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

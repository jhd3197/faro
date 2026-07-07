//! TCP listener, per-connection handshake, and the request loop.
//!
//! Two entry points:
//!   * [`serve`] — normal operation: accept connections, complete a *paired*
//!     handshake, refuse any peer whose static key isn't pinned, then service
//!     requests under the daemon's policy.
//!   * [`serve_pairing`] — pairing window: accept *pairing* handshakes keyed by a
//!     6-digit code, pin each controller that completes one, and persist.

use crate::config::{config_path, Config};
use crate::ops;
use anyhow::{Context, Result};
use faro_agent_proto::{
    identity::Identity,
    msg::{Hello, Request, Response, PROTOCOL_VERSION},
    pairing, Auth, Role, SecureChannel,
};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

/// Shared daemon state handed to each connection.
#[derive(Clone)]
pub struct Daemon {
    pub identity: Arc<Identity>,
    pub config: Arc<Mutex<Config>>,
    pub config_dir: PathBuf,
}

impl Daemon {
    pub fn new(identity: Identity, config: Config, config_dir: PathBuf) -> Self {
        Self {
            identity: Arc::new(identity),
            config: Arc::new(Mutex::new(config)),
            config_dir,
        }
    }
}

/// Serve paired connections until the listener errors or the process exits.
pub async fn serve(listener: TcpListener, daemon: Daemon) -> Result<()> {
    loop {
        let (stream, peer) = listener.accept().await.context("accept")?;
        let daemon = daemon.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_paired(stream, daemon).await {
                tracing::info!(%peer, "connection closed: {e:#}");
            }
        });
    }
}

/// Complete a paired handshake, verify the peer is pinned, then run its requests.
pub async fn handle_paired(stream: TcpStream, daemon: Daemon) -> Result<()> {
    let sk = daemon.identity.private_bytes()?;
    // We don't know *which* pinned peer is dialing until the handshake reveals
    // their static key, so accept any well-formed XX handshake, then authorise.
    let mut channel =
        SecureChannel::establish(stream, Role::Responder, &sk, Auth::Paired { expect_remote: None })
            .await
            .context("paired handshake")?;

    let peer_key = channel.remote_static_b64();
    let peer_name = {
        let cfg = daemon.config.lock().await;
        if !cfg.is_paired(&peer_key) {
            // Refuse an unpinned peer. It knows nothing of our filesystem; the
            // connection just closes.
            let _ = channel
                .send(&Response::denied(
                    "this controller is not paired with the daemon — run `faro-agentd pair`",
                ))
                .await;
            anyhow::bail!("rejected unpinned peer {peer_key}");
        }
        cfg.peer_name(&peer_key).unwrap_or("paired peer").to_string()
    };

    // First message must be a Hello with a compatible protocol version.
    let hello: Hello = channel.recv().await.context("expected hello")?;
    if hello.protocol_version != PROTOCOL_VERSION {
        let _ = channel
            .send(&Response::error(format!(
                "protocol version mismatch: daemon speaks {PROTOCOL_VERSION}, controller speaks {}",
                hello.protocol_version
            )))
            .await;
        anyhow::bail!("protocol mismatch with {peer_name}");
    }
    tracing::info!(peer = %peer_name, client = %hello.client_name, "controller connected");

    request_loop(&mut channel, &daemon, &peer_name).await;
    Ok(())
}

/// Answer a peer's requests under the daemon's live policy until it hangs up.
/// Shared by the paired path and the post-pairing tail of a pairing connection.
async fn request_loop<S>(channel: &mut SecureChannel<S>, daemon: &Daemon, peer_name: &str)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    loop {
        let req: Request = match channel.recv().await {
            Ok(r) => r,
            Err(_) => break, // peer hung up
        };
        let policy = daemon.config.lock().await.policy;
        let summary = describe(&req);
        let resp = ops::handle(req, policy).await;
        log_op(peer_name, &summary, &resp);
        if channel.send(&resp).await.is_err() {
            break;
        }
    }
}

/// Run a pairing window on `listener`: every controller that completes a
/// handshake with the PSK derived from `code` gets pinned and persisted. Loops
/// until the listener is dropped (the caller decides how long the window stays
/// open). Emits each successful pairing through `on_paired` at the moment of
/// pinning, then keeps answering that controller's requests — it asks for
/// `SystemInfo` right after the ack to name the machine in its confirmation UI.
pub async fn serve_pairing<F>(
    listener: TcpListener,
    daemon: Daemon,
    code: String,
    on_paired: F,
) -> Result<()>
where
    F: Fn(String, String) + Send + Sync + Clone + 'static,
{
    loop {
        let (stream, peer) = listener.accept().await.context("accept")?;
        let daemon = daemon.clone();
        let code = code.clone();
        let on_paired = on_paired.clone();
        tokio::spawn(async move {
            match pair_handshake(stream, &daemon, &code).await {
                Ok((mut channel, name, key)) => {
                    on_paired(name.clone(), key);
                    request_loop(&mut channel, &daemon, &name).await;
                }
                Err(e) => tracing::info!(%peer, "pairing attempt failed: {e:#}"),
            }
        });
    }
}

/// One pairing handshake: authenticate with the code's PSK, learn + pin the
/// controller's static key, persist, and ack. Returns the still-open channel
/// with `(client_name, public_key_b64)` so the caller can keep serving it —
/// dropping the socket here was the v1.3 bug that made every UI pairing report
/// failure (the controller's post-ack `SystemInfo` hit a closed connection).
pub async fn pair_handshake(
    stream: TcpStream,
    daemon: &Daemon,
    code: &str,
) -> Result<(SecureChannel<TcpStream>, String, String)> {
    let sk = daemon.identity.private_bytes()?;
    let psk = pairing::psk_from_code(code);
    let mut channel =
        SecureChannel::establish(stream, Role::Responder, &sk, Auth::Pairing { psk })
            .await
            .context("pairing handshake (wrong code?)")?;

    let peer_key = channel.remote_static_b64();
    let hello: Hello = channel.recv().await.context("expected hello during pairing")?;
    if hello.protocol_version != PROTOCOL_VERSION {
        let _ = channel
            .send(&Response::error(format!(
                "protocol version mismatch: daemon speaks {PROTOCOL_VERSION}, controller speaks {}",
                hello.protocol_version
            )))
            .await;
        anyhow::bail!("protocol mismatch while pairing {}", hello.client_name);
    }

    {
        let mut cfg = daemon.config.lock().await;
        cfg.upsert_peer(&hello.client_name, &peer_key);
        cfg.save(&config_path(&daemon.config_dir))
            .context("persist paired peer")?;
    }

    // Acknowledge so the controller knows it's pinned.
    channel.send(&Response::Ok).await.ok();
    tracing::info!(client = %hello.client_name, key = %peer_key, "paired new controller");
    Ok((channel, hello.client_name, peer_key))
}

/// Pair one connection, then serve it until the peer hangs up. Returns
/// `(client_name, public_key_b64)` once the controller disconnects. Composed
/// from [`pair_handshake`] + the request loop; `serve_pairing` uses the pieces
/// directly so it can notify at pin time.
pub async fn pair_connection(
    stream: TcpStream,
    daemon: Daemon,
    code: &str,
) -> Result<(String, String)> {
    let (mut channel, name, key) = pair_handshake(stream, &daemon, code).await?;
    request_loop(&mut channel, &daemon, &name).await;
    Ok((name, key))
}

/// One-line human summary of a request for the audit log (never includes file
/// bodies or command output).
fn describe(req: &Request) -> String {
    match req {
        Request::Ping => "ping".into(),
        Request::SystemInfo => "system-info".into(),
        Request::ListDir { path } => format!("list {path}"),
        Request::Stat { path } => format!("stat {path}"),
        Request::ReadFile { path, .. } => format!("read {path}"),
        Request::ReadChunk { path, offset, .. } => format!("read-chunk {path}@{offset}"),
        Request::WriteChunk { path, offset, .. } => format!("write-chunk {path}@{offset}"),
        Request::Delete { path, .. } => format!("delete {path}"),
        Request::CreateDir { path } => format!("mkdir {path}"),
        Request::Rename { from, to } => format!("rename {from} -> {to}"),
        Request::Chmod { path, mode } => format!("chmod {path} {mode:o}"),
        Request::Exec { command, .. } => format!("exec: {command}"),
    }
}

fn log_op(peer: &str, summary: &str, resp: &Response) {
    match resp {
        Response::Error { message, denied: true } => {
            tracing::warn!(%peer, "DENIED {summary} — {message}")
        }
        Response::Error { message, denied: false } => {
            tracing::warn!(%peer, "FAILED {summary} — {message}")
        }
        _ => tracing::info!(%peer, "{summary}"),
    }
}

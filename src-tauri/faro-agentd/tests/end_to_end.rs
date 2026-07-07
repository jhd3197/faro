//! End-to-end: a controller pairs with, then drives, a `faro-agentd` over a real
//! TCP loopback connection. Exercises the full stack — framing, Noise handshake,
//! pin check, protocol Hello, and every op class — without a second machine.

use faro_agent_proto::{
    identity::Identity,
    msg::{Hello, Request, Response, PROTOCOL_VERSION},
    Auth, Role, SecureChannel,
};
use faro_agentd::{Config, Daemon};
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

/// Build a daemon with its own identity and temp config dir.
fn make_daemon(config: Config) -> (Daemon, Identity, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let identity = Identity::generate().unwrap();
    let daemon = Daemon {
        identity: Arc::new(identity.clone()),
        config: Arc::new(Mutex::new(config)),
        config_dir: dir.path().to_path_buf(),
    };
    (daemon, identity, dir)
}

/// Connect a controller and complete the paired handshake + Hello.
async fn connect_paired(
    addr: std::net::SocketAddr,
    client_id: &Identity,
    server_pub: Vec<u8>,
) -> SecureChannel<TcpStream> {
    let stream = TcpStream::connect(addr).await.unwrap();
    let ck = client_id.private_bytes().unwrap();
    let mut ch = SecureChannel::establish(
        stream,
        Role::Initiator,
        &ck,
        Auth::Paired { expect_remote: Some(server_pub) },
    )
    .await
    .unwrap();
    ch.send(&Hello {
        protocol_version: PROTOCOL_VERSION,
        client_name: "test-controller".into(),
    })
    .await
    .unwrap();
    ch
}

#[tokio::test]
async fn pair_then_exec_and_transfer() {
    // Daemon starts with no peers; client pairs first.
    let (daemon, server_id, _dir) = make_daemon(Config::default());
    let server_pub = server_id.public_bytes().unwrap();
    let client_id = Identity::generate().unwrap();

    // --- pairing window ---
    let pair_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let pair_addr = pair_listener.local_addr().unwrap();
    let pair_daemon = daemon.clone();
    let pair_srv = tokio::spawn(async move {
        let (stream, _) = pair_listener.accept().await.unwrap();
        faro_agentd::pair_connection(stream, pair_daemon, "135790")
            .await
            .unwrap()
    });

    let code_psk = faro_agent_proto::pairing::psk_from_code("135790");
    let client_stream = TcpStream::connect(pair_addr).await.unwrap();
    let ck = client_id.private_bytes().unwrap();
    let mut pch = SecureChannel::establish(
        client_stream,
        Role::Initiator,
        &ck,
        Auth::Pairing { psk: code_psk },
    )
    .await
    .unwrap();
    pch.send(&Hello {
        protocol_version: PROTOCOL_VERSION,
        client_name: "test-controller".into(),
    })
    .await
    .unwrap();
    let ack: Response = pch.recv().await.unwrap();
    assert!(matches!(ack, Response::Ok), "pairing should be acknowledged");

    // The real controller asks for SystemInfo on the pairing channel right after
    // the ack (to name the machine in its confirmation UI) — the daemon must keep
    // serving the freshly-paired channel instead of dropping it (the v1.3 bug
    // that made every UI pairing report failure).
    pch.send(&Request::SystemInfo).await.unwrap();
    match pch.recv::<Response>().await.unwrap() {
        Response::SystemInfo(info) => assert!(!info.hostname.is_empty()),
        other => panic!("expected system info on the pairing channel, got {other:?}"),
    }

    // pair_connection returns once the controller hangs up.
    drop(pch);
    let (name, key) = pair_srv.await.unwrap();
    assert_eq!(name, "test-controller");
    assert_eq!(key, client_id.public_b64());

    // The daemon persisted the pin.
    assert!(daemon.config.lock().await.is_paired(client_id.public_b64()));

    // --- normal serving ---
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let serve_daemon = daemon.clone();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let _ = faro_agentd::handle_paired(stream, serve_daemon).await;
    });

    let mut ch = connect_paired(addr, &client_id, server_pub).await;

    // Ping
    ch.send(&Request::Ping).await.unwrap();
    assert!(matches!(ch.recv::<Response>().await.unwrap(), Response::Pong));

    // SystemInfo
    ch.send(&Request::SystemInfo).await.unwrap();
    match ch.recv::<Response>().await.unwrap() {
        Response::SystemInfo(info) => {
            assert!(!info.hostname.is_empty());
            assert!(!info.os.is_empty());
        }
        other => panic!("expected system info, got {other:?}"),
    }

    // Exec — a command that prints a known token on every platform.
    let token = "faro_agentd_ok";
    let command = if cfg!(windows) {
        format!("Write-Output {token}")
    } else {
        format!("echo {token}")
    };
    ch.send(&Request::Exec { command, timeout_ms: 15_000, max_bytes: 65536 })
        .await
        .unwrap();
    match ch.recv::<Response>().await.unwrap() {
        Response::Exec { stdout, exit_code, timed_out, .. } => {
            assert!(!timed_out, "command timed out");
            assert_eq!(exit_code, Some(0));
            assert!(stdout.contains(token), "stdout was {stdout:?}");
        }
        other => panic!("expected exec result, got {other:?}"),
    }

    // Write a file, read it back through the chunk path.
    let tmp = tempfile::tempdir().unwrap();
    let file_path = tmp.path().join("hello.txt").to_string_lossy().to_string();
    let payload = b"round trip through faro-agentd".to_vec();
    ch.send(&Request::WriteChunk {
        path: file_path.clone(),
        offset: 0,
        data: base64_encode(&payload),
        truncate: true,
        done: true,
    })
    .await
    .unwrap();
    match ch.recv::<Response>().await.unwrap() {
        Response::Written { bytes } => assert_eq!(bytes, payload.len() as u64),
        other => panic!("expected written, got {other:?}"),
    }

    ch.send(&Request::ReadChunk { path: file_path.clone(), offset: 0, len: 0 })
        .await
        .unwrap();
    match ch.recv::<Response>().await.unwrap() {
        Response::Chunk { data, eof } => {
            assert!(eof);
            assert_eq!(base64_decode(&data), payload);
        }
        other => panic!("expected chunk, got {other:?}"),
    }

    // ListDir should include the file we wrote.
    ch.send(&Request::ListDir { path: tmp.path().to_string_lossy().to_string() })
        .await
        .unwrap();
    match ch.recv::<Response>().await.unwrap() {
        Response::Dir { entries } => {
            assert!(entries.iter().any(|e| e.name == "hello.txt"));
        }
        other => panic!("expected dir, got {other:?}"),
    }
}

#[tokio::test]
async fn read_only_policy_denies_exec_and_writes() {
    let mut config = Config::default();
    config.policy.allow_exec = false;
    config.policy.allow_write = false;
    let (daemon, server_id, _dir) = make_daemon(config);
    let server_pub = server_id.public_bytes().unwrap();
    let client_id = Identity::generate().unwrap();
    daemon
        .config
        .lock()
        .await
        .upsert_peer("test-controller", client_id.public_b64());

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let serve_daemon = daemon.clone();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let _ = faro_agentd::handle_paired(stream, serve_daemon).await;
    });

    let mut ch = connect_paired(addr, &client_id, server_pub).await;

    // Exec denied
    ch.send(&Request::Exec { command: "echo hi".into(), timeout_ms: 5000, max_bytes: 1024 })
        .await
        .unwrap();
    match ch.recv::<Response>().await.unwrap() {
        Response::Error { denied, .. } => assert!(denied, "exec should be policy-denied"),
        other => panic!("expected denial, got {other:?}"),
    }

    // Write denied
    ch.send(&Request::CreateDir { path: "/tmp/should-not-happen".into() })
        .await
        .unwrap();
    match ch.recv::<Response>().await.unwrap() {
        Response::Error { denied, .. } => assert!(denied, "write should be policy-denied"),
        other => panic!("expected denial, got {other:?}"),
    }

    // Reads still allowed
    ch.send(&Request::SystemInfo).await.unwrap();
    assert!(matches!(ch.recv::<Response>().await.unwrap(), Response::SystemInfo(_)));
}

#[tokio::test]
async fn unpinned_peer_is_refused() {
    // Daemon has NO peers; an unknown controller must be rejected.
    let (daemon, server_id, _dir) = make_daemon(Config::default());
    let server_pub = server_id.public_bytes().unwrap();
    let stranger = Identity::generate().unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let serve_daemon = daemon.clone();
    let srv = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        faro_agentd::handle_paired(stream, serve_daemon).await
    });

    let stream = TcpStream::connect(addr).await.unwrap();
    let ck = stranger.private_bytes().unwrap();
    let mut ch = SecureChannel::establish(
        stream,
        Role::Initiator,
        &ck,
        Auth::Paired { expect_remote: Some(server_pub) },
    )
    .await
    .unwrap();
    // The daemon sends a denial then drops the connection.
    let resp: Result<Response, _> = ch.recv().await;
    match resp {
        Ok(Response::Error { denied, .. }) => assert!(denied),
        Ok(other) => panic!("expected denial, got {other:?}"),
        Err(_) => { /* connection closed without a message is also acceptable */ }
    }
    // The server task returns an error for the rejected peer.
    assert!(srv.await.unwrap().is_err());
}

fn base64_encode(b: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(b)
}
fn base64_decode(s: &str) -> Vec<u8> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.decode(s).unwrap()
}

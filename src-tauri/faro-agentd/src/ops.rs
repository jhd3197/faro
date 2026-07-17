//! Native execution of a controller's [`Request`] on this machine.
//!
//! Every side-effecting op is gated by the daemon's [`Policy`] *before* it runs;
//! a refusal comes back as [`Response::denied`] so the controller (and the AI
//! behind it) can tell "the owner disallowed this" apart from "it failed".

use crate::config::Policy;
use base64::Engine as _;
use faro_agent_proto::msg::{
    DirEntry, FileKind, Request, Response, SystemInfo, EXEC_TIMEOUT_MS_MAX, EXEC_TIMEOUT_MS_MIN,
};
use std::path::Path;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

/// Per-`ReadChunk` cap, so one request can't ask the daemon to buffer a whole
/// large file into a single frame. The controller streams a download as many
/// chunks; Faro's transfer engine drives the offsets.
const MAX_CHUNK: u64 = 256 * 1024;

fn b64() -> base64::engine::general_purpose::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

/// Handle one request. `client_name` is only used for logging by the caller.
pub async fn handle(req: Request, policy: Policy) -> Response {
    match req {
        Request::Ping => Response::Pong,
        Request::SystemInfo => Response::SystemInfo(system_info()),
        Request::ListDir { path } => list_dir(&path).await,
        Request::Stat { path } => stat(&path).await,
        Request::ReadFile { path, max_bytes } => read_file(&path, max_bytes).await,
        Request::ReadChunk { path, offset, len } => read_chunk(&path, offset, len).await,

        // --- writes (gated) ---
        Request::WriteChunk { .. }
        | Request::Delete { .. }
        | Request::CreateDir { .. }
        | Request::Rename { .. }
        | Request::Chmod { .. }
            if !policy.allow_write =>
        {
            Response::denied("this machine is read-only for remote agents (writes disabled)")
        }
        Request::WriteChunk { path, offset, data, truncate, done } => {
            write_chunk(&path, offset, &data, truncate, done).await
        }
        Request::Delete { path, recursive } => delete(&path, recursive).await,
        Request::CreateDir { path } => create_dir(&path).await,
        Request::Rename { from, to } => rename(&from, &to).await,
        Request::Chmod { path, mode } => chmod(&path, mode).await,

        // --- exec (gated) ---
        Request::Exec { .. } if !policy.allow_exec => {
            Response::denied("command execution is disabled on this machine")
        }
        Request::Exec { command, timeout_ms, max_bytes } => {
            exec(&command, timeout_ms, max_bytes).await
        }
    }
}

pub fn system_info() -> SystemInfo {
    SystemInfo {
        os: os_name().to_string(),
        hostname: crate::agent_name(),
        arch: std::env::consts::ARCH.to_string(),
        shell: shell_name().to_string(),
        username: whoami_user(),
        home_dir: dirs::home_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default(),
        agentd_version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

fn os_name() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "android") {
        "android"
    } else {
        "linux"
    }
}

fn shell_name() -> &'static str {
    if cfg!(windows) {
        "powershell"
    } else {
        "sh"
    }
}

fn whoami_user() -> String {
    std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "unknown".into())
}

// ---- filesystem ----

async fn list_dir(path: &str) -> Response {
    let mut rd = match tokio::fs::read_dir(path).await {
        Ok(rd) => rd,
        Err(e) => return Response::error(format!("list {path}: {e}")),
    };
    let mut entries = Vec::new();
    loop {
        match rd.next_entry().await {
            Ok(Some(entry)) => {
                let full = entry.path();
                match entry.metadata().await {
                    Ok(md) => entries.push(dir_entry(&full, entry.file_name().to_string_lossy().as_ref(), &md)),
                    // A dangling symlink etc. — surface the name with Other kind.
                    Err(_) => entries.push(DirEntry {
                        name: entry.file_name().to_string_lossy().to_string(),
                        path: full.to_string_lossy().to_string(),
                        kind: FileKind::Other,
                        size: 0,
                        modified: None,
                        mode: None,
                    }),
                }
            }
            Ok(None) => break,
            Err(e) => return Response::error(format!("list {path}: {e}")),
        }
    }
    Response::Dir { entries }
}

async fn stat(path: &str) -> Response {
    let p = Path::new(path);
    match tokio::fs::symlink_metadata(p).await {
        Ok(md) => {
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| path.to_string());
            Response::Stat { entry: dir_entry(p, &name, &md) }
        }
        Err(e) => Response::error(format!("stat {path}: {e}")),
    }
}

fn dir_entry(path: &Path, name: &str, md: &std::fs::Metadata) -> DirEntry {
    let kind = if md.file_type().is_dir() {
        FileKind::Directory
    } else if md.file_type().is_symlink() {
        FileKind::Symlink
    } else if md.file_type().is_file() {
        FileKind::File
    } else {
        FileKind::Other
    };
    let modified = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64);
    DirEntry {
        name: name.to_string(),
        path: path.to_string_lossy().to_string(),
        kind,
        size: md.len(),
        modified,
        mode: file_mode(md),
    }
}

#[cfg(unix)]
fn file_mode(md: &std::fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    Some(md.mode())
}

#[cfg(not(unix))]
fn file_mode(_md: &std::fs::Metadata) -> Option<u32> {
    None
}

async fn read_file(path: &str, max_bytes: u64) -> Response {
    let cap = max_bytes.min(4 * 1024 * 1024) as usize;
    let mut file = match tokio::fs::File::open(path).await {
        Ok(f) => f,
        Err(e) => return Response::error(format!("open {path}: {e}")),
    };
    let mut buf = Vec::new();
    let mut chunk = vec![0u8; 64 * 1024];
    let mut truncated = false;
    loop {
        match file.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.len() >= cap {
                    buf.truncate(cap);
                    truncated = true;
                    break;
                }
            }
            Err(e) => return Response::error(format!("read {path}: {e}")),
        }
    }
    Response::File {
        data: b64().encode(&buf),
        bytes: buf.len() as u64,
        truncated,
    }
}

async fn read_chunk(path: &str, offset: u64, len: u64) -> Response {
    let want = if len == 0 { MAX_CHUNK } else { len.min(MAX_CHUNK) };
    let mut file = match tokio::fs::File::open(path).await {
        Ok(f) => f,
        Err(e) => return Response::error(format!("open {path}: {e}")),
    };
    if let Err(e) = file.seek(std::io::SeekFrom::Start(offset)).await {
        return Response::error(format!("seek {path}: {e}"));
    }
    let mut buf = vec![0u8; want as usize];
    let mut filled = 0usize;
    loop {
        match file.read(&mut buf[filled..]).await {
            Ok(0) => break,
            Ok(n) => {
                filled += n;
                if filled >= buf.len() {
                    break;
                }
            }
            Err(e) => return Response::error(format!("read {path}: {e}")),
        }
    }
    buf.truncate(filled);
    // EOF when we read fewer bytes than a full requested chunk.
    let eof = filled < want as usize;
    Response::Chunk { data: b64().encode(&buf), eof }
}

async fn write_chunk(path: &str, offset: u64, data_b64: &str, truncate: bool, _done: bool) -> Response {
    let data = match b64().decode(data_b64) {
        Ok(d) => d,
        Err(e) => return Response::error(format!("decode chunk: {e}")),
    };
    let mut opts = tokio::fs::OpenOptions::new();
    opts.write(true).create(true);
    if truncate {
        opts.truncate(true);
    }
    let mut file = match opts.open(path).await {
        Ok(f) => f,
        Err(e) => return Response::error(format!("open {path} for write: {e}")),
    };
    if let Err(e) = file.seek(std::io::SeekFrom::Start(offset)).await {
        return Response::error(format!("seek {path}: {e}"));
    }
    if let Err(e) = file.write_all(&data).await {
        return Response::error(format!("write {path}: {e}"));
    }
    if let Err(e) = file.flush().await {
        return Response::error(format!("flush {path}: {e}"));
    }
    Response::Written { bytes: data.len() as u64 }
}

async fn delete(path: &str, recursive: bool) -> Response {
    let meta = match tokio::fs::symlink_metadata(path).await {
        Ok(m) => m,
        Err(e) => return Response::error(format!("delete {path}: {e}")),
    };
    let res = if meta.is_dir() {
        if recursive {
            tokio::fs::remove_dir_all(path).await
        } else {
            tokio::fs::remove_dir(path).await
        }
    } else {
        tokio::fs::remove_file(path).await
    };
    match res {
        Ok(()) => Response::Ok,
        Err(e) => Response::error(format!("delete {path}: {e}")),
    }
}

async fn create_dir(path: &str) -> Response {
    match tokio::fs::create_dir_all(path).await {
        Ok(()) => Response::Ok,
        Err(e) => Response::error(format!("mkdir {path}: {e}")),
    }
}

async fn rename(from: &str, to: &str) -> Response {
    match tokio::fs::rename(from, to).await {
        Ok(()) => Response::Ok,
        Err(e) => Response::error(format!("rename {from} -> {to}: {e}")),
    }
}

#[cfg(unix)]
async fn chmod(path: &str, mode: u32) -> Response {
    use std::os::unix::fs::PermissionsExt;
    match tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).await {
        Ok(()) => Response::Ok,
        Err(e) => Response::error(format!("chmod {path}: {e}")),
    }
}

#[cfg(not(unix))]
async fn chmod(_path: &str, _mode: u32) -> Response {
    // Windows has no POSIX mode bits; treat as a no-op success so the UI's
    // chmod affordance (hidden anyway via capabilities) doesn't hard-error.
    Response::Ok
}

// ---- exec ----

/// Clamp a controller-supplied exec timeout to the SHARED bound the Agent Bridge
/// uses (15 min), not a lower daemon-private cap — otherwise a bridge-accepted
/// `--timeout-ms 900000` gets silently shortened here (Plan 10 Phase 0e).
fn clamp_exec_timeout(timeout_ms: u64) -> Duration {
    Duration::from_millis(timeout_ms.clamp(EXEC_TIMEOUT_MS_MIN, EXEC_TIMEOUT_MS_MAX))
}

async fn exec(command: &str, timeout_ms: u64, max_bytes: u64) -> Response {
    let mut cmd = build_command(command);
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .stdin(std::process::Stdio::null());

    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return Response::error(format!("spawn shell: {e}")),
    };

    let dur = clamp_exec_timeout(timeout_ms);
    let output = tokio::time::timeout(dur, child.wait_with_output()).await;

    let (mut stdout, mut stderr, exit_code, timed_out) = match output {
        Ok(Ok(out)) => (
            out.stdout,
            out.stderr,
            out.status.code(),
            false,
        ),
        Ok(Err(e)) => return Response::error(format!("run command: {e}")),
        // Timed out — the child is dropped, killing it (kill_on_drop below).
        Err(_) => (Vec::new(), b"[timed out]".to_vec(), None, true),
    };

    let cap = max_bytes.clamp(1, 4 * 1024 * 1024) as usize;
    let mut truncated = false;
    if stdout.len() > cap {
        stdout.truncate(cap);
        truncated = true;
    }
    if stderr.len() > cap {
        stderr.truncate(cap);
        truncated = true;
    }

    Response::Exec {
        stdout: String::from_utf8_lossy(&stdout).to_string(),
        stderr: String::from_utf8_lossy(&stderr).to_string(),
        exit_code,
        truncated,
        timed_out,
    }
}

#[cfg(windows)]
fn build_command(command: &str) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("powershell");
    cmd.args(["-NoProfile", "-NonInteractive", "-Command", command]);
    cmd.kill_on_drop(true);
    cmd
}

#[cfg(not(windows))]
fn build_command(command: &str) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("sh");
    cmd.args(["-c", command]);
    cmd.kill_on_drop(true);
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    // Plan 10 Phase 0e: the daemon must clamp to the SAME 15-min ceiling the
    // Agent Bridge accepts, so a `--timeout-ms 900000` isn't silently shortened.
    #[test]
    fn exec_timeout_uses_shared_15min_ceiling() {
        // 15 min passes through unchanged (would have been capped at 10 min before).
        assert_eq!(clamp_exec_timeout(900_000), Duration::from_millis(900_000));
        // A 12-min request survives past the old 10-min cap.
        assert_eq!(clamp_exec_timeout(720_000), Duration::from_millis(720_000));
        // Above the ceiling is clamped down to 15 min, not lower.
        assert_eq!(clamp_exec_timeout(3_600_000), Duration::from_millis(EXEC_TIMEOUT_MS_MAX));
        // Floor still protects against a `1` typo.
        assert_eq!(clamp_exec_timeout(0), Duration::from_millis(EXEC_TIMEOUT_MS_MIN));
    }

    // Plan 10 Phase 1: a multi-line script with nested quotes must run VERBATIM
    // through the daemon's exec path (`sh -c` / `powershell -Command` gets the
    // whole program as one argument, so nothing re-tokenizes it at a shell
    // boundary). This is what `/exec_script` relies on for agent targets.
    #[tokio::test]
    async fn multiline_script_runs_verbatim() {
        let policy = Policy { allow_exec: true, allow_write: true };

        #[cfg(windows)]
        let script = "$msg = \"hello 'world'\"\nWrite-Output $msg\nWrite-Output \"line2\"";
        #[cfg(not(windows))]
        let script = "cat <<'EOF'\nhello 'world'\nline2\nEOF";

        let resp = handle(
            Request::Exec { command: script.to_string(), timeout_ms: 30_000, max_bytes: 65_536 },
            policy,
        )
        .await;

        match resp {
            Response::Exec { stdout, exit_code, timed_out, .. } => {
                assert!(!timed_out, "script timed out");
                assert_eq!(exit_code, Some(0), "non-zero exit");
                // Both lines survive, and the nested single-quotes are intact.
                assert!(stdout.contains("hello 'world'"), "quotes mangled: {stdout:?}");
                assert!(stdout.contains("line2"), "second line missing: {stdout:?}");
            }
            other => panic!("expected Exec response, got {other:?}"),
        }
    }
}

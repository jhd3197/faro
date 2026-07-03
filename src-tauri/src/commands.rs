use crate::agent::ChatRequest;
use crate::profiles::ConnectionProfile;
use crate::remotefs::{Capabilities, DirEntry, RemoteFs};
use crate::session::{HostDecision, JobHandle, Session, SshSession};
use crate::transfer::{OverwritePolicy, Transfer, TransferStatus};
use crate::AppState;
use base64::Engine as _;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Manager, State};
use uuid::Uuid;

pub const LOCAL_SESSION: &str = "local";

/// Convert any error to a string for crossing the IPC boundary. The frontend
/// surfaces these directly to the user, so the Display message must be useful.
fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

// ---------- Profiles ----------

#[tauri::command]
pub async fn list_profiles(
    state: State<'_, AppState>,
) -> Result<Vec<ConnectionProfile>, String> {
    state.profiles.list().await.map_err(err)
}

#[tauri::command]
pub async fn save_profile(
    profile: ConnectionProfile,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.profiles.upsert(profile).await.map_err(err)
}

#[tauri::command]
pub async fn delete_profile(
    id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.profiles.delete(&id).await.map_err(err)
}

// ---------- Sessions ----------

#[tauri::command]
pub async fn connect(
    profile_id: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let profile = state
        .profiles
        .get(&profile_id)
        .await
        .map_err(err)?
        .ok_or_else(|| format!("profile {profile_id} not found"))?;
    let session_id = state.sessions.connect(profile, app).await.map_err(err)?;
    // Re-apply a previously-granted Agent Bridge access for this profile (session
    // ids are per-connect, so the bridge tracks the persistent grant by profile).
    state
        .bridge
        .on_session_connected(&session_id, &profile_id)
        .await;
    Ok(session_id)
}

#[tauri::command]
pub async fn disconnect(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.sessions.disconnect(&session_id).await.map_err(err)
}

// ---------- Faro Agent (Faro-to-Faro remote control) ----------

/// Discover `faro-agentd` daemons on the local network over mDNS. Best-effort;
/// returns an empty list on a network without multicast rather than erroring.
#[tauri::command]
pub async fn discover_agents() -> Result<Vec<crate::session::agent::discovery::Discovered>, String> {
    Ok(crate::session::agent::discovery::browse(Duration::from_millis(1500)).await)
}

/// This controller's own agent public key (base64). Shown in the pairing UI so a
/// user can eyeball which controller a daemon just pinned.
#[tauri::command]
pub async fn agent_public_key() -> Result<String, String> {
    crate::session::agent::controller_public_key().map_err(err)
}

/// Result of pairing, surfaced to the UI to confirm the machine.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPairResult {
    pub fingerprint: String,
    pub hostname: String,
    pub os: String,
}

/// Pair with a `faro-agentd` at `host:port` using a one-time `code`, then persist
/// the daemon's pinned key into the profile so subsequent connects are keyed. The
/// profile must already exist (created by the connection form) and use protocol
/// `"faro-agent"`.
#[tauri::command]
pub async fn pair_agent(
    profile_id: String,
    code: String,
    state: State<'_, AppState>,
) -> Result<AgentPairResult, String> {
    let mut profile = state
        .profiles
        .get(&profile_id)
        .await
        .map_err(err)?
        .ok_or_else(|| format!("profile {profile_id} not found"))?;
    let outcome = crate::session::agent_pair(&profile.host, profile.port, &code)
        .await
        .map_err(err)?;
    profile.agent_key = Some(outcome.server_key);
    state.profiles.upsert(profile).await.map_err(err)?;
    Ok(AgentPairResult {
        fingerprint: outcome.fingerprint,
        hostname: outcome.system_info.hostname,
        os: outcome.system_info.os,
    })
}

/// List the in-flight tracked commands (agent/bridge `exec`s, tails) running on a
/// session, so the UI can show a Jobs panel with a Kill button. Returns an empty
/// list for a non-SSH or unknown session rather than erroring.
#[tauri::command]
pub async fn list_agent_jobs(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<JobHandle>, String> {
    match state.sessions.get_ssh(&session_id).await {
        Some(ssh) => Ok(ssh.list_jobs().await),
        None => Ok(Vec::new()),
    }
}

/// Terminate a running tracked job (by op_id) on a session — the user-facing
/// "stop this" for a long background command. Returns `true` if a matching live
/// job was found and signalled.
#[tauri::command]
pub async fn kill_agent_job(
    session_id: String,
    op_id: String,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    let Some(ssh) = state.sessions.get_ssh(&session_id).await else {
        return Err(format!("session {session_id} is not a live SSH connection"));
    };
    ssh.kill_job(&op_id).await.map_err(err)
}

#[tauri::command]
pub async fn respond_to_host_prompt(
    request_id: String,
    decision: HostDecision,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .sessions
        .prompts
        .resolve(&request_id, decision)
        .await
        .map_err(err)
}

/// Answer a keyboard-interactive auth prompt (e.g. a forced password change for
/// an expired/temp password). `responses` is one string per prompt, or `null`
/// when the user cancelled the dialog (which aborts the connection).
#[tauri::command]
pub async fn respond_to_auth_prompt(
    request_id: String,
    responses: Option<Vec<String>>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .sessions
        .auth_prompts
        .resolve(&request_id, responses)
        .await
        .map_err(err)
}

// ---------- File system ----------

#[tauri::command]
pub async fn list_directory(
    session_id: String,
    path: String,
    state: State<'_, AppState>,
) -> Result<Vec<DirEntry>, String> {
    let fs = fs_for(&session_id, &state).await?;
    fs.list_dir(&path).await.map_err(err)
}

#[tauri::command]
pub async fn capabilities(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<Capabilities, String> {
    let fs = fs_for(&session_id, &state).await?;
    Ok(fs.capabilities())
}

/// Largest local file we'll slurp for an image thumbnail. Beyond this the grid
/// falls back to a type icon rather than reading a huge file into memory.
const MAX_PREVIEW_BYTES: u64 = 4 * 1024 * 1024; // 4 MiB

/// Read a small **local** file and return it base64-encoded, so the grid view
/// can render a real image preview instead of a generic icon. Local-only and
/// size-capped: remote previews would need a per-protocol read primitive, which
/// the `RemoteFs` trait doesn't expose yet — the frontend treats an error here
/// as "no preview" and shows the icon.
#[tauri::command]
pub async fn read_file_preview(
    session_id: String,
    path: String,
) -> Result<String, String> {
    if session_id != LOCAL_SESSION {
        return Err("preview is only supported for local files".into());
    }
    let data = tokio::task::spawn_blocking(move || -> std::io::Result<Vec<u8>> {
        let meta = std::fs::metadata(&path)?;
        if meta.len() > MAX_PREVIEW_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "file too large for preview",
            ));
        }
        std::fs::read(&path)
    })
    .await
    .map_err(err)?
    .map_err(err)?;
    Ok(base64::engine::general_purpose::STANDARD.encode(data))
}

// ---------- Terminal ----------

#[tauri::command]
pub async fn open_terminal(
    session_id: String,
    cols: u32,
    rows: u32,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    if session_id == LOCAL_SESSION {
        return Err("Local terminal is not supported".into());
    }
    let ssh = state
        .sessions
        .get_ssh(&session_id)
        .await
        .ok_or_else(|| format!("FTP sessions have no shell"))?;
    state
        .ptys
        .open(&ssh, cols, rows, app)
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn terminal_write(
    terminal_id: String,
    data: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.ptys.write(&terminal_id, data).await.map_err(err)
}

#[tauri::command]
pub async fn terminal_resize(
    terminal_id: String,
    cols: u32,
    rows: u32,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .ptys
        .resize(&terminal_id, cols, rows)
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn close_terminal(
    terminal_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.ptys.close(&terminal_id).await.map_err(err)
}

// ---------- Transfers ----------

#[tauri::command]
pub async fn start_download(
    session_id: String,
    remote_path: String,
    local_dir: String,
    overwrite_policy: Option<OverwritePolicy>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let session = state
        .sessions
        .get(&session_id)
        .await
        .ok_or_else(|| format!("session {session_id} not found"))?;
    state
        .transfers
        .start_download(
            session,
            remote_path,
            local_dir,
            overwrite_policy.unwrap_or_default(),
            app,
        )
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn start_upload(
    session_id: String,
    local_path: String,
    remote_dir: String,
    overwrite_policy: Option<OverwritePolicy>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let session = state
        .sessions
        .get(&session_id)
        .await
        .ok_or_else(|| format!("session {session_id} not found"))?;
    state
        .transfers
        .start_upload(
            session,
            local_path,
            remote_dir,
            overwrite_policy.unwrap_or_default(),
            app,
        )
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn cancel_transfer(
    transfer_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.transfers.cancel(&transfer_id).await.map_err(err)
}

#[tauri::command]
pub async fn list_transfers(
    state: State<'_, AppState>,
) -> Result<Vec<Transfer>, String> {
    Ok(state.transfers.list().await)
}

#[tauri::command]
pub async fn start_directory_download(
    session_id: String,
    remote_dir: String,
    local_dir: String,
    overwrite_policy: Option<OverwritePolicy>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<String>, String> {
    let session = state
        .sessions
        .get(&session_id)
        .await
        .ok_or_else(|| format!("session {session_id} not found"))?;
    state
        .transfers
        .start_directory_download(
            session,
            remote_dir,
            local_dir,
            overwrite_policy.unwrap_or_default(),
            app,
        )
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn start_directory_upload(
    session_id: String,
    local_dir: String,
    remote_dir: String,
    overwrite_policy: Option<OverwritePolicy>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<String>, String> {
    let session = state
        .sessions
        .get(&session_id)
        .await
        .ok_or_else(|| format!("session {session_id} not found"))?;
    state
        .transfers
        .start_directory_upload(
            session,
            local_dir,
            remote_dir,
            overwrite_policy.unwrap_or_default(),
            app,
        )
        .await
        .map_err(err)
}

// ---------- File ops (rename, delete, mkdir, chmod) ----------
//
// Polymorphic dispatch on session type. The UI doesn't need to know whether
// it's talking to SFTP or FTP — RemoteFs hides that.

async fn fs_for(
    session_id: &str,
    state: &AppState,
) -> Result<Box<dyn RemoteFs>, String> {
    if session_id == LOCAL_SESSION {
        return Ok(Box::new(crate::remotefs::local::LocalFs));
    }
    let session = state
        .sessions
        .get(session_id)
        .await
        .ok_or_else(|| format!("session {session_id} not found"))?;
    Ok(fs_for_session(&session))
}

pub fn fs_for_session(session: &Arc<Session>) -> Box<dyn RemoteFs> {
    match &**session {
        Session::Ssh(ssh) => Box::new(crate::remotefs::sftp::SftpFs::new(ssh.clone())),
        Session::Ftp(ftp) => Box::new(crate::remotefs::ftp::FtpFs::new(ftp.clone())),
        Session::Object(obj) => {
            Box::new(crate::remotefs::object::ObjectFs::new(obj.clone()))
        }
        Session::Agent(agent) => Box::new(crate::remotefs::agent::AgentFs::new(agent.clone())),
    }
}

#[tauri::command]
pub async fn rename_path(
    session_id: String,
    from: String,
    to: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let fs = fs_for(&session_id, &state).await?;
    fs.rename(&from, &to).await.map_err(err)
}

#[tauri::command]
pub async fn delete_path(
    session_id: String,
    path: String,
    recursive: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let fs = fs_for(&session_id, &state).await?;
    fs.delete(&path, recursive).await.map_err(err)
}

#[tauri::command]
pub async fn create_directory(
    session_id: String,
    path: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let fs = fs_for(&session_id, &state).await?;
    fs.create_dir(&path).await.map_err(err)
}

#[tauri::command]
pub async fn chmod_path(
    session_id: String,
    path: String,
    mode: u32,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let fs = fs_for(&session_id, &state).await?;
    fs.chmod(&path, mode).await.map_err(err)
}

// ---------- Duplicate ----------

/// POSIX single-quote a string so it survives spaces / shell metacharacters when
/// interpolated into a remote command. `'` is closed, escaped, and reopened.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Split a POSIX path into (parent, last-component). "/foo" → ("", "foo"),
/// "/a/b" → ("/a", "b"), "rel" → (".", "rel").
fn split_remote(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(0) => ("", &path[1..]),
        Some(i) => (&path[..i], &path[i + 1..]),
        None => (".", path),
    }
}

/// "foo.txt" + 1 → "foo copy.txt"; + 2 → "foo copy 2.txt". The extension (a dot
/// that isn't the leading char) is preserved; dotfiles / extension-less names
/// just get the suffix appended.
fn copy_name(name: &str, n: usize) -> String {
    let suffix = if n <= 1 {
        " copy".to_string()
    } else {
        format!(" copy {n}")
    };
    match name.rfind('.') {
        Some(i) if i > 0 => format!("{}{}{}", &name[..i], suffix, &name[i..]),
        _ => format!("{name}{suffix}"),
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}

fn duplicate_local(path: &str) -> Result<(), String> {
    let src = Path::new(path);
    if !src.exists() {
        return Err(format!("{path} does not exist"));
    }
    let parent = src
        .parent()
        .ok_or_else(|| "can't duplicate this path".to_string())?;
    let name = src
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| "invalid file name".to_string())?;

    let mut n = 1;
    let mut dst = parent.join(copy_name(name, n));
    while dst.exists() {
        n += 1;
        if n > 1000 {
            return Err("too many copies already exist".into());
        }
        dst = parent.join(copy_name(name, n));
    }

    if src.is_dir() {
        copy_dir_recursive(src, &dst).map_err(err)?;
    } else {
        std::fs::copy(src, &dst).map_err(err)?;
    }
    Ok(())
}

async fn duplicate_ssh(ssh: &Arc<SshSession>, path: &str) -> Result<(), String> {
    let path = path.trim_end_matches('/');
    let (parent, name) = split_remote(path);
    let mut n = 1;
    let dst = loop {
        let cand = format!("{parent}/{}", copy_name(name, n));
        let probe = ssh
            .exec(&format!("test -e {}", sh_quote(&cand)))
            .await
            .map_err(err)?;
        if probe.exit_code != Some(0) {
            break cand; // doesn't exist yet
        }
        n += 1;
        if n > 1000 {
            return Err("too many copies already exist".into());
        }
    };
    let out = ssh
        .exec(&format!(
            "cp -a -- {} {}",
            sh_quote(path),
            sh_quote(&dst)
        ))
        .await
        .map_err(err)?;
    if out.exit_code != Some(0) {
        let msg = out.stderr.trim();
        return Err(if msg.is_empty() {
            "copy failed".into()
        } else {
            format!("copy failed: {msg}")
        });
    }
    Ok(())
}

/// Copy a file/folder next to the original under a free "… copy" name. Local
/// uses a filesystem copy; SSH uses `cp -a` server-side. FTP/object stores have
/// no copy primitive, so they're refused.
#[tauri::command]
pub async fn duplicate_path(
    session_id: String,
    path: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if session_id == LOCAL_SESSION {
        return duplicate_local(&path);
    }
    let session = state
        .sessions
        .get(&session_id)
        .await
        .ok_or_else(|| format!("session {session_id} not found"))?;
    match &*session {
        Session::Ssh(ssh) => duplicate_ssh(ssh, &path).await,
        _ => Err("Duplicate isn't supported on this connection yet".into()),
    }
}

// ---------- Server-side archive + download ----------

/// Archive a remote directory on the server (a single `tar`/`zip` command) and
/// download the resulting file — far cheaper than walking the tree and pulling
/// each file. The temp archive lives in a unique `/tmp` dir so the downloaded
/// file keeps a friendly name (`<folder>.tar.gz`); it's removed once the
/// transfer settles. SSH/SFTP only.
#[tauri::command]
pub async fn start_archive_download(
    session_id: String,
    remote_path: String,
    format: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let ssh = state
        .sessions
        .get_ssh(&session_id)
        .await
        .ok_or_else(|| "Archiving requires an SSH/SFTP connection".to_string())?;
    let session = state
        .sessions
        .get(&session_id)
        .await
        .ok_or_else(|| format!("session {session_id} not found"))?;

    let folder = remote_path.trim_end_matches('/');
    if folder.is_empty() {
        return Err("Can't archive the filesystem root".into());
    }
    let (parent, base) = split_remote(folder);
    let parent = if parent.is_empty() { "/" } else { parent };

    let is_zip = format == "zip";
    let ext = if is_zip { "zip" } else { "tar.gz" };
    let tmp_dir = format!("/tmp/faro-archive-{}", Uuid::new_v4());
    let tmp = format!("{tmp_dir}/{base}.{ext}");

    // `cd parent` (zip) / `-C parent` (tar) so paths inside the archive are
    // relative to the folder, not absolute.
    let build = if is_zip {
        format!(
            "mkdir -p {dir} && cd {p} && zip -r -q {out} {b}",
            dir = sh_quote(&tmp_dir),
            p = sh_quote(parent),
            out = sh_quote(&tmp),
            b = sh_quote(&format!("./{base}")),
        )
    } else {
        format!(
            "mkdir -p {dir} && tar -czf {out} -C {p} -- {b}",
            dir = sh_quote(&tmp_dir),
            out = sh_quote(&tmp),
            p = sh_quote(parent),
            b = sh_quote(base),
        )
    };

    let out = ssh.exec(&build).await.map_err(err)?;
    if out.exit_code != Some(0) {
        let _ = ssh.exec(&format!("rm -rf {}", sh_quote(&tmp_dir))).await;
        let stderr = out.stderr.trim();
        let hint = if is_zip
            && (stderr.contains("not found") || stderr.contains("command not found"))
        {
            "  (the `zip` command may not be installed on the server — try .tar.gz)"
        } else {
            ""
        };
        return Err(format!(
            "Archive failed: {}{}",
            if stderr.is_empty() {
                "non-zero exit"
            } else {
                stderr
            },
            hint
        ));
    }

    let dir = app
        .path()
        .download_dir()
        .or_else(|_| app.path().app_data_dir())
        .map_err(err)?;
    std::fs::create_dir_all(&dir).map_err(err)?;
    let local_dir = dir.to_string_lossy().into_owned();

    let id = state
        .transfers
        .start_download(session, tmp.clone(), local_dir, OverwritePolicy::Rename, app)
        .await
        .map_err(err)?;

    // Poll the transfer; once it settles, delete the server-side temp dir.
    let transfers = Arc::clone(&state.transfers);
    let ssh_cleanup = Arc::clone(&ssh);
    let tmp_dir_cleanup = tmp_dir.clone();
    let id_cleanup = id.clone();
    tokio::spawn(async move {
        for _ in 0..14_400u32 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            match transfers.snapshot(&id_cleanup).await {
                Some(t) => {
                    if matches!(
                        t.status,
                        TransferStatus::Done
                            | TransferStatus::Error
                            | TransferStatus::Skipped
                            | TransferStatus::Canceled
                    ) {
                        break;
                    }
                }
                None => break,
            }
        }
        let _ = ssh_cleanup
            .exec(&format!("rm -rf {}", sh_quote(&tmp_dir_cleanup)))
            .await;
    });

    Ok(id)
}

// ---------- Importers (OpenSSH config, FileZilla, PuTTY) ----------

use crate::importers::{self, ProfilePreview};

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImporterPaths {
    pub openssh: Option<String>,
    pub filezilla: Option<String>,
    pub putty: Option<String>,
}

#[tauri::command]
pub fn importer_default_paths() -> ImporterPaths {
    ImporterPaths {
        openssh: importers::openssh::default_path().map(|p| p.display().to_string()),
        filezilla: importers::filezilla::default_path().map(|p| p.display().to_string()),
        putty: importers::putty::default_path().map(|p| p.display().to_string()),
    }
}

#[tauri::command]
pub fn import_openssh(path: Option<String>) -> Result<Vec<ProfilePreview>, String> {
    let path = path
        .map(std::path::PathBuf::from)
        .or_else(importers::openssh::default_path)
        .ok_or_else(|| "could not determine ~/.ssh/config location".to_string())?;
    importers::openssh::parse_file(&path).map_err(err)
}

#[tauri::command]
pub fn import_filezilla(path: Option<String>) -> Result<Vec<ProfilePreview>, String> {
    let path = path
        .map(std::path::PathBuf::from)
        .or_else(importers::filezilla::default_path)
        .ok_or_else(|| "could not determine FileZilla sitemanager.xml location".to_string())?;
    importers::filezilla::parse_file(&path).map_err(err)
}

#[tauri::command]
pub fn import_putty() -> Result<Vec<ProfilePreview>, String> {
    importers::putty::parse_default().map_err(err)
}

#[tauri::command]
pub async fn save_imported_profiles(
    previews: Vec<ProfilePreview>,
    state: State<'_, AppState>,
) -> Result<usize, String> {
    let n = previews.len();
    for preview in previews {
        state
            .profiles
            .upsert(preview.into_profile())
            .await
            .map_err(err)?;
    }
    Ok(n)
}

// ---------- Sync ----------

use crate::sync::{self, SyncDirection, SyncPlan, SyncStrategy};

#[tauri::command]
pub async fn sync_plan(
    session_id: String,
    local_path: String,
    remote_path: String,
    direction: SyncDirection,
    strategy: SyncStrategy,
    state: State<'_, AppState>,
) -> Result<SyncPlan, String> {
    let local_fs: Box<dyn RemoteFs> = Box::new(crate::remotefs::local::LocalFs);
    let remote_fs = fs_for(&session_id, &state).await?;
    sync::plan(
        local_fs.as_ref(),
        remote_fs.as_ref(),
        &local_path,
        &remote_path,
        direction,
        strategy,
    )
    .await
    .map_err(err)
}

#[tauri::command]
pub async fn sync_execute(
    session_id: String,
    plan: SyncPlan,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<String>, String> {
    let session = state
        .sessions
        .get(&session_id)
        .await
        .ok_or_else(|| format!("session {session_id} not found"))?;
    let local_fs: Box<dyn RemoteFs> = Box::new(crate::remotefs::local::LocalFs);
    let remote_fs = fs_for_session(&session);

    let mut ids = Vec::new();
    let policy = crate::transfer::OverwritePolicy::Overwrite;

    for copy in plan.copies {
        let dest_parent = parent_of(&copy.destination_path);
        let id = match plan.direction {
            SyncDirection::LocalToRemote => {
                state
                    .transfers
                    .start_upload(
                        session.clone(),
                        copy.source_path,
                        dest_parent,
                        policy,
                        app.clone(),
                    )
                    .await
                    .map_err(err)?
            }
            SyncDirection::RemoteToLocal => {
                state
                    .transfers
                    .start_download(
                        session.clone(),
                        copy.source_path,
                        dest_parent,
                        policy,
                        app.clone(),
                    )
                    .await
                    .map_err(err)?
            }
        };
        ids.push(id);
    }

    // Apply Mirror deletes after queueing transfers. We don't gate on
    // transfer completion — the user already confirmed the plan — but we
    // do execute deletes serially in this call so the function only
    // returns once the destination is in its final shape.
    for d in plan.deletes {
        let fs: &dyn RemoteFs = match plan.direction {
            SyncDirection::LocalToRemote => remote_fs.as_ref(),
            SyncDirection::RemoteToLocal => local_fs.as_ref(),
        };
        let _ = fs.delete(&d.path, false).await; // best-effort
    }

    Ok(ids)
}

fn parent_of(p: &str) -> String {
    let last_slash = p.rfind(|c: char| c == '/' || c == '\\');
    match last_slash {
        Some(0) => "/".to_string(),
        Some(i) => p[..i].to_string(),
        None => ".".to_string(),
    }
}

// ---------- Edit-in-place ----------

#[tauri::command]
pub async fn start_edit(
    session_id: String,
    remote_path: String,
    editor: Option<String>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<crate::editor::EditStartedEvent, String> {
    let session = state
        .sessions
        .get(&session_id)
        .await
        .ok_or_else(|| format!("session {session_id} not found"))?;
    state
        .editors
        .start(session, session_id, remote_path, editor, app)
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn stop_edit(
    edit_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.editors.stop(&edit_id).await.map_err(err)
}

// ---------- Agent Bridge ----------

use crate::bridge::{ActivityEntry, ApprovalDecision, ApprovalPolicy, BridgeStatus, SavedCommand};

#[tauri::command]
pub async fn bridge_start(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<BridgeStatus, String> {
    state.bridge.start(app).await.map_err(err)
}

#[tauri::command]
pub async fn bridge_stop(state: State<'_, AppState>) -> Result<BridgeStatus, String> {
    state.bridge.stop().await;
    Ok(state.bridge.status().await)
}

/// The master on/off switch (persisted, default off). On => start + publish the
/// discovery file + auto-start next launch; off => stop + remove the token/file.
#[tauri::command]
pub async fn bridge_set_enabled(
    enabled: bool,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<BridgeStatus, String> {
    state.bridge.set_enabled(app, enabled).await.map_err(err)
}

#[tauri::command]
pub async fn bridge_status(state: State<'_, AppState>) -> Result<BridgeStatus, String> {
    Ok(state.bridge.status().await)
}

#[tauri::command]
pub async fn bridge_set_session_access(
    session_id: String,
    enabled: bool,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<BridgeStatus, String> {
    state.bridge.set_access(&app, &session_id, enabled).await;
    Ok(state.bridge.status().await)
}

#[tauri::command]
pub async fn bridge_set_policy(
    policy: ApprovalPolicy,
    state: State<'_, AppState>,
) -> Result<BridgeStatus, String> {
    state.bridge.set_policy(policy).await;
    Ok(state.bridge.status().await)
}

#[tauri::command]
pub async fn respond_to_bridge_approval(
    request_id: String,
    decision: ApprovalDecision,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .bridge
        .resolve_approval(&request_id, decision)
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn bridge_activity(
    state: State<'_, AppState>,
) -> Result<Vec<ActivityEntry>, String> {
    Ok(state.bridge.recent_activity().await)
}

#[tauri::command]
pub async fn bridge_clear_activity(state: State<'_, AppState>) -> Result<(), String> {
    state.bridge.clear_activity().await;
    Ok(())
}

// ---------- Saved commands (pre-approved; local-UI managed only) ----------

#[tauri::command]
pub async fn bridge_list_commands(
    state: State<'_, AppState>,
) -> Result<Vec<SavedCommand>, String> {
    Ok(state.bridge.list_commands().await)
}

#[tauri::command]
pub async fn bridge_save_command(
    command: SavedCommand,
    state: State<'_, AppState>,
) -> Result<Vec<SavedCommand>, String> {
    Ok(state.bridge.upsert_command(command).await)
}

#[tauri::command]
pub async fn bridge_delete_command(
    id: String,
    state: State<'_, AppState>,
) -> Result<Vec<SavedCommand>, String> {
    Ok(state.bridge.delete_command(&id).await)
}

// ---------- Agent console export ----------

/// Write the Agent console's text to the user's Downloads folder (falling back
/// to the app data dir) and return the saved path. Kept plugin-free to match the
/// rest of the app — a direct write, not the dialog/fs plugins. `name` is the
/// caller-suggested filename; path separators are stripped and a name collision
/// gets a " (n)" suffix so an export never clobbers an existing file.
#[tauri::command]
pub async fn export_agent_log(
    content: String,
    name: String,
    app: AppHandle,
) -> Result<String, String> {
    let dir = app
        .path()
        .download_dir()
        .or_else(|_| app.path().app_data_dir())
        .map_err(err)?;
    std::fs::create_dir_all(&dir).map_err(err)?;

    let safe: String = name.chars().filter(|c| !matches!(c, '/' | '\\')).collect();
    let safe = if safe.trim().is_empty() {
        "faro-agent-console.txt".to_string()
    } else {
        safe
    };
    let (stem, ext) = match safe.rsplit_once('.') {
        Some((s, e)) => (s.to_string(), format!(".{e}")),
        None => (safe.clone(), String::new()),
    };
    let mut path = dir.join(&safe);
    let mut n = 1;
    while path.exists() {
        path = dir.join(format!("{stem} ({n}){ext}"));
        n += 1;
    }
    std::fs::write(&path, content).map_err(err)?;
    Ok(path.to_string_lossy().into_owned())
}

// ---------- Agent Bridge ----------

/// Notify the bridge which session is currently focused in the UI. The bridge
/// exposes this in `faro_context` so agents know what the user is looking at.
#[tauri::command]
pub async fn bridge_set_active_session(
    session_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.bridge.set_active_session(session_id).await;
    Ok(())
}

/// Send a message to the built-in Agent chat. The backend calls Anthropic's
/// API with the Faro bridge tools and returns the assistant's final response.
#[tauri::command]
pub async fn agent_chat_cmd(
    req: ChatRequest,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<crate::agent::ChatResponse, String> {
    crate::agent::agent_chat(&app, &state.bridge, req)
        .await
        .map_err(|e| e.to_string())
}

/// Register Faro's MCP server with Claude Code so the user doesn't have to
/// copy/paste the `claude mcp add` command. Best-effort: reports success or
/// the error text so the UI can guide the user.
#[tauri::command]
pub async fn bridge_register_mcp(url: String, token: String) -> Result<String, String> {
    // Try the most likely binary names across platforms.
    let candidates: Vec<&str> = if cfg!(windows) {
        vec!["claude.exe", "claude"]
    } else {
        vec!["claude"]
    };

    let args = [
        "mcp",
        "add",
        "--transport",
        "http",
        "faro",
        &url,
        "--header",
        &format!("Authorization: Bearer {token}"),
    ];

    let mut last_err = String::new();
    for bin in candidates {
        let output = std::process::Command::new(bin)
            .args(&args)
            .output()
            .map_err(|e| format!("couldn't run {bin}: {e}"))?;
        if output.status.success() {
            return Ok(format!(
                "Faro MCP server registered as 'faro'. Claude Code can now use it."
            ));
        }
        last_err = format!(
            "{} exited with {}: {}",
            bin,
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Err(format!(
        "Couldn't register the MCP server automatically. Make sure Claude Code is installed and on your PATH. {last_err}"
    ))
}

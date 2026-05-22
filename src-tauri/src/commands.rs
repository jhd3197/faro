use crate::profiles::ConnectionProfile;
use crate::remotefs::{Capabilities, DirEntry, RemoteFs};
use crate::session::{HostDecision, Session};
use crate::transfer::{OverwritePolicy, Transfer};
use crate::AppState;
use std::sync::Arc;
use tauri::{AppHandle, State};

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
    state.sessions.connect(profile, app).await.map_err(err)
}

#[tauri::command]
pub async fn disconnect(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.sessions.disconnect(&session_id).await.map_err(err)
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

fn fs_for_session(session: &Arc<Session>) -> Box<dyn RemoteFs> {
    match &**session {
        Session::Ssh(ssh) => Box::new(crate::remotefs::sftp::SftpFs::new(ssh.clone())),
        Session::Ftp(ftp) => Box::new(crate::remotefs::ftp::FtpFs::new(ftp.clone())),
        Session::Object(obj) => {
            Box::new(crate::remotefs::object::ObjectFs::new(obj.clone()))
        }
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
        .start(session, session_id, remote_path, app)
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

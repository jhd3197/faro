use std::sync::Arc;
use tauri::Manager;

// The CLI crate (faro-cli/src/main.rs) imports from these via the `faro_lib`
// crate, so they need to be `pub` rather than `mod`. None of them expose
// secrets directly — credentials live in profiles::ConnectionProfile, which
// the CLI deliberately redacts in `profiles show`.
pub mod bridge;
pub mod commands;
pub mod agent;
mod agent_host;
pub mod db;
mod deeplink;
mod diskscan;
mod editor;
mod foldersync;
pub mod importers;
mod known_hosts;
pub mod oauth;
pub mod profiles;
pub mod remotefs;
pub mod scan;
pub mod session;
pub mod sync;
mod terminal;
mod transfer;

pub struct AppState {
    pub sessions: Arc<session::SessionManager>,
    pub ptys: Arc<terminal::PtyManager>,
    pub profiles: Arc<profiles::ProfileStore>,
    pub transfers: Arc<transfer::TransferManager>,
    pub editors: Arc<editor::EditManager>,
    pub bridge: Arc<bridge::BridgeState>,
    pub agent_host: Arc<agent_host::AgentHost>,
    pub foldersync: Arc<foldersync::FolderSync>,
    /// Running disk-usage scans (Plan 4). Ephemeral — not persisted.
    pub diskscan: Arc<diskscan::ScanManager>,
    /// Shared `faro.db` — the per-connection index (sync_state today; scan/search
    /// caches later). See `db.rs`.
    pub db: Arc<db::Db>,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "faro_lib=info,warn".into()),
        )
        .init();

    tauri::Builder::default()
        // single-instance MUST be the first plugin: on Windows/Linux a
        // `faro://` link launches a second process, and this forwards its argv
        // (which carries the URL) to the running instance and focuses it,
        // instead of opening a duplicate window. The `deep-link` feature makes
        // the forwarded URL fire the same on_open_url handler below.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            use tauri::Manager;
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.set_focus();
            }
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let handle = app.handle().clone();
            let profile_store = Arc::new(
                profiles::ProfileStore::load_or_create(&handle)
                    .expect("failed to initialise profile store"),
            );
            let db = {
                let dir = handle
                    .path()
                    .app_data_dir()
                    .expect("resolving app_data_dir for faro.db");
                std::fs::create_dir_all(&dir).ok();
                Arc::new(db::Db::open(&dir.join("faro.db")).expect("failed to open faro.db"))
            };
            let state = AppState {
                sessions: Arc::new(session::SessionManager::new()),
                ptys: Arc::new(terminal::PtyManager::new()),
                profiles: profile_store,
                transfers: Arc::new(transfer::TransferManager::new()),
                editors: Arc::new(editor::EditManager::new()),
                bridge: Arc::new(
                    bridge::BridgeState::load_or_create(&handle).unwrap_or_default(),
                ),
                agent_host: Arc::new(
                    agent_host::AgentHost::load(&handle)
                        .expect("failed to initialise agent host settings"),
                ),
                foldersync: Arc::new(
                    foldersync::FolderSync::load(&handle)
                        .expect("failed to initialise folder sync settings"),
                ),
                diskscan: Arc::new(diskscan::ScanManager::new()),
                db,
            };
            app.manage(state);

            // Bring the Agent Bridge back up if the user left its master switch
            // on, so the `faro-cli agent …` path keeps working across restarts.
            // Spawned off the async runtime so the sync setup() returns at once.
            let bridge = app.state::<AppState>().bridge.clone();
            let bridge_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                bridge.auto_start_if_enabled(bridge_handle).await;
            });

            // Likewise the Remote-control host (this machine as a Faro Agent).
            let host = app.state::<AppState>().agent_host.clone();
            let host_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                host.auto_start_if_enabled(host_handle).await;
            });

            // Restart any folder-sync pairs the user left enabled.
            let foldersync = app.state::<AppState>().foldersync.clone();
            let foldersync_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                foldersync.auto_start_if_enabled(foldersync_handle).await;
            });

            // faro:// deep links. `on_open_url` covers the app-already-running
            // case (macOS always; Windows/Linux via single-instance forwarding).
            {
                use tauri_plugin_deep_link::DeepLinkExt;
                let dl_handle = app.handle().clone();
                app.deep_link().on_open_url(move |event| {
                    deeplink::handle_urls(&dl_handle, event.urls().as_slice());
                });
                // Cold start: the OS may have launched us WITH the URL already.
                if let Ok(Some(urls)) = app.deep_link().get_current() {
                    deeplink::handle_urls(&app.handle().clone(), urls.as_slice());
                }
                // On dev/Linux the scheme must be registered at runtime; on a
                // packaged build the installer does it. Best-effort.
                #[cfg(any(windows, target_os = "linux"))]
                {
                    let _ = app.deep_link().register_all();
                }
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_profiles,
            commands::save_profile,
            commands::reorder_profiles,
            commands::delete_profile,
            commands::connect,
            commands::disconnect,
            commands::discover_agents,
            commands::agent_public_key,
            commands::pair_agent,
            agent_host::agent_host_status,
            agent_host::agent_host_set_enabled,
            agent_host::agent_host_open_pairing,
            agent_host::agent_host_close_pairing,
            agent_host::agent_host_set_policy,
            agent_host::agent_host_revoke_peer,
            foldersync::foldersync_list,
            foldersync::foldersync_upsert,
            foldersync::foldersync_remove,
            foldersync::foldersync_set_enabled,
            foldersync::foldersync_sync_now,
            diskscan::diskscan_start,
            diskscan::diskscan_status,
            diskscan::diskscan_tree,
            diskscan::diskscan_cancel,
            diskscan::diskscan_forget,
            commands::list_agent_jobs,
            commands::kill_agent_job,
            commands::respond_to_host_prompt,
            commands::respond_to_auth_prompt,
            commands::importer_default_paths,
            commands::import_openssh,
            commands::import_filezilla,
            commands::import_putty,
            commands::save_imported_profiles,
            commands::sync_plan,
            commands::sync_execute,
            commands::start_edit,
            commands::stop_edit,
            commands::list_directory,
            commands::capabilities,
            commands::read_file_preview,
            commands::open_terminal,
            commands::terminal_write,
            commands::terminal_resize,
            commands::close_terminal,
            commands::start_download,
            commands::start_upload,
            commands::start_directory_download,
            commands::start_directory_upload,
            commands::cancel_transfer,
            commands::list_transfers,
            commands::rename_path,
            commands::delete_path,
            commands::create_directory,
            commands::chmod_path,
            commands::duplicate_path,
            commands::start_archive_download,
            commands::bridge_start,
            commands::bridge_stop,
            commands::bridge_set_enabled,
            commands::bridge_status,
            commands::bridge_set_session_access,
            commands::bridge_set_policy,
            commands::bridge_set_active_session,
            commands::bridge_register_mcp,
            commands::agent_chat_cmd,
            commands::respond_to_bridge_approval,
            commands::bridge_activity,
            commands::bridge_clear_activity,
            commands::bridge_list_commands,
            commands::bridge_save_command,
            commands::bridge_delete_command,
            commands::export_agent_log,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

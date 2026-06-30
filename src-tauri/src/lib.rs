use std::sync::Arc;
use tauri::Manager;

// The CLI crate (faro-cli/src/main.rs) imports from these via the `faro_lib`
// crate, so they need to be `pub` rather than `mod`. None of them expose
// secrets directly — credentials live in profiles::ConnectionProfile, which
// the CLI deliberately redacts in `profiles show`.
pub mod bridge;
mod commands;
pub mod agent;
mod editor;
pub mod importers;
mod known_hosts;
pub mod profiles;
pub mod remotefs;
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
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let handle = app.handle().clone();
            let profile_store = Arc::new(
                profiles::ProfileStore::load_or_create(&handle)
                    .expect("failed to initialise profile store"),
            );
            let state = AppState {
                sessions: Arc::new(session::SessionManager::new()),
                ptys: Arc::new(terminal::PtyManager::new()),
                profiles: profile_store,
                transfers: Arc::new(transfer::TransferManager::new()),
                editors: Arc::new(editor::EditManager::new()),
                bridge: Arc::new(
                    bridge::BridgeState::load_or_create(&handle).unwrap_or_default(),
                ),
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

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_profiles,
            commands::save_profile,
            commands::delete_profile,
            commands::connect,
            commands::disconnect,
            commands::respond_to_host_prompt,
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

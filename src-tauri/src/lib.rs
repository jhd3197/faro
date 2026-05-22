use std::sync::Arc;
use tauri::Manager;

mod commands;
mod profiles;
mod remotefs;
mod session;
mod terminal;
mod transfer;

pub struct AppState {
    pub sessions: Arc<session::SessionManager>,
    pub ptys: Arc<terminal::PtyManager>,
    pub profiles: Arc<profiles::ProfileStore>,
    pub transfers: Arc<transfer::TransferManager>,
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
            };
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_profiles,
            commands::save_profile,
            commands::delete_profile,
            commands::connect,
            commands::disconnect,
            commands::list_directory,
            commands::capabilities,
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

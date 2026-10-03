pub mod api;
pub mod backoff;
pub mod client;
pub mod commands;
pub mod config;
pub mod events;
pub mod monitor;
pub mod poller;
pub mod runtime;
pub mod sse;
pub mod state;

use std::sync::Arc;

use tauri::Manager;

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let dir = app.path().app_config_dir()?;
            let state = commands::AppState::new(dir.join("settings.json"), Arc::new(config::KeyringStore));
            if let Some(settings) = config::load_settings(&state.config_path) {
                let key = state.secrets.get(&settings.base_url).ok().flatten();
                if let Err(e) = commands::restart_monitor(app.handle(), &state, &settings, key) {
                    eprintln!("monitor failed to start: {e}");
                }
            }
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::save_settings,
            commands::test_connection,
            commands::is_saved_url,
            commands::get_snapshot
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

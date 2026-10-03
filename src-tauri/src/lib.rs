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
pub mod window;

use std::sync::Arc;

use tauri::Manager;

pub fn run() {
    tauri::Builder::default()
        // Remembers size/position/maximized across launches. Never resizes by itself; a saved
        // position that no monitor contains is ignored and the window stays centred.
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::SIZE
                        | tauri_plugin_window_state::StateFlags::POSITION
                        | tauri_plugin_window_state::StateFlags::MAXIMIZED,
                )
                .build(),
        )
        .setup(|app| {
            let dir = app.path().app_config_dir()?;
            let state = commands::AppState::new(dir.join("settings.json"), Arc::new(config::KeyringStore));
            let settings = config::load_settings(&state.config_path);
            // Pin, first-launch fit and show; always shows the window, whatever fails.
            window::prepare(
                app.handle(),
                settings.as_ref().is_some_and(|s| s.always_on_top),
                !window::has_saved_state(&dir),
            );
            if let Some(settings) = settings {
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
            commands::get_snapshot,
            commands::get_activity,
            commands::load_model,
            commands::unload_model,
            commands::set_always_on_top
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

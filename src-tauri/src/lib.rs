pub mod api;
pub mod client;
pub mod config;
pub mod events;
pub mod sse;
pub mod state;

pub fn run() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

pub mod api;
pub mod events;
pub mod sse;

pub fn run() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

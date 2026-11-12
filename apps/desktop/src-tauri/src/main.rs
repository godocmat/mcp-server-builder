#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use builder_app::{Reply, SmokeRequest, SmokeStatus};
use tracing_subscriber::EnvFilter;

#[tauri::command]
fn workspace_smoke(request: SmokeRequest) -> Reply<SmokeStatus> {
    builder_app::workspace_smoke(request)
}

fn main() -> tauri::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .json()
        .init();

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![workspace_smoke])
        .run(tauri::generate_context!())
}

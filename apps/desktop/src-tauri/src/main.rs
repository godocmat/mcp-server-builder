#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(all(feature = "ci-webdriver", not(debug_assertions)))]
compile_error!("ci-webdriver requires a debug build");

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

    let builder = tauri::Builder::default();
    #[cfg(all(debug_assertions, feature = "ci-webdriver"))]
    let builder = if let Some(port) = std::env::var("TAURI_WEBDRIVER_PORT")
        .ok()
        .and_then(|port| port.parse::<std::num::NonZeroU16>().ok())
    {
        builder.plugin(tauri_plugin_wdio_webdriver::init_with_port(port.get()))
    } else {
        builder
    };

    builder
        .invoke_handler(tauri::generate_handler![workspace_smoke])
        .run(tauri::generate_context!())
}

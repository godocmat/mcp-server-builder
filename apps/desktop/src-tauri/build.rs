#[path = "src/commands.rs"]
mod commands;

fn main() {
    let manifest = tauri_build::AppManifest::new().commands(commands::MAIN_COMMANDS);
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(manifest))
        .expect("failed to build desktop context");
}

fn main() {
    let manifest = tauri_build::AppManifest::new().commands(&["workspace_smoke"]);
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(manifest))
        .expect("failed to build desktop context");
}

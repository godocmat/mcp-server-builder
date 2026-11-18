#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(all(feature = "ci-webdriver", not(debug_assertions)))]
compile_error!("ci-webdriver requires a debug build");

mod export;
mod ipc;
mod paths;
mod preview;
mod project;

use std::sync::Arc;

use builder_app::{ProjectService, Reply, SmokeRequest, SmokeStatus};
use tauri::{Manager, Runtime, WindowEvent};
use tracing_subscriber::{
    EnvFilter, Layer, filter::filter_fn, layer::SubscriberExt, util::SubscriberInitExt,
};

#[tauri::command]
fn workspace_smoke(request: SmokeRequest) -> Reply<SmokeStatus> {
    builder_app::workspace_smoke(request)
}

fn main() -> tauri::Result<()> {
    tracing_subscriber::registry()
        .with(EnvFilter::from_default_env())
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .json()
                .with_filter(filter_fn(builder_app::playground::sdk_diagnostic_filter)),
        )
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

    let desktop = ipc::Desktop::new(ProjectService::for_user());
    ipc::Desktop::expire_handles(&desktop);
    desktop_builder(builder, desktop).run(tauri::generate_context!())
}

fn desktop_builder<R: Runtime>(
    builder: tauri::Builder<R>,
    desktop: Arc<ipc::Desktop>,
) -> tauri::Builder<R> {
    builder
        .plugin(tauri_plugin_dialog::init())
        .manage(desktop)
        .on_window_event(|window, event| {
            if window.label() == "main" && matches!(event, WindowEvent::Destroyed) {
                window.state::<Arc<ipc::Desktop>>().clear();
            }
        })
        .invoke_handler(tauri::generate_handler![
            workspace_smoke,
            paths::select_path,
            project::project_list,
            project::project_open,
            project::project_create,
            project::project_validate,
            project::project_save,
            project::project_recover,
            export::generation_plan,
            export::generation_write,
            preview::policy_review,
            preview::policy_approve,
            preview::playground_start,
            preview::playground_stop,
            preview::playground_discover,
            preview::playground_call,
            preview::playground_trace,
            preview::scenario_run,
            preview::run_poll,
            preview::run_cancel,
        ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use tauri::ipc::{CallbackFn, InvokeBody};
    use tauri::test::{INVOKE_KEY, MockRuntime, get_ipc_response, mock_builder};
    use tauri::webview::InvokeRequest;

    #[test]
    fn project_and_review_commands_cross_only_the_main_local_ipc_boundary() {
        let profile = tempfile::tempdir().unwrap();
        let app = desktop_builder(
            mock_builder(),
            ipc::Desktop::new(ProjectService::with_profile(profile.path().into())),
        )
        .build(tauri::generate_context!())
        .unwrap();
        let main = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        let secondary = tauri::WebviewWindowBuilder::new(&app, "secondary", Default::default())
            .build()
            .unwrap();
        let origin = if cfg!(windows) {
            "http://tauri.localhost/"
        } else {
            "tauri://localhost/"
        };
        let invoke =
            |window: &tauri::WebviewWindow<MockRuntime>, url: &str, cmd: &str, request: Value| {
                get_ipc_response(
                    window,
                    InvokeRequest {
                        cmd: cmd.into(),
                        callback: CallbackFn(0),
                        error: CallbackFn(1),
                        url: url.parse().unwrap(),
                        body: InvokeBody::Json(json!({ "request": request })),
                        headers: Default::default(),
                        invoke_key: INVOKE_KEY.to_string(),
                    },
                )
                .map(|reply| reply.deserialize::<Value>().unwrap())
            };

        let listed = invoke(&main, origin, "project_list", json!({})).unwrap();
        assert_eq!(listed, json!({ "ok": true, "data": [], "diagnostics": [] }));
        assert!(invoke(&secondary, origin, "project_list", json!({})).is_err());
        assert!(invoke(&main, "https://example.invalid/", "project_list", json!({})).is_err());

        let approval = invoke(
            &main,
            origin,
            "policy_approve",
            json!({
                "reviewId": uuid::Uuid::new_v4(),
                "digest": builder_app::ManifestRevision::of(b"unregistered")
            }),
        )
        .unwrap();
        assert_eq!(approval["ok"], false);
        assert_eq!(approval["error"]["code"], "policy.approval_required");
    }
}

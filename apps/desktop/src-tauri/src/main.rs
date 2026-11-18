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

    const LOCAL_ORIGIN: &str = if cfg!(windows) {
        "http://tauri.localhost/"
    } else {
        "tauri://localhost/"
    };

    fn invoke(
        window: &tauri::WebviewWindow<MockRuntime>,
        url: &str,
        command: &str,
        request: Value,
    ) -> Result<Value, Value> {
        get_ipc_response(
            window,
            InvokeRequest {
                cmd: command.into(),
                callback: CallbackFn(0),
                error: CallbackFn(1),
                url: url.parse().unwrap(),
                body: InvokeBody::Json(json!({ "request": request })),
                headers: Default::default(),
                invoke_key: INVOKE_KEY.to_string(),
            },
        )
        .map(|reply| reply.deserialize::<Value>().unwrap())
    }

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
        let listed = invoke(&main, LOCAL_ORIGIN, "project_list", json!({})).unwrap();
        assert_eq!(listed, json!({ "ok": true, "data": [], "diagnostics": [] }));
        assert!(invoke(&secondary, LOCAL_ORIGIN, "project_list", json!({})).is_err());
        assert!(invoke(&main, "https://example.invalid/", "project_list", json!({})).is_err());

        let approval = invoke(
            &main,
            LOCAL_ORIGIN,
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

    #[cfg(windows)]
    #[test]
    #[ignore = "stage the built mcp-builder.exe next to the desktop test executable"]
    fn reviewed_fixture_round_trips_through_desktop_ipc_and_real_stdio() {
        use std::time::{Duration, Instant};

        use builder_app::ProjectLocation;

        const MARKER: &str = "desktop-fixture-redaction-check-b5f4";
        let project = tempfile::tempdir().unwrap();
        let profile = tempfile::tempdir().unwrap();
        let worker = std::env::current_exe()
            .unwrap()
            .with_file_name("mcp-builder.exe");
        assert!(worker.is_file(), "stage the first-party CLI at {worker:?}");

        let mut document: Value = serde_json::from_str(include_str!(
            "../../../../crates/builder-project/tests/fixtures/catalog-v1.json"
        ))
        .unwrap();
        document["scenarios"][0]["fixtures"][0]["body"]["private"] = json!(MARKER);
        std::fs::write(
            project.path().join(builder_project::MANIFEST_NAME),
            serde_json::to_vec(&document).unwrap(),
        )
        .unwrap();
        let projects = ProjectService::with_profile(profile.path().into());
        let opened = tauri::async_runtime::block_on(
            projects.open(ProjectLocation::Root(project.path().into())),
        );
        let (snapshot, _) = ipc::unpack(opened).unwrap();

        let app = desktop_builder(mock_builder(), ipc::Desktop::new(projects))
            .build(tauri::generate_context!())
            .unwrap();
        let main = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        let request = |command, arguments| invoke(&main, LOCAL_ORIGIN, command, arguments).unwrap();
        let read = |command, arguments| {
            let reply = request(command, arguments);
            assert_eq!(reply["ok"], true, "{command}: {reply}");
            assert!(!reply.to_string().contains(MARKER), "{command}");
            reply["data"].clone()
        };
        let poll = |id: &Value| {
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                let progress = read("run_poll", json!({ "id": id, "afterCursor": 0 }));
                if progress["terminal"] == true {
                    return progress["events"][0].clone();
                }
                assert!(Instant::now() < deadline, "operation did not finish");
                std::thread::sleep(Duration::from_millis(20));
            }
        };
        let subject = json!({
            "kind": "launch",
            "registrationId": snapshot.registration_id,
            "expectedRevision": snapshot.revision
        });
        let review = read(
            "policy_review",
            json!({
                "subject": subject,
                "mode": { "kind": "fixture", "scenarioId": "known_item" },
                "access": { "approvedGrantIds": ["catalog_read"] },
                "protocol": "2026-07-28",
                "capturePayloads": true
            }),
        );
        assert_eq!(review["summary"]["approvedGrants"][0]["id"], "catalog_read");
        assert_eq!(
            review["summary"]["arguments"][0],
            builder_app::playground::WORKER_ARG,
        );
        let approved = read(
            "policy_approve",
            json!({
                "reviewId": review["reviewId"],
                "digest": review["digest"]
            }),
        );
        let launch =
            json!({ "subject": subject, "approval": { "approvalId": approved["approvalId"] } });
        let starting = read("playground_start", launch.clone());
        let started = poll(&starting["operationId"]);
        assert_eq!(started["kind"], "started", "{started}");
        assert_eq!(started["discovery"]["protocolRevision"], "2026-07-28");
        let connection = json!({ "connectionId": started["connectionId"] });

        let running = read(
            "scenario_run",
            json!({
                "registrationId": snapshot.registration_id,
                "expectedRevision": snapshot.revision,
                "scenarioId": "known_item",
                "connectionId": started["connectionId"]
            }),
        );
        let completed = poll(&running["runId"]);
        assert_eq!(completed["kind"], "completed", "{completed}");
        assert_eq!(completed["run"]["result"]["outcome"], "passed");
        assert_eq!(completed["run"]["result"]["runId"], running["runId"]);
        assert_eq!(
            completed["run"]["payload"]["structuredContent"]["name"],
            "Sample item"
        );
        let trace = read("playground_trace", connection.clone());
        assert!(
            trace["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry.get("payload").is_some())
        );
        assert_eq!(read("playground_stop", connection.clone())["stopped"], true);
        assert_eq!(
            request("playground_trace", connection)["error"]["code"],
            "playground.connection_unavailable"
        );
        assert_eq!(
            request("playground_start", launch)["error"]["code"],
            "policy.approval_required"
        );
    }
}

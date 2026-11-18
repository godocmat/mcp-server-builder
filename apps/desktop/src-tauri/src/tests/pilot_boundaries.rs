use std::fs;
use std::path::{Path, PathBuf};

use super::*;
use crate::paths::tests::record_fixture_selection;

struct Pilot {
    dir: tempfile::TempDir,
    state: Arc<ipc::Desktop>,
    _app: tauri::App<MockRuntime>,
    main: tauri::WebviewWindow<MockRuntime>,
}

impl Pilot {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("profile");
        let state = ipc::Desktop::new(ProjectService::with_profile(profile));
        let app = desktop_builder(mock_builder(), Arc::clone(&state))
            .build(tauri::generate_context!())
            .unwrap();
        let main = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        Self {
            dir,
            state,
            _app: app,
            main,
        }
    }

    fn folder(&self, name: &str) -> PathBuf {
        let path = self.dir.path().join(name);
        fs::create_dir(&path).unwrap();
        path
    }

    fn selection(&self, path: &Path, purpose: &str) -> Value {
        json!({ "selectionId": record_fixture_selection(&self.state, path, purpose) })
    }

    fn request(&self, command: &str, arguments: Value) -> Value {
        invoke(&self.main, LOCAL_ORIGIN, command, arguments).unwrap()
    }

    fn success(&self, command: &str, arguments: Value) -> Value {
        let reply = self.request(command, arguments);
        assert_eq!(reply["ok"], true, "{command}: {reply}");
        reply["data"].clone()
    }

    fn create(&self, path: &Path) -> Value {
        self.success(
            "project_create",
            json!({ "destination": self.selection(path, "projectDestination"), "name": "Catalog" }),
        )
    }

    #[cfg(windows)]
    fn terminal_event(&self, id: &Value) -> Value {
        use std::time::{Duration, Instant};

        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let progress = self.success("run_poll", json!({ "id": id, "afterCursor": 0 }));
            if progress["terminal"] == true {
                return progress["events"][0].clone();
            }
            assert!(Instant::now() < deadline, "operation did not finish");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

#[test]
fn project_save_requires_current_revision_and_recovery_needs_its_own_folder() {
    let pilot = Pilot::new();
    let dir = pilot.folder("catalog");
    let wrong_purpose = pilot.request(
        "project_create",
        json!({ "destination": pilot.selection(&dir, "projectRoot"), "name": "Catalog" }),
    );
    assert_eq!(wrong_purpose["error"]["code"], "input.invalid");
    let manifest = dir.join(builder_project::MANIFEST_NAME);
    assert!(!manifest.exists());
    let created = pilot.create(&dir);
    let reopened = pilot.success(
        "project_open",
        json!({ "root": pilot.selection(&dir, "projectRoot") }),
    );
    assert_eq!(created["revision"], reopened["revision"]);
    assert_eq!(created["registrationId"], reopened["registrationId"]);

    let mut document = created["document"].clone();
    document["name"] = json!("Catalog draft");
    let stale = pilot.request(
        "project_save",
        json!({
            "registrationId": created["registrationId"],
            "expectedRevision": builder_app::ManifestRevision::of(b"prior revision"),
            "document": document,
        }),
    );
    assert_eq!(stale["error"]["code"], "project.revision_conflict");
    assert_eq!(
        stale["error"]["details"]["currentRevision"],
        created["revision"]
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(&manifest).unwrap()).unwrap()["name"],
        "Catalog"
    );
    assert_eq!(
        pilot.success("project_validate", json!({ "document": document }))["valid"],
        true
    );
    let saved = pilot.success(
        "project_save",
        json!({
            "registrationId": created["registrationId"],
            "expectedRevision": created["revision"],
            "document": document,
        }),
    );
    assert_ne!(saved["revision"], created["revision"]);

    let source = json!({ "kind": "draft", "document": document });
    let missing_destination = pilot.request(
        "project_recover",
        json!({ "registrationId": created["registrationId"], "source": source }),
    );
    assert_eq!(missing_destination["error"]["code"], "input.invalid");
    let copy = pilot.folder("recovered");
    let recovered = pilot.success(
        "project_recover",
        json!({
            "registrationId": created["registrationId"],
            "source": source,
            "destination": pilot.selection(&copy, "recoveryDestination"),
        }),
    );
    assert_ne!(recovered["registrationId"], saved["registrationId"]);
    assert_eq!(recovered["document"]["name"], "Catalog draft");

    let backup_copy = pilot.folder("backup copy");
    let copied = pilot.success(
        "project_recover",
        json!({
            "registrationId": created["registrationId"],
            "source": { "kind": "backup", "revision": created["revision"] },
            "destination": pilot.selection(&backup_copy, "recoveryDestination"),
        }),
    );
    assert_eq!(copied["document"]["name"], "Catalog");
    assert_ne!(
        copied["document"]["projectId"],
        created["document"]["projectId"]
    );
    assert_eq!(
        json!(builder_app::ManifestRevision::of(
            &fs::read(manifest).unwrap()
        )),
        saved["revision"]
    );

    let restored = pilot.success(
        "project_recover",
        json!({
            "root": pilot.selection(&dir, "projectRoot"),
            "expectedRevision": saved["revision"],
            "source": { "kind": "backup", "revision": created["revision"] },
        }),
    );
    assert_eq!(restored["revision"], created["revision"]);
    assert_eq!(restored["document"]["name"], "Catalog");
}

#[test]
fn pending_native_picker_prevents_a_second_picker_operation() {
    let pilot = Pilot::new();
    let _first_picker = pilot.state.picking.try_lock().unwrap();
    let reply = pilot.request("select_path", json!({ "purpose": "projectRoot" }));
    assert_eq!(reply["error"]["code"], "input.invalid");
    assert_eq!(
        reply["error"]["message"],
        "A folder picker is already open."
    );
    assert!(reply.get("data").is_none());
}

#[cfg(windows)]
#[test]
fn export_binding_requires_the_reviewed_destination_digest_and_current_revision() {
    const MARKER: &str = "export-secret-canary-6f3d54";
    let pilot = Pilot::new();
    let original = pilot.create(&pilot.folder("catalog"));
    let parent = pilot.folder("exports");
    let plan_request = |folder, revision: &Value, previous: Option<&Path>| {
        json!({
            "registrationId": original["registrationId"],
            "expectedRevision": revision,
            "destination": pilot.selection(&parent, "exportParent"),
            "folderName": folder,
            "fromExport": previous.map(|path| pilot.selection(path, "previousExport")),
        })
    };
    let invalid = pilot.request(
        "generation_plan",
        plan_request("nested/folder", &original["revision"], None),
    );
    assert_eq!(invalid["error"]["field"], "/folderName");

    let plan = |folder, revision: &Value| {
        pilot.success("generation_plan", plan_request(folder, revision, None))
    };
    let denied = plan("no-publication", &original["revision"]);
    let wrong_digest = pilot.request(
        "generation_write",
        json!({
            "planId": denied["planId"],
            "planDigest": builder_app::ManifestRevision::of(b"other plan"),
        }),
    );
    assert_eq!(wrong_digest["error"]["code"], "export.input_changed");
    let publish = |review: &Value| {
        pilot.request(
            "generation_write",
            json!({ "planId": review["planId"], "planDigest": review["planDigest"] }),
        )
    };
    assert_eq!(publish(&denied)["error"]["code"], "export.input_changed");
    assert!(!parent.join("no-publication").exists());

    let stale = plan("stale", &original["revision"]);
    let mut document = original["document"].clone();
    document["name"] = json!("Catalog updated");
    let updated = pilot.success(
        "project_save",
        json!({
            "registrationId": original["registrationId"],
            "expectedRevision": original["revision"],
            "document": document,
        }),
    );
    assert_eq!(publish(&stale)["error"]["code"], "export.input_changed");
    assert!(!parent.join("stale").exists());

    let fresh = plan("fresh", &updated["revision"]);
    assert_eq!(publish(&fresh)["ok"], true);
    let export = parent.join("fresh");
    assert!(export.join("generation.json").is_file());
    assert_eq!(publish(&fresh)["error"]["code"], "export.input_changed");
    fs::write(export.join("runtime-config.local.json"), MARKER).unwrap();

    let regenerated = pilot.success(
        "generation_plan",
        plan_request("regenerated", &updated["revision"], Some(&export)),
    );
    let reply = publish(&regenerated);
    assert_eq!(reply["ok"], true, "{reply}");
    assert!(!reply.to_string().contains(MARKER));
    assert!(
        !parent
            .join("regenerated/runtime-config.local.json")
            .exists()
    );
}

#[test]
fn preview_rejects_invalid_credential_input_without_echoing_it() {
    let pilot = Pilot::new();
    let project = pilot.create(&pilot.folder("catalog"));
    let canary = "over-limit-secret".repeat(1200);
    let reply = pilot.request(
        "policy_review",
        json!({
            "subject": {
                "kind": "launch",
                "registrationId": project["registrationId"],
                "expectedRevision": project["revision"],
            },
            "mode": { "kind": "live" },
            "access": { "credentials": { "service": { "kind": "session", "value": canary } } },
        }),
    );
    assert_eq!(reply["error"]["code"], "credential.unavailable");
    assert!(!reply.to_string().contains("over-limit-secret"));
    assert!(reply.get("data").is_none());
}

#[cfg(windows)]
#[test]
#[ignore = "stage the built mcp-builder.exe next to the desktop test executable"]
fn reviewed_live_call_checks_scope_before_reading_and_retires_the_connection() {
    const MARKER: &str = "native-review-credential-canary-499d";
    const UNAPPROVED: &str = "unapproved-local-note-f8b3";
    let pilot = Pilot::new();
    let project_dir = pilot.folder("catalog");
    fs::write(
        project_dir.join(builder_project::MANIFEST_NAME),
        include_bytes!("../../../../../crates/builder-project/tests/fixtures/catalog-v1.json"),
    )
    .unwrap();
    let project = pilot.success(
        "project_open",
        json!({ "root": pilot.selection(&project_dir, "projectRoot") }),
    );
    let docs = pilot.folder("docs");
    fs::create_dir(docs.join("notes")).unwrap();
    fs::write(docs.join("notes/42.txt"), "Reviewed note").unwrap();
    fs::write(docs.join("notes/43.txt"), UNAPPROVED).unwrap();
    let subject = |revision: &Value| {
        json!({
            "kind": "launch",
            "registrationId": project["registrationId"],
            "expectedRevision": revision,
        })
    };
    let review = |subject| {
        pilot.request(
            "policy_review",
            json!({
                "subject": subject,
                "mode": { "kind": "live" },
                "access": {
                    "approvedGrantIds": ["catalog_read", "note_read"],
                    "fileRoots": { "catalog_docs": pilot.selection(&docs, "fileRoot") },
                    "credentials": { "catalog_auth": { "kind": "session", "value": MARKER } },
                },
                "capturePayloads": true,
            }),
        )
    };
    let stale = review(subject(&json!(builder_app::ManifestRevision::of(b"old"))));
    assert_eq!(stale["error"]["code"], "policy.approval_required");
    assert!(!stale.to_string().contains(MARKER));

    let previous_subject = subject(&project["revision"]);
    let previous_review = review(previous_subject.clone());
    assert_eq!(previous_review["ok"], true, "{previous_review}");
    let previous_approval = pilot.success(
        "policy_approve",
        json!({
            "reviewId": previous_review["data"]["reviewId"],
            "digest": previous_review["data"]["digest"],
        }),
    );
    let mut revised_document = project["document"].clone();
    revised_document["name"] = json!("Catalog updated after approval");
    let revised = pilot.success(
        "project_save",
        json!({
            "registrationId": project["registrationId"],
            "expectedRevision": project["revision"],
            "document": revised_document,
        }),
    );
    let stale_start = pilot.success(
        "playground_start",
        json!({
            "subject": previous_subject,
            "approval": { "approvalId": previous_approval["approvalId"] },
        }),
    );
    let rejected = pilot.terminal_event(&stale_start["operationId"]);
    assert_eq!(rejected["kind"], "failed", "{rejected}");
    assert_eq!(rejected["error"]["code"], "policy.approval_required");
    assert!(rejected.get("connectionId").is_none());

    let launch_subject = subject(&revised["revision"]);
    let review = review(launch_subject.clone());
    assert_eq!(review["ok"], true, "{review}");
    assert_eq!(
        review["data"]["summary"]["credentialSources"]["catalog_auth"],
        json!({ "kind": "session" })
    );
    assert!(!review.to_string().contains(MARKER));
    let approved = pilot.success(
        "policy_approve",
        json!({ "reviewId": review["data"]["reviewId"], "digest": review["data"]["digest"] }),
    );
    let started = pilot.success(
        "playground_start",
        json!({ "subject": launch_subject, "approval": { "approvalId": approved["approvalId"] } }),
    );
    let connection = pilot.terminal_event(&started["operationId"]);
    assert_eq!(connection["kind"], "started", "{connection}");
    let id = &connection["connectionId"];
    let discovered = pilot.success("playground_discover", json!({ "connectionId": id }));
    assert_eq!(discovered["resourceTemplates"][0]["name"], "Catalog note");

    let call = |leaf, timeout| {
        pilot.request(
            "playground_call",
            json!({
                "connectionId": id,
                "timeoutMs": timeout,
                "call": { "method": "resources/read", "uri": format!("catalog://docs/items/{leaf}") },
            }),
        )
    };
    assert_eq!(call("42", 0)["error"]["field"], "/timeoutMs");
    let denied = call("43", 1000);
    assert_eq!(denied["ok"], true, "{denied}");
    let result = pilot.terminal_event(&denied["data"]["runId"]);
    assert_eq!(result["run"]["result"]["outcome"], "denied");
    assert!(!result.to_string().contains(UNAPPROVED));
    let completed = pilot.success("run_cancel", json!({ "id": denied["data"]["runId"] }));
    assert_eq!(completed["cancellationRequested"], false);
    let allowed = call("42", 1000);
    assert_eq!(allowed["ok"], true, "{allowed}");
    let result = pilot.terminal_event(&allowed["data"]["runId"]);
    assert_eq!(
        result["run"]["payload"]["contents"][0]["text"],
        "Reviewed note"
    );
    let trace = pilot.success("playground_trace", json!({ "connectionId": id }));
    assert!(!trace.to_string().contains(MARKER));
    assert!(!trace.to_string().contains(UNAPPROVED));
    assert_eq!(
        pilot.success("playground_stop", json!({ "connectionId": id }))["stopped"],
        true,
    );
    assert_eq!(
        pilot.request("playground_discover", json!({ "connectionId": id }))["error"]["code"],
        "playground.connection_unavailable",
    );
}

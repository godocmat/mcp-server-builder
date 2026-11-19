#![cfg(any(windows, target_os = "linux", target_os = "macos"))]

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use builder_project::{parse_manifest, runtime_bindings, runtime_definition};
use mcp_policy::{ApprovedGrant, CallerGrant, CallerIdentity, DenyReason, Effect, OperatorPolicy};
use mcp_runtime::{CancellationToken, ErrorKind, ExecutionContext, Principal, Runtime};
use mcp_starter_adapters::{LocalBindings, StarterAdapter};
use serde_json::{Value, json};

const URI: &str = "catalog://docs/items/";
const LOCAL: Principal<'static> = Principal::Local {
    operator: "file-fixture",
};
const CANARY: &str = "harmless-outside-file-fixture";

fn runtime(root: Option<&Path>, grant_path: &str) -> Runtime {
    let mut manifest: Value = serde_json::from_slice(include_bytes!(
        "../../builder-project/tests/fixtures/catalog-v1.json"
    ))
    .unwrap();
    manifest["policy"]["grants"][1]["target"]["relativePathTemplate"] = json!(grant_path);
    manifest["policy"]["grants"][1]["limits"]["maxOutputBytes"] = json!(1024);
    manifest["policy"]["grants"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "id":"guide_read","capabilityId":"catalog_guide","operation":"file.read",
            "target":{"kind":"fileRead","rootRef":"catalog_docs","relativePathTemplate":"guide.txt"}
        }));
    let manifest = parse_manifest(&serde_json::to_vec(&manifest).unwrap()).unwrap();
    let definition = runtime_definition(&manifest);
    let grants: Vec<_> = definition
        .requested_policy
        .grants
        .iter()
        .filter(|grant| grant.capability_id != "lookup_item")
        .cloned()
        .map(ApprovedGrant::new)
        .collect();
    let approvals = OperatorPolicy {
        caller_grants: vec![CallerGrant {
            identity: CallerIdentity::Local {
                operator: "file-fixture".into(),
            },
            capability_ids: grants
                .iter()
                .map(|entry| entry.grant.capability_id.clone())
                .collect(),
            grant_ids: grants.iter().map(|entry| entry.grant.id.clone()).collect(),
            required_scopes: Default::default(),
            limits: Default::default(),
        }],
        approved_grants: grants,
        ..Default::default()
    };
    let mut local = LocalBindings::default();
    if let Some(root) = root {
        local.file_roots.insert("catalog_docs".into(), root.into());
    }
    let adapter = StarterAdapter::new(
        manifest.document().project_id,
        runtime_bindings(&manifest),
        local,
    )
    .unwrap();
    Runtime::new(definition, approvals, Arc::new(adapter)).unwrap()
}

#[tokio::test]
async fn approved_fixed_and_templated_reads_use_the_mapped_root() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("guide.txt"), "Catalog guide").unwrap();
    fs::create_dir(root.path().join("notes")).unwrap();
    fs::write(root.path().join("notes/42.txt"), "Known note").unwrap();
    fs::write(root.path().join("notes/43.txt"), CANARY).unwrap();
    let runtime = runtime(Some(root.path()), "notes/42.txt");
    let context = ExecutionContext::new(&LOCAL, CancellationToken::new());
    let read = runtime
        .read_resource("catalog://docs/guide", &context)
        .await
        .unwrap();
    assert_eq!(read.contents[0].text, "Catalog guide");
    let read = runtime
        .read_resource(&format!("{URI}42"), &context)
        .await
        .unwrap();
    assert_eq!(read.contents[0].text, "Known note");
    let present = runtime
        .read_resource(&format!("{URI}43"), &context)
        .await
        .unwrap_err();
    let missing = runtime
        .read_resource(&format!("{URI}44"), &context)
        .await
        .unwrap_err();
    assert_eq!(present.kind, ErrorKind::Denied);
    assert_eq!(present.denial, Some(DenyReason::TargetDenied));
    assert_eq!(present, missing);
    assert!(!format!("{present:?} {present} {}", present.message()).contains(CANARY));
    let outside = runtime
        .read_resource(&format!("{URI}%2e%2e%2fguide.txt"), &context)
        .await
        .unwrap_err();
    assert_eq!(outside.kind, ErrorKind::InvalidArguments);
    let unapproved = ExecutionContext::new(&Principal::Anonymous, CancellationToken::new());
    assert_eq!(
        runtime
            .read_resource("catalog://docs/guide", &unapproved)
            .await
            .unwrap_err()
            .kind,
        ErrorKind::UnknownResource,
    );
}

#[tokio::test]
async fn missing_mappings_and_invalid_or_oversized_files_report_sanitized_failures() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("notes")).unwrap();
    let uri = format!("{URI}42");
    let context = ExecutionContext::new(&LOCAL, CancellationToken::new());
    let unmapped = runtime(None, "notes/42.txt");
    let error = unmapped.read_resource(&uri, &context).await.unwrap_err();
    assert_eq!(error.kind, ErrorKind::AdapterFailed);
    let live = runtime(Some(root.path()), "notes/42.txt");
    let error = live.read_resource(&uri, &context).await.unwrap_err();
    assert_eq!(error.kind, ErrorKind::AdapterFailed);
    assert_eq!(error.effect, Effect::None);
    assert!(
        !format!("{error:?} {error} {}", error.message()).contains(root.path().to_str().unwrap())
    );
    fs::write(root.path().join("notes/42.txt"), [0xff]).unwrap();
    assert_eq!(
        live.read_resource(&uri, &context).await.unwrap_err().kind,
        ErrorKind::AdapterFailed
    );
    fs::write(root.path().join("notes/42.txt"), CANARY.repeat(1024)).unwrap();
    let error = live.read_resource(&uri, &context).await.unwrap_err();
    assert_eq!(error.kind, ErrorKind::OutputLimit);
    assert!(!format!("{error:?} {error} {}", error.message()).contains(CANARY));
    fs::write(root.path().join("notes/42.txt"), "ready").unwrap();
    assert_eq!(
        live.read_resource(&uri, &context).await.unwrap().contents[0].text,
        "ready"
    );
}

#[tokio::test]
async fn granted_link_path_does_not_read_the_linked_file() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("root");
    fs::create_dir_all(root.join("notes")).unwrap();
    let outside = parent.path().join("outside.txt");
    fs::write(&outside, CANARY).unwrap();
    fs::hard_link(&outside, root.join("notes/42.txt")).unwrap();
    let live = runtime(Some(&root), "notes/42.txt");
    let context = ExecutionContext::new(&LOCAL, CancellationToken::new());
    let uri = format!("{URI}42");
    let error = tokio::time::timeout(Duration::from_secs(2), live.read_resource(&uri, &context))
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Denied);
    assert_eq!(error.denial, Some(DenyReason::TargetDenied));
    assert!(!format!("{error:?} {error} {}", error.message()).contains(CANARY));
    assert_eq!(fs::read_to_string(outside).unwrap(), CANARY);
}

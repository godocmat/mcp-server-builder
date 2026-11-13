use builder_project::{
    Assertion, Capability, Fixture, Invocation, ScenarioMode, ScenarioV1, ValidatedManifest,
    parse_manifest, validate_document,
};
use mcp_policy::{
    ApprovedGrant, CallerGrant, CallerIdentity, DenyReason, ExecutionLimits, GrantTarget,
    LimitOverrides, OperatorPolicy, Redactor,
};
use mcp_runtime::{CancellationToken, ExecutionContext, Principal, Runtime, RuntimeError};
use serde_json::{Map, Value, json};

use super::{RunOutcome, ScenarioError, evaluate, evaluate_with_redactor, fixture_runtime};

fn manifest() -> ValidatedManifest {
    parse_manifest(include_bytes!(
        "../../../builder-project/tests/fixtures/catalog-v1.json"
    ))
    .unwrap()
}

fn approved_policy(manifest: &ValidatedManifest) -> OperatorPolicy {
    let project = manifest.document();
    OperatorPolicy {
        approved_grants: project
            .policy
            .grants
            .iter()
            .cloned()
            .map(ApprovedGrant::new)
            .collect(),
        approved_prompt_ids: ["review_item".into()].into(),
        caller_grants: vec![CallerGrant {
            identity: CallerIdentity::Local {
                operator: "fixture-test".into(),
            },
            capability_ids: project
                .capabilities
                .iter()
                .map(|capability| capability.id().into())
                .collect(),
            grant_ids: project
                .policy
                .grants
                .iter()
                .map(|grant| grant.id.clone())
                .collect(),
            required_scopes: Default::default(),
            limits: Default::default(),
        }],
        ..Default::default()
    }
}

fn principal() -> Principal<'static> {
    Principal::Local {
        operator: "fixture-test",
    }
}

fn item_arguments(id: &str) -> Map<String, Value> {
    Map::from_iter([("id".into(), json!(id))])
}

async fn call_fixture(
    runtime: &Runtime,
    call: &Invocation,
    context: &ExecutionContext<'_>,
) -> Result<Value, RuntimeError> {
    match call {
        Invocation::Tool { name, arguments } => runtime
            .call_tool(name, arguments.clone(), context)
            .await
            .map(|value| serde_json::to_value(value).unwrap()),
        Invocation::Resource { uri } => runtime
            .read_resource(uri, context)
            .await
            .map(|value| serde_json::to_value(value).unwrap()),
        Invocation::Prompt { name, arguments } => runtime
            .get_prompt(name, arguments.clone(), context)
            .await
            .map(|value| serde_json::to_value(value).unwrap()),
    }
}

#[tokio::test]
async fn saved_catalog_scenarios_use_the_shared_runtime() {
    let manifest = manifest();
    let principal = principal();
    for scenario in &manifest.document().scenarios {
        let runtime = fixture_runtime(&manifest, &scenario.id, approved_policy(&manifest)).unwrap();
        let context = ExecutionContext::new(&principal, CancellationToken::new());
        let response = call_fixture(&runtime, &scenario.call, &context).await;
        let evaluation = evaluate(scenario, response.as_ref().map_err(RuntimeError::code));
        assert_eq!(evaluation.outcome, RunOutcome::Passed, "{}", scenario.id);
        assert!(
            evaluation
                .assertions
                .iter()
                .all(|assertion| assertion.passed)
        );
    }
}

#[tokio::test]
async fn exact_fixture_paths_require_a_matching_approved_target() {
    let manifest = manifest();
    let principal = principal();
    let policy = approved_policy(&manifest);
    let runtime = fixture_runtime(&manifest, "known_item", policy.clone()).unwrap();
    let context = || ExecutionContext::new(&principal, CancellationToken::new());

    let missing = runtime
        .call_tool("lookup_item", item_arguments("43"), &context())
        .await
        .unwrap_err();
    assert_eq!(missing.code(), "fixture.missing");
    let invalid_path = runtime
        .call_tool("lookup_item", item_arguments(".."), &context())
        .await
        .unwrap_err();
    assert_eq!(invalid_path.code(), "input.invalid");

    let mut narrowed = policy;
    let GrantTarget::Rest { path_template, .. } = &mut narrowed.approved_grants[0].grant.target
    else {
        panic!("expected the catalog REST grant");
    };
    *path_template = "/items/43".into();
    let runtime = fixture_runtime(&manifest, "known_item", narrowed).unwrap();
    let denied = runtime
        .call_tool("lookup_item", item_arguments("42"), &context())
        .await
        .unwrap_err();
    assert_eq!(denied.code(), "policy.denied");
    assert_eq!(denied.denial, Some(DenyReason::TargetDenied));

    let runtime = fixture_runtime(&manifest, "note", approved_policy(&manifest)).unwrap();
    let denied = runtime
        .read_resource("catalog://docs/items/43", &context())
        .await
        .unwrap_err();
    assert_eq!(denied.denial, Some(DenyReason::TargetDenied));
}

#[tokio::test]
async fn synthetic_peer_respects_explicit_ranges_without_dns() {
    let manifest = manifest();
    let principal = principal();
    let mut policy = approved_policy(&manifest);
    let GrantTarget::Rest {
        allowed_peer_ranges,
        ..
    } = &mut policy.approved_grants[0].grant.target
    else {
        panic!("expected the catalog REST grant");
    };
    *allowed_peer_ranges = Some(vec!["127.0.0.1/32".parse().unwrap()]);
    let runtime = fixture_runtime(&manifest, "known_item", policy).unwrap();
    let context = ExecutionContext::new(&principal, CancellationToken::new());
    let denied = runtime
        .call_tool("lookup_item", item_arguments("42"), &context)
        .await
        .unwrap_err();
    assert_eq!(denied.denial, Some(DenyReason::TargetDenied));
}

#[tokio::test]
async fn runtime_applies_approved_limits_cancellation_and_visibility() {
    let manifest = manifest();
    let principal = principal();
    let mut policy = approved_policy(&manifest);
    policy.approved_grants[0].grant.limits = Some(LimitOverrides {
        max_output_bytes: Some(80.try_into().unwrap()),
        ..Default::default()
    });
    let runtime = fixture_runtime(&manifest, "known_item", policy.clone()).unwrap();
    let context = ExecutionContext::new(&principal, CancellationToken::new());
    let oversized = runtime
        .call_tool("lookup_item", item_arguments("42"), &context)
        .await
        .unwrap_err();
    assert_eq!(oversized.code(), "runtime.output_limit");

    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let context = ExecutionContext::new(&principal, cancellation);
    let cancelled = runtime
        .call_tool("lookup_item", item_arguments("42"), &context)
        .await
        .unwrap_err();
    assert_eq!(cancelled.code(), "run.cancelled");

    policy.approved_grants.clear();
    let runtime = fixture_runtime(&manifest, "known_item", policy).unwrap();
    let context = ExecutionContext::new(&principal, CancellationToken::new());
    let hidden = runtime
        .call_tool("lookup_item", item_arguments("42"), &context)
        .await
        .unwrap_err();
    assert_eq!(hidden.code(), "mcp.unknown_tool");
}

#[tokio::test]
async fn synthetic_http_status_and_body_follow_runtime_output_contract() {
    let manifest = manifest();
    let principal = principal();
    let mut document = manifest.into_document();
    let mut fixture_response = |status, body, remove_output_schema| {
        document.scenarios[0].fixtures = Some(vec![Fixture::Rest {
            binding_id: "catalog_lookup".into(),
            method: "GET".into(),
            path: "/items/42".into(),
            status,
            body,
        }]);
        if remove_output_schema {
            let Capability::Tool(tool) = &mut document.capabilities[0] else {
                panic!("expected the catalog tool");
            };
            tool.output_schema = None;
        }
        let manifest = validate_document(document.clone()).unwrap();
        fixture_runtime(&manifest, "known_item", approved_policy(&manifest)).unwrap()
    };
    let context = || ExecutionContext::new(&principal, CancellationToken::new());
    for (status, body, code) in [
        (
            503,
            json!({"message":"Temporary failure"}),
            "adapter.failed",
        ),
        (200, json!([42]), "mcp.invalid_output"),
    ] {
        let runtime = fixture_response(status, body, false);
        let error = runtime
            .call_tool("lookup_item", item_arguments("42"), &context())
            .await
            .unwrap_err();
        assert_eq!(error.code(), code);
    }
    for (status, expected) in [(200, "[42]"), (204, ""), (205, "")] {
        let runtime = fixture_response(status, json!([42]), true);
        let result = runtime
            .call_tool("lookup_item", item_arguments("42"), &context())
            .await
            .unwrap();
        assert_eq!(result.content[0].text, expected);
        assert!(result.structured_content.is_none());
    }
}

#[test]
fn fixture_constructor_requires_a_saved_fixture_mode_scenario() {
    let mut document = manifest().into_document();
    document.scenarios[0].fixtures = None;
    document.scenarios[0].mode = ScenarioMode::Live;
    let manifest = validate_document(document).unwrap();
    assert_eq!(
        fixture_runtime(&manifest, "absent", approved_policy(&manifest)).err(),
        Some(ScenarioError::NotFound)
    );
    assert_eq!(
        fixture_runtime(&manifest, "known_item", approved_policy(&manifest)).err(),
        Some(ScenarioError::LiveRequiresOptIn)
    );
}

fn scenario_with(assertions: Vec<Assertion>) -> ScenarioV1 {
    let mut scenario = manifest().into_document().scenarios.remove(0);
    scenario.assertions = assertions;
    scenario
}

#[test]
fn assertion_failures_keep_sanitized_paths_and_fixed_metadata() {
    let mut scenario = scenario_with(vec![
        Assertion::Equals {
            path: "/structuredContent/a~1b~0/0".into(),
            expected: json!({"z":1,"a":2}),
        },
        Assertion::Schema {
            path: "/structuredContent/name".into(),
            schema: json!({"type":"integer"}),
        },
        Assertion::Equals {
            path: "/structuredContent/private/sensitive-marker".into(),
            expected: json!("expected-marker"),
        },
        Assertion::Equals {
            path: "/structuredContent/Authorization".into(),
            expected: json!("expected-marker"),
        },
        Assertion::Equals {
            path: "/structuredContent/missing".into(),
            expected: Value::Null,
        },
    ]);
    scenario.sensitive_paths = Some(vec!["/structuredContent/private".into()]);
    let response = json!({
        "content":[],
        "structuredContent": {
            "a/b~":[{"a":2,"z":1}],
            "name":"actual-marker",
            "private":{"sensitive-marker":"actual-marker"},
            "Authorization":"actual-marker"
        }
    });
    let result = evaluate(&scenario, Ok(&response));
    assert_eq!(result.outcome, RunOutcome::AssertionFailed);
    assert_eq!(result.diagnostic_code, Some("scenario.assertion_failed"));
    assert!(result.assertions[0].passed);
    assert_eq!(
        result.assertions[1].path.as_deref(),
        Some("/structuredContent/name")
    );
    assert_eq!(result.assertions[1].code, Some("scenario.schema_mismatch"));
    assert_eq!(result.assertions[2].code, Some("scenario.value_mismatch"));
    assert_eq!(result.assertions[4].code, Some("scenario.path_missing"));
    let evidence = serde_json::to_string(&result).unwrap();
    for marker in [
        "expected-marker",
        "actual-marker",
        "sensitive-marker",
        "Authorization",
    ] {
        assert!(!evidence.contains(marker));
    }
}

#[test]
fn assertion_paths_mask_known_secrets_in_pointer_components() {
    let scenario = scenario_with(vec![
        Assertion::Equals {
            path: "/structuredContent/a~1b~0".into(),
            expected: json!(42),
        },
        Assertion::Equals {
            path: "/structuredContent/id".into(),
            expected: json!(42),
        },
    ]);
    let response = json!({"content":[], "structuredContent":{"a/b~":42, "id":42}});
    let redactor = Redactor::new(["a/b~"], [] as [&str; 0]).unwrap();
    let result = evaluate_with_redactor(&scenario, Ok(&response), &redactor);
    assert_eq!(result.outcome, RunOutcome::Passed);
    assert!(result.assertions[0].path.is_none());
    assert_eq!(
        result.assertions[1].path.as_deref(),
        Some("/structuredContent/id")
    );
}

#[test]
fn error_assertions_preserve_missing_fixture_and_cancellation_outcomes() {
    for (code, outcome) in [
        ("fixture.missing", RunOutcome::Failed),
        ("run.cancelled", RunOutcome::Cancelled),
        ("runtime.timeout", RunOutcome::Passed),
        ("policy.denied", RunOutcome::Passed),
        ("adapter.failed", RunOutcome::Passed),
        ("input.invalid", RunOutcome::Passed),
    ] {
        let scenario = scenario_with(vec![Assertion::ErrorCode { code: code.into() }]);
        assert_eq!(evaluate(&scenario, Err(code)).outcome, outcome);
    }

    let scenario = scenario_with(vec![]);
    for (code, outcome) in [
        ("runtime.timeout", RunOutcome::TimedOut),
        ("policy.denied", RunOutcome::Denied),
        ("input.invalid", RunOutcome::Invalid),
        ("transport.disconnected", RunOutcome::Failed),
    ] {
        assert_eq!(evaluate(&scenario, Err(code)).outcome, outcome);
    }

    let scenario = scenario_with(vec![Assertion::ErrorCode {
        code: "policy.denied".into(),
    }]);
    let result = evaluate(&scenario, Err("adapter.failed"));
    assert_eq!(result.outcome, RunOutcome::AssertionFailed);
    assert_eq!(
        result.assertions[0].code,
        Some("scenario.error_code_mismatch")
    );
}

#[test]
fn normalized_results_are_bounded_and_assertion_schemas_stay_offline() {
    let scenario = scenario_with(vec![Assertion::Schema {
        path: "".into(),
        schema: json!({"$ref":"https://example.test/schema.json"}),
    }]);
    let empty = json!({"content":[]});
    let result = evaluate(&scenario, Ok(&empty));
    assert_eq!(result.outcome, RunOutcome::Invalid);
    assert_eq!(
        result.assertions[0].code,
        Some("scenario.invalid_assertion")
    );

    let oversized =
        json!({"content":[{"text":"x".repeat(ExecutionLimits::MAX_OUTPUT_BYTES as usize)}]});
    let deep = (0..64).fold(empty, |value, _| json!([value]));
    for response in [oversized, deep] {
        let result = evaluate(&scenario, Ok(&response));
        assert_eq!(result.outcome, RunOutcome::Failed);
        assert_eq!(result.diagnostic_code, Some("runtime.output_limit"));
        assert!(result.assertions.is_empty());
    }
}

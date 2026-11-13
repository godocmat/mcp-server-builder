use std::num::NonZeroU64;
use std::time::{Duration, Instant};

use builder_project::{Capability, parse_manifest};
use mcp_policy::{
    Action, ApprovedGrant, CallerGrant, CallerIdentity, DenyReason, HttpMethod, Operation,
    OperationContext, OperatorPolicy, Policy, Principal, VerifiedTarget,
};

const CATALOG: &[u8] = include_bytes!("fixtures/catalog-v1.json");
const OPERATOR: &str = "fixture-operator";

#[test]
fn schema_one_policy_drives_the_shared_evaluator() {
    let manifest = parse_manifest(CATALOG).unwrap();
    let document = manifest.document();
    let requested = &document.policy;
    requested.validate().unwrap();
    let operator = OperatorPolicy {
        approved_grants: requested
            .grants
            .iter()
            .cloned()
            .map(ApprovedGrant::new)
            .collect(),
        caller_grants: vec![CallerGrant {
            identity: CallerIdentity::Local {
                operator: OPERATOR.into(),
            },
            capability_ids: requested
                .grants
                .iter()
                .map(|grant| grant.capability_id.clone())
                .collect(),
            grant_ids: requested
                .grants
                .iter()
                .map(|grant| grant.id.clone())
                .collect(),
            required_scopes: Default::default(),
            limits: Default::default(),
        }],
        ..Default::default()
    };
    let policy = Policy::new(requested, &operator).unwrap();
    let principal = Principal::Local { operator: OPERATOR };
    let now = Instant::now();
    let origin = "https://catalog.example.test".parse().unwrap();
    let tool = document
        .capabilities
        .iter()
        .find_map(|capability| match capability {
            Capability::Tool(tool) if tool.id == "lookup_item" => Some(tool),
            _ => None,
        })
        .unwrap();
    let rest = VerifiedTarget::Rest {
        origin: &origin,
        method: HttpMethod::Get,
        path: "/items/42",
        peer: "93.184.215.14".parse().unwrap(),
        redirected: false,
    };
    let rest_call = OperationContext {
        principal: &principal,
        capability_id: &tool.id,
        action: Action::Adapter {
            operation: Operation::HttpRead,
            target: &rest,
        },
        capability_timeout_ms: tool.timeout_ms.and_then(NonZeroU64::new),
        requested_limits: Default::default(),
        ceiling: None,
        admitted_at: now,
        now,
    };
    let permit = policy.evaluate(&rest_call).unwrap();
    assert_eq!(permit.grant_id(), Some("catalog_read"));
    assert_eq!(permit.limits().timeout(), Duration::from_secs(1));
    assert_eq!(permit.limits().max_output_bytes(), 16_384);

    for (path, allowed) in [("notes/42.txt", true), ("notes/43.txt", false)] {
        let file = VerifiedTarget::FileRead {
            root_ref: "catalog_docs",
            relative_path: path,
        };
        let file_call = OperationContext {
            capability_id: "catalog_note",
            action: Action::Adapter {
                operation: Operation::FileRead,
                target: &file,
            },
            capability_timeout_ms: None,
            ..rest_call
        };
        let decision = policy.evaluate(&file_call);
        if allowed {
            assert_eq!(decision.unwrap().grant_id(), Some("note_read"));
        } else {
            assert_eq!(decision.unwrap_err(), DenyReason::TargetDenied);
        }
    }
}

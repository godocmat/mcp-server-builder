use std::num::NonZeroU64;
use std::time::{Duration, Instant};

use mcp_policy::{
    Action, ApprovedGrant, CallerGrant, CallerIdentity, CapabilityAction, DatabaseAccess,
    DenyReason, ExecutionLimits, GitOperation, GrantTarget, HttpMethod, HttpOrigin, LimitOverrides,
    Operation, OperationContext, OperatorPolicy, Policy, Principal, RequestedPolicy, RestRoute,
    VerifiedTarget,
};
use serde_json::{Value, json};

const CATALOG: &str = "lookup_item";
const CATALOG_GRANT: &str = "catalog_read";
const OPERATOR: &str = "local-operator";
const ISSUER: &str = "https://issuer.example.test";
const PUBLIC_PEER: &str = "93.184.215.14";
const ORIGIN: &str = "https://catalog.example.test";

fn requested(grants: Value) -> RequestedPolicy {
    serde_json::from_value(json!({ "grants": grants })).unwrap()
}

fn catalog() -> RequestedPolicy {
    requested(json!([{
        "id": CATALOG_GRANT,
        "capabilityId": CATALOG,
        "operation": "http.read",
        "target": { "kind": "rest", "origin": ORIGIN, "method": "GET", "pathTemplate": "/items/{id}" }
    }]))
}

fn approval(requests: &RequestedPolicy) -> OperatorPolicy {
    OperatorPolicy {
        approved_grants: requests
            .grants
            .iter()
            .cloned()
            .map(ApprovedGrant::new)
            .collect(),
        caller_grants: vec![CallerGrant {
            identity: CallerIdentity::Local {
                operator: OPERATOR.into(),
            },
            capability_ids: requests
                .grants
                .iter()
                .map(|grant| grant.capability_id.clone())
                .collect(),
            grant_ids: requests
                .grants
                .iter()
                .map(|grant| grant.id.clone())
                .collect(),
            required_scopes: Default::default(),
            limits: LimitOverrides::default(),
        }],
        ..OperatorPolicy::default()
    }
}

fn rest<'a>(origin: &'a HttpOrigin, path: &'a str, peer: &str) -> VerifiedTarget<'a> {
    VerifiedTarget::Rest {
        origin,
        method: HttpMethod::Get,
        path,
        peer: peer.parse().unwrap(),
        redirected: false,
    }
}

fn context<'a>(
    principal: &'a Principal<'a>,
    capability_id: &'a str,
    target: &'a VerifiedTarget<'a>,
    now: Instant,
) -> OperationContext<'a> {
    OperationContext {
        principal,
        capability_id,
        action: Action::Adapter {
            operation: Operation::from(target),
            target,
        },
        capability_timeout_ms: None,
        requested_limits: LimitOverrides::default(),
        ceiling: None,
        admitted_at: now,
        now,
    }
}

#[test]
fn portable_request_round_trips_and_requires_operator_and_caller_authority() {
    let request = catalog();
    assert_eq!(
        serde_json::to_value(&request).unwrap(),
        json!({ "grants": [{
        "id": CATALOG_GRANT, "capabilityId": CATALOG, "operation": "http.read",
        "target": { "kind": "rest", "origin": ORIGIN, "method": "GET", "pathTemplate": "/items/{id}" }
    }] })
    );

    let origin = ORIGIN.parse().unwrap();
    let target = rest(&origin, "/items/42", PUBLIC_PEER);
    let now = Instant::now();
    let local = Principal::Local { operator: OPERATOR };
    let call = context(&local, CATALOG, &target, now);
    let missing_operator = OperatorPolicy::default();
    assert_eq!(
        Policy::new(&request, &missing_operator)
            .unwrap()
            .evaluate(&call)
            .unwrap_err(),
        DenyReason::CallerDenied
    );

    let mut operator = approval(&request);
    operator.approved_grants.clear();
    assert_eq!(
        Policy::new(&request, &operator)
            .unwrap()
            .evaluate(&call)
            .unwrap_err(),
        DenyReason::GrantRequired
    );

    let mut operator = approval(&request);
    operator.caller_grants[0].grant_ids.clear();
    assert_eq!(
        Policy::new(&request, &operator)
            .unwrap()
            .evaluate(&call)
            .unwrap_err(),
        DenyReason::CallerDenied
    );

    let operator = approval(&request);
    let policy = Policy::new(&request, &operator).unwrap();
    assert_eq!(
        policy.evaluate(&call).unwrap().grant_id(),
        Some(CATALOG_GRANT)
    );
    assert_eq!(
        policy
            .evaluate(&context(&Principal::Anonymous, CATALOG, &target, now))
            .unwrap_err(),
        DenyReason::PrincipalRequired
    );
    assert_eq!(
        policy
            .evaluate(&context(&local, "other_capability", &target, now))
            .unwrap_err(),
        DenyReason::CapabilityDenied
    );
    assert!(policy.is_visible(
        &local,
        CATALOG,
        CapabilityAction::Adapter(Operation::HttpRead),
        now
    ));
    assert!(!policy.is_visible(
        &local,
        CATALOG,
        CapabilityAction::Adapter(Operation::HttpWrite),
        now
    ));
    assert!(!policy.is_visible(
        &Principal::Anonymous,
        CATALOG,
        CapabilityAction::Adapter(Operation::HttpRead),
        now
    ));

    let mut effects = 0;
    for caller in [&Principal::Anonymous, &local] {
        if policy
            .evaluate(&context(caller, CATALOG, &target, now))
            .is_ok()
        {
            effects += 1;
        }
    }
    assert_eq!(effects, 1);
}

#[test]
fn malformed_or_ambiguous_policy_fails_closed() {
    for invalid in [
        json!(null),
        json!({}),
        json!([[]]),
        json!({ "grants": null }),
        json!({ "grants": [], "approveAll": true }),
        json!({ "grants": [[CATALOG_GRANT, CATALOG, "http.read", {
            "kind": "rest", "origin": ORIGIN, "method": "GET", "pathTemplate": "/items/{id}"
        }]] }),
    ] {
        assert!(serde_json::from_value::<RequestedPolicy>(invalid).is_err());
    }

    let mut valid = serde_json::to_value(catalog()).unwrap();
    for (key, invalid) in [
        ("limits", json!(null)),
        ("limits", json!({ "timeoutMs": 0 })),
        ("limits", json!({ "maxOutputBytes": null })),
        ("limits", json!({ "timeoutMs": -1 })),
        ("limits", json!([500, 1024])),
        ("operation", json!("http.*")),
        ("target", json!({ "kind": "anything" })),
        ("target", json!(["fileRead", "docs"])),
    ] {
        let mut input = valid.clone();
        input["grants"][0][key] = invalid;
        assert!(serde_json::from_value::<RequestedPolicy>(input).is_err());
    }
    valid["grants"][0]["target"]["allowedPeerRanges"] = json!(["10.0.0.0/33"]);
    assert!(serde_json::from_value::<RequestedPolicy>(valid).is_err());
    let shape_error =
        serde_json::from_str::<RequestedPolicy>(r#"{"grants":[],"grants":[]}"#).unwrap_err();
    assert!(shape_error.to_string().contains("policy.invalid_shape"));
    let shape_error = serde_json::from_value::<RequestedPolicy>(json!({
        "grants": [], "synthetic-unknown-key": "synthetic-value"
    }))
    .unwrap_err();
    assert!(!shape_error.to_string().contains("synthetic-"));

    let mut invalid = catalog();
    invalid.grants[0].operation = Operation::HttpWrite;
    let error = invalid.validate().unwrap_err();
    assert_eq!(error.code(), "policy.operation_target_mismatch");
    assert_eq!(error.field(), "/grants/0/operation");

    let mut ambiguous = catalog();
    ambiguous.grants.push(ambiguous.grants[0].clone());
    assert_eq!(ambiguous.validate().unwrap_err().field(), "/grants/1/id");

    let valid = catalog();
    let mut operator = approval(&valid);
    operator.approved_grants[0].grant.id = "unrequested".into();
    assert_eq!(
        Policy::new(&valid, &operator).err().unwrap().code(),
        "policy.unknown_approved_grant"
    );

    let mut operator = approval(&valid);
    operator
        .caller_grants
        .push(operator.caller_grants[0].clone());
    assert_eq!(
        Policy::new(&valid, &operator).err().unwrap().field(),
        "/callerGrants/1/identity"
    );
}

#[test]
fn remote_identity_scope_capability_and_grant_are_all_required() {
    let request = catalog();
    let mut operator = approval(&request);
    let mut rule = operator.caller_grants[0].clone();
    rule.identity = CallerIdentity::Remote {
        issuer: ISSUER.into(),
        subject: "alice".into(),
        client_id: None,
    };
    rule.required_scopes.insert("catalog:read".into());
    operator.caller_grants.push(rule.clone());
    rule.identity = CallerIdentity::Remote {
        issuer: ISSUER.into(),
        subject: "alice".into(),
        client_id: Some("restricted".into()),
    };
    rule.grant_ids.clear();
    operator.caller_grants.push(rule.clone());
    rule.identity = CallerIdentity::Remote {
        issuer: ISSUER.into(),
        subject: "bob".into(),
        client_id: None,
    };
    operator.caller_grants.push(rule);
    let policy = Policy::new(&request, &operator).unwrap();
    let origin = ORIGIN.parse().unwrap();
    let target = rest(&origin, "/items/42", PUBLIC_PEER);
    let now = Instant::now();
    let valid_until = now + Duration::from_secs(2);
    let mut remote = Principal::Remote {
        issuer: ISSUER,
        subject: "alice",
        client_id: Some("regular"),
        scopes: &["catalog:read"],
        expires_at: valid_until,
    };
    assert_eq!(
        policy
            .evaluate(&context(&remote, CATALOG, &target, now))
            .unwrap()
            .deadline(),
        valid_until
    );
    assert!(policy.is_visible(
        &remote,
        CATALOG,
        CapabilityAction::Adapter(Operation::HttpRead),
        now
    ));
    assert_eq!(
        policy
            .evaluate(&context(&remote, CATALOG, &target, valid_until))
            .unwrap_err(),
        DenyReason::AuthorizationExpired
    );

    for (issuer, subject, client, scopes, reason) in [
        (
            ISSUER,
            "alice",
            Some("regular"),
            &["other:read"][..],
            DenyReason::InsufficientScope,
        ),
        (
            ISSUER,
            "alice",
            Some("restricted"),
            &["catalog:read"][..],
            DenyReason::CallerDenied,
        ),
        (
            ISSUER,
            "bob",
            Some("regular"),
            &["catalog:read"][..],
            DenyReason::CallerDenied,
        ),
        (
            "https://different.example.test",
            "alice",
            None,
            &["catalog:read"][..],
            DenyReason::CallerDenied,
        ),
    ] {
        remote = Principal::Remote {
            issuer,
            subject,
            client_id: client,
            scopes,
            expires_at: valid_until,
        };
        assert_eq!(
            policy
                .evaluate(&context(&remote, CATALOG, &target, now))
                .unwrap_err(),
            reason
        );
        assert!(!policy.is_visible(
            &remote,
            CATALOG,
            CapabilityAction::Adapter(Operation::HttpRead),
            now
        ));
    }
    assert_eq!(
        serde_json::to_value(DenyReason::InsufficientScope).unwrap(),
        "auth.insufficient_scope"
    );
}

#[test]
fn prompt_visibility_and_rendering_require_explicit_admission() {
    let requests = RequestedPolicy::default();
    let mut operator = approval(&requests);
    let now = Instant::now();
    let principal = Principal::Local { operator: OPERATOR };
    operator.caller_grants[0]
        .capability_ids
        .insert("review".into());
    let context = OperationContext {
        principal: &principal,
        capability_id: "review",
        action: Action::RenderPrompt,
        capability_timeout_ms: None,
        requested_limits: LimitOverrides::default(),
        ceiling: None,
        admitted_at: now,
        now,
    };
    assert_eq!(
        Policy::new(&requests, &operator)
            .unwrap()
            .evaluate(&context)
            .unwrap_err(),
        DenyReason::GrantRequired
    );
    operator.approved_prompt_ids.insert("review".into());
    let policy = Policy::new(&requests, &operator).unwrap();
    assert!(policy.is_visible(&principal, "review", CapabilityAction::RenderPrompt, now));
    assert!(!policy.is_visible(
        &principal,
        "review",
        CapabilityAction::Adapter(Operation::FileRead),
        now
    ));
    let permit = policy.evaluate(&context).unwrap();
    assert_eq!(permit.grant_id(), None);
    assert_eq!(permit.limits(), ExecutionLimits::default());
}

#[test]
fn rest_checks_origin_method_path_and_already_verified_peer() {
    let request = catalog();
    let operator = approval(&request);
    let policy = Policy::new(&request, &operator).unwrap();
    let local = Principal::Local { operator: OPERATOR };
    let now = Instant::now();
    let origin = "HTTPS://CATALOG.example.test:443/".parse().unwrap();
    let different_port = "https://catalog.example.test:444".parse().unwrap();
    let different_scheme = "http://catalog.example.test".parse().unwrap();
    assert!(
        policy
            .evaluate(&context(
                &local,
                CATALOG,
                &rest(&origin, "/items/%34%32", PUBLIC_PEER),
                now
            ))
            .is_ok()
    );
    for (origin, path, peer, reason) in [
        (
            &different_port,
            "/items/42",
            PUBLIC_PEER,
            DenyReason::TargetDenied,
        ),
        (
            &different_scheme,
            "/items/42",
            PUBLIC_PEER,
            DenyReason::TargetDenied,
        ),
        (&origin, "/items/42/", PUBLIC_PEER, DenyReason::TargetDenied),
        (
            &origin,
            "/elsewhere/42",
            PUBLIC_PEER,
            DenyReason::TargetDenied,
        ),
        (
            &origin,
            "/items/42/child",
            PUBLIC_PEER,
            DenyReason::TargetDenied,
        ),
        (
            &origin,
            "/items/%2e%2e",
            PUBLIC_PEER,
            DenyReason::InvalidTarget,
        ),
        (
            &origin,
            "/items/%2Fprivate",
            PUBLIC_PEER,
            DenyReason::InvalidTarget,
        ),
        (
            &origin,
            "/items/%252Fprivate",
            PUBLIC_PEER,
            DenyReason::InvalidTarget,
        ),
        (
            &origin,
            "/items/42?mode=other",
            PUBLIC_PEER,
            DenyReason::InvalidTarget,
        ),
        (
            &origin,
            "/items/42",
            "::ffff:127.0.0.1",
            DenyReason::TargetDenied,
        ),
        (
            &origin,
            "/items/42",
            "169.254.169.254",
            DenyReason::TargetDenied,
        ),
        (&origin, "/items/42", "fd00::1", DenyReason::TargetDenied),
    ] {
        let target = rest(origin, path, peer);
        assert_eq!(
            policy
                .evaluate(&context(&local, CATALOG, &target, now))
                .unwrap_err(),
            reason
        );
    }
    let target = VerifiedTarget::Rest {
        origin: &origin,
        method: HttpMethod::Post,
        path: "/items/42",
        peer: PUBLIC_PEER.parse().unwrap(),
        redirected: false,
    };
    let mut call = context(&local, CATALOG, &target, now);
    assert_eq!(
        policy.evaluate(&call).unwrap_err(),
        DenyReason::GrantRequired
    );
    call.action = Action::Adapter {
        operation: Operation::HttpRead,
        target: &target,
    };
    assert_eq!(
        policy.evaluate(&call).unwrap_err(),
        DenyReason::OperationMismatch
    );
}

#[test]
fn repeated_path_variables_match_the_same_segment_value() {
    let mut request = catalog();
    let GrantTarget::Rest { path_template, .. } = &mut request.grants[0].target else {
        unreachable!()
    };
    *path_template = "/items/{id}/notes/{id}.txt".into();
    let operator = approval(&request);
    let policy = Policy::new(&request, &operator).unwrap();
    let local = Principal::Local { operator: OPERATOR };
    let origin = ORIGIN.parse().unwrap();
    let now = Instant::now();
    let matching = rest(&origin, "/items/42/notes/%34%32.txt", PUBLIC_PEER);
    assert!(
        policy
            .evaluate(&context(&local, CATALOG, &matching, now))
            .is_ok()
    );
    let mismatched = rest(&origin, "/items/42/notes/43.txt", PUBLIC_PEER);
    assert_eq!(
        policy
            .evaluate(&context(&local, CATALOG, &mismatched, now))
            .unwrap_err(),
        DenyReason::TargetDenied
    );
}

#[test]
fn rest_internal_ranges_and_redirects_require_matching_narrow_approval() {
    let mut request = catalog();
    let GrantTarget::Rest {
        allowed_peer_ranges,
        ..
    } = &mut request.grants[0].target
    else {
        unreachable!()
    };
    *allowed_peer_ranges = Some(vec!["10.1.0.0/16".parse().unwrap()]);
    let mut invalid = request.clone();
    let GrantTarget::Rest {
        allowed_peer_ranges,
        ..
    } = &mut invalid.grants[0].target
    else {
        unreachable!()
    };
    *allowed_peer_ranges = Some(vec!["10.1.2.7/24".parse().unwrap()]);
    assert_eq!(
        invalid.validate().unwrap_err().code(),
        "policy.invalid_peer_range"
    );

    let mut operator = approval(&request);
    let GrantTarget::Rest {
        allowed_peer_ranges,
        ..
    } = &mut operator.approved_grants[0].grant.target
    else {
        unreachable!()
    };
    *allowed_peer_ranges = Some(vec!["10.1.2.0/24".parse().unwrap()]);
    let origin = ORIGIN.parse().unwrap();
    let now = Instant::now();
    let local = Principal::Local { operator: OPERATOR };
    let policy = Policy::new(&request, &operator).unwrap();
    let target = rest(&origin, "/items/42", "10.1.2.7");
    assert!(
        policy
            .evaluate(&context(&local, CATALOG, &target, now))
            .is_ok()
    );
    for address in ["10.1.3.7", "10.2.0.1", PUBLIC_PEER] {
        let target = rest(&origin, "/items/42", address);
        assert_eq!(
            policy
                .evaluate(&context(&local, CATALOG, &target, now))
                .unwrap_err(),
            DenyReason::TargetDenied
        );
    }
    let redirect = VerifiedTarget::Rest {
        origin: &origin,
        method: HttpMethod::Get,
        path: "/items/42",
        peer: "10.1.2.7".parse().unwrap(),
        redirected: true,
    };
    assert_eq!(
        policy
            .evaluate(&context(&local, CATALOG, &redirect, now))
            .unwrap_err(),
        DenyReason::RedirectDenied
    );
    operator.approved_grants[0].redirects.push(RestRoute {
        origin: origin.clone(),
        method: HttpMethod::Get,
        path: "/items/42".into(),
    });
    let policy = Policy::new(&request, &operator).unwrap();
    assert!(
        policy
            .evaluate(&context(&local, CATALOG, &redirect, now))
            .is_ok()
    );
    let other_redirect = VerifiedTarget::Rest {
        origin: &origin,
        method: HttpMethod::Get,
        path: "/items/43",
        peer: "10.1.2.7".parse().unwrap(),
        redirected: true,
    };
    assert_eq!(
        policy
            .evaluate(&context(&local, CATALOG, &other_redirect, now))
            .unwrap_err(),
        DenyReason::RedirectDenied
    );
}

#[test]
fn file_scope_requires_root_and_safe_matching_path() {
    let request = requested(json!([{
        "id": "notes_read", "capabilityId": "notes", "operation": "file.read",
        "target": { "kind": "fileRead", "rootRef": "docs" }
    }]));
    let mut operator = approval(&request);
    operator.approved_grants[0].grant.target = GrantTarget::FileRead {
        root_ref: "docs".into(),
        relative_path_template: Some("notes/{id}.txt".into()),
    };
    let policy = Policy::new(&request, &operator).unwrap();
    let local = Principal::Local { operator: OPERATOR };
    let now = Instant::now();
    let allowed = VerifiedTarget::FileRead {
        root_ref: "docs",
        relative_path: "notes/42.txt",
    };
    assert!(
        policy
            .evaluate(&context(&local, "notes", &allowed, now))
            .is_ok()
    );
    for (root_ref, relative_path, reason) in [
        ("other", "notes/42.txt", DenyReason::TargetDenied),
        ("docs", "notes/private/42.txt", DenyReason::TargetDenied),
        ("docs", "notes/.txt", DenyReason::TargetDenied),
        ("docs", "notes-other/42.txt", DenyReason::TargetDenied),
        ("docs", "notes/../42.txt", DenyReason::InvalidTarget),
        ("docs", "notes/CON.txt", DenyReason::InvalidTarget),
        ("docs", "notes/42.txt:other", DenyReason::InvalidTarget),
        ("docs", "C:\\notes\\42.txt", DenyReason::InvalidTarget),
        ("docs", "/notes/42.txt", DenyReason::InvalidTarget),
    ] {
        let target = VerifiedTarget::FileRead {
            root_ref,
            relative_path,
        };
        assert_eq!(
            policy
                .evaluate(&context(&local, "notes", &target, now))
                .unwrap_err(),
            reason
        );
    }
}

#[test]
fn other_adapter_grants_bind_kind_operation_and_references() {
    let request = requested(json!([
        { "id": "sq", "capabilityId": "sqlite", "operation": "db.read", "target": {
            "kind": "sqlite", "targetRef": "db", "bindingId": "fixed"
        }},
        { "id": "pg", "capabilityId": "pg", "operation": "db.write", "target": {
            "kind": "postgres", "targetRef": "db", "bindingId": "fixed"
        }},
        { "id": "git", "capabilityId": "git", "operation": "git.read", "target": {
            "kind": "git", "repositoryRef": "repo", "operation": "readDiff"
        }},
        { "id": "exec", "capabilityId": "exec", "operation": "exec.run", "target": {
            "kind": "executable", "executableRef": "program", "cwdRef": "work", "bindingId": "fixed"
        }},
        { "id": "custom", "capabilityId": "custom", "operation": "custom.invoke", "target": {
            "kind": "customRust", "source": "src/handler.rs", "entry": "invoke"
        }}
    ]));
    let operator = approval(&request);
    let policy = Policy::new(&request, &operator).unwrap();
    let local = Principal::Local { operator: OPERATOR };
    let now = Instant::now();
    for (id, target) in [
        (
            "sqlite",
            VerifiedTarget::Sqlite {
                target_ref: "db",
                binding_id: "fixed",
                access: DatabaseAccess::Read,
            },
        ),
        (
            "pg",
            VerifiedTarget::Postgres {
                target_ref: "db",
                binding_id: "fixed",
                access: DatabaseAccess::Write,
            },
        ),
        (
            "git",
            VerifiedTarget::Git {
                repository_ref: "repo",
                operation: GitOperation::ReadDiff,
            },
        ),
        (
            "exec",
            VerifiedTarget::Executable {
                executable_ref: "program",
                cwd_ref: "work",
                binding_id: "fixed",
            },
        ),
        (
            "custom",
            VerifiedTarget::CustomRust {
                source: "src/handler.rs",
                entry: "invoke",
            },
        ),
    ] {
        assert!(
            policy.evaluate(&context(&local, id, &target, now)).is_ok(),
            "{id}"
        );
    }
    for (id, target, reason) in [
        (
            "sqlite",
            VerifiedTarget::Sqlite {
                target_ref: "db",
                binding_id: "fixed",
                access: DatabaseAccess::Write,
            },
            DenyReason::GrantRequired,
        ),
        (
            "sqlite",
            VerifiedTarget::Postgres {
                target_ref: "db",
                binding_id: "fixed",
                access: DatabaseAccess::Read,
            },
            DenyReason::TargetDenied,
        ),
        (
            "pg",
            VerifiedTarget::Postgres {
                target_ref: "db",
                binding_id: "other",
                access: DatabaseAccess::Write,
            },
            DenyReason::TargetDenied,
        ),
        (
            "git",
            VerifiedTarget::Git {
                repository_ref: "repo",
                operation: GitOperation::ShowCommit,
            },
            DenyReason::TargetDenied,
        ),
        (
            "exec",
            VerifiedTarget::Executable {
                executable_ref: "program",
                cwd_ref: "elsewhere",
                binding_id: "fixed",
            },
            DenyReason::TargetDenied,
        ),
        (
            "custom",
            VerifiedTarget::CustomRust {
                source: "src/handler.rs",
                entry: "other",
            },
            DenyReason::TargetDenied,
        ),
    ] {
        assert_eq!(
            policy
                .evaluate(&context(&local, id, &target, now))
                .unwrap_err(),
            reason,
            "{id}"
        );
    }
}

fn bounds(timeout_ms: u64, output_bytes: u64) -> LimitOverrides {
    LimitOverrides {
        timeout_ms: NonZeroU64::new(timeout_ms),
        max_output_bytes: NonZeroU64::new(output_bytes),
    }
}

#[test]
fn overlapping_permissions_tighten_limits_and_rechecks_keep_admission_time() {
    let mut requests = catalog();
    let mut narrower = requests.grants[0].clone();
    narrower.id = "narrow".into();
    narrower.limits = Some(bounds(300, 600));
    requests.grants[0].limits = Some(bounds(500, 400));
    requests.grants.push(narrower);
    let mut operator = approval(&requests);
    operator.approved_grants[0].grant.limits = Some(bounds(400, 350));
    operator.limits = bounds(700, 450);
    operator.caller_grants[0].limits = bounds(650, 320);
    let policy = Policy::new(&requests, &operator).unwrap();
    let origin = ORIGIN.parse().unwrap();
    let target = rest(&origin, "/items/42", PUBLIC_PEER);
    let local = Principal::Local { operator: OPERATOR };
    let now = Instant::now();
    let mut context = context(&local, CATALOG, &target, now);
    context.requested_limits = bounds(900, 256);
    context.capability_timeout_ms = NonZeroU64::new(120_000);
    let permit = policy.evaluate(&context).unwrap();
    assert_eq!(permit.grant_id(), Some(CATALOG_GRANT));
    assert_eq!(permit.limits().timeout(), Duration::from_millis(300));
    assert_eq!(permit.limits().max_output_bytes(), 256);
    assert_eq!(permit.deadline(), now + Duration::from_millis(300));
    context.ceiling = Some(permit.ceiling());
    context.now = now + Duration::from_millis(299);
    assert_eq!(
        policy.evaluate(&context).unwrap().deadline(),
        permit.deadline()
    );
    let mut updated_operator = operator.clone();
    updated_operator.approved_grants.pop();
    context.requested_limits = LimitOverrides::default();
    let updated_policy = Policy::new(&requests, &updated_operator).unwrap();
    let recheck = updated_policy.evaluate(&context).unwrap();
    assert_eq!(recheck.ceiling(), permit.ceiling());
    updated_operator.approved_grants.clear();
    assert_eq!(
        Policy::new(&requests, &updated_operator)
            .unwrap()
            .evaluate(&context)
            .unwrap_err(),
        DenyReason::GrantRequired
    );
    context.now = permit.deadline();
    assert_eq!(
        policy.evaluate(&context).unwrap_err(),
        DenyReason::DeadlineExceeded
    );
}

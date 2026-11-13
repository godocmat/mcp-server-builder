use std::num::NonZeroU64;
use std::time::Duration;

use mcp_policy::{
    BufferedText, ByteLimitExceeded, Effect, ExecutionLimits, LimitOverrides, REDACTED,
    RedactionError, Redactor,
};
use serde_json::json;

#[test]
fn structured_evidence_omits_credentials_and_masks_paths_and_known_values() {
    let canary = "synthetic-known-7fe3";
    let redactor = Redactor::new([canary], ["/body/a~1b/1/~0key", "/diff/actual"]).unwrap();
    let evidence = json!({
        "headers": {"AUTHORIZATION": "opaque-header", "Set-Cookie": "opaque-cookie", "content-type": "application/json"},
        "headerList": [{"name": "X-API-Key", "value": "opaque-key"}, ["Proxy-Authorization", "opaque-proxy"]],
        "oauthCallback": {"code": "opaque-oauth", "state": "opaque-state"},
        "callbackBody": {"code": "opaque-oauth", "state": "opaque-state"},
        "mcp-session-id": "opaque-session",
        "body": {"password": "opaque-password", "a/b": ["visible", {"~key": "opaque-path"}]},
        "diff": {"actual": {"nested": "opaque-assertion"}, "expected": "keep"},
        "diagnostic": {"message": format!("upstream echoed {canary}"), "details": { canary: true }}
    });
    let result = redactor
        .redact_json(evidence, Redactor::DEFAULT_ENTRY_BYTES)
        .unwrap();
    assert_eq!(
        result,
        json!({
            "headers": {"content-type": "application/json"},
            "headerList": [null, null],
            "callbackBody": null,
            "body": {"password": REDACTED, "a/b": ["visible", {"~key": REDACTED}]},
            "diff": {"actual": REDACTED, "expected": "keep"},
            "diagnostic": {"message": format!("upstream echoed {REDACTED}"), "details": {}}
        })
    );
    let emitted = serde_json::to_string(&result).unwrap();
    assert!(!emitted.contains(canary));
    assert!(!emitted.contains("opaque-"));
}

#[test]
fn stderr_redaction_checks_every_chunk_boundary_before_display() {
    let canary = "synthetic-β🔑-canary";
    let redactor = Redactor::new([canary], [] as [&str; 0]).unwrap();
    let raw = format!("upstream echoed {canary} during a call\n");
    for split in 0..=raw.len() {
        let mut capture = BufferedText::new(128);
        capture.push(&raw.as_bytes()[..split]);
        capture.push(&raw.as_bytes()[split..]);
        let result = capture.finish(&redactor, 128);
        assert_eq!(
            result.text,
            format!("upstream echoed {REDACTED} during a call\n")
        );
        assert!(!result.truncated);
    }

    let mut capture = BufferedText::new(9);
    capture.push(&canary.as_bytes()[..9]);
    capture.push(&canary.as_bytes()[9..]);
    capture.push(b"further stderr");
    let result = capture.finish(&redactor, 64);
    assert!(result.text.is_empty());
    assert!(result.truncated);
}

#[test]
fn free_text_omits_credential_records_and_masks_before_utf8_truncation() {
    let redactor = Redactor::new(["secret-canary"], [] as [&str; 0]).unwrap();
    for raw in [
        "Upstream: Authorization: Basic\nopaque-header",
        "Cookie: session=opaque-cookie",
        "callback https://auth.example.test/?code=opaque-code&state=opaque-state",
        "callback https://auth.example.test/?%63ode=opaque-code",
        "failed to connect to https://local-user:opaque-password@upstream.example.test/",
        r#"failed with {"access_token":"opaque-token"}"#,
        "-----BEGIN RSA PRIVATE KEY-----\nopaque-material",
    ] {
        assert_eq!(redactor.redact_text(raw, 1024).text, REDACTED);
    }
    let result = redactor.redact_text("café secret-canary", 4);
    assert_eq!(result.text, "caf");
    assert!(result.truncated);
    let result = redactor.redact_text("start secret-canary", 12);
    assert_eq!(result.text, "start [REDAC");
    assert!(result.truncated);
}

#[test]
fn redaction_handles_overlaps_and_rejects_malformed_or_over_budget_evidence() {
    let redactor = Redactor::new(["ab-cd", "cd-ef", "ab"], [] as [&str; 0]).unwrap();
    assert_eq!(redactor.redact_text("ab-cd-ef", 128).text, REDACTED);
    let redactor = Redactor::new(["[REDACTED]", "canary"], [] as [&str; 0]).unwrap();
    assert!(redactor.redact_text("canary", 128).text.is_empty());
    let redactor = Redactor::new(["[1,2]"], [] as [&str; 0]).unwrap();
    assert_eq!(
        redactor.redact_json(json!([1, 2]), 128),
        Err(RedactionError::SensitiveEncoding)
    );
    assert_eq!(
        Redactor::new([] as [&str; 0], ["/bad~2path"]).err(),
        Some(RedactionError::InvalidPointer)
    );
    assert_eq!(
        Redactor::default().redact_json(json!("a\"b"), 5),
        Err(RedactionError::ByteLimitExceeded)
    );
    assert_eq!(
        Redactor::default().redact_json(json!("a\"b"), 6).unwrap(),
        json!("a\"b")
    );
    let redactor = Redactor::new([] as [&str; 0], [""]).unwrap();
    assert_eq!(
        redactor.redact_json(json!({"any": "opaque"}), 128).unwrap(),
        REDACTED
    );
}

#[test]
fn limits_enforce_hard_caps_and_cumulative_byte_accounting() {
    assert_eq!(
        ExecutionLimits::default().timeout(),
        Duration::from_secs(10)
    );
    let hard_cap = ExecutionLimits::for_capability(NonZeroU64::new(u64::MAX));
    assert_eq!(hard_cap.timeout(), Duration::from_secs(30));
    assert_eq!(hard_cap.max_output_bytes(), 1024 * 1024);
    let narrowed = hard_cap.tighten(LimitOverrides {
        timeout_ms: NonZeroU64::new(300),
        max_output_bytes: NonZeroU64::new(4),
    });
    assert_eq!(narrowed.intersect(hard_cap), narrowed);
    let mut budget = narrowed.output_budget();
    budget.consume(3).unwrap();
    assert_eq!(budget.consume(2), Err(ByteLimitExceeded));
    assert_eq!(budget.remaining(), 1);
    budget.consume(1).unwrap();
    assert_eq!(budget.consume(1), Err(ByteLimitExceeded));
    assert_eq!(budget.tighten(3), Err(ByteLimitExceeded));
    assert_eq!(budget.remaining(), 0);
    assert_eq!(budget.tighten(10), Err(ByteLimitExceeded));

    let mut budget = narrowed.output_budget();
    budget.consume(2).unwrap();
    budget.tighten(3).unwrap();
    assert_eq!(budget.remaining(), 1);
    let mut input = ExecutionLimits::input_budget();
    input.consume(1024 * 1024).unwrap();
    assert_eq!(input.consume(1), Err(ByteLimitExceeded));
    assert_eq!(serde_json::to_value(Effect::Unknown).unwrap(), "unknown");
}

use builder_project::{
    IssueCode, MAX_MANIFEST_BYTES, ManifestError, ManifestRevision, SchemaVersion, ValidationIssue,
    is_portable_path, parse_manifest, validate_document,
};
use serde_json::{Value, json};

const FIXTURE: &[u8] = include_bytes!("fixtures/catalog-v1.json");

fn sample() -> Value {
    serde_json::from_slice(FIXTURE).unwrap()
}

fn issues(error: ManifestError) -> Vec<ValidationIssue> {
    match error {
        ManifestError::Invalid(issues) => issues,
        other => panic!("expected validation issues, got {other:?}"),
    }
}

fn check_field(mut input: Value, field: &str, replacement: Value, code: IssueCode) {
    *input.pointer_mut(field).unwrap() = replacement;
    let failures = issues(parse_manifest(&serde_json::to_vec(&input).unwrap()).unwrap_err());
    assert!(
        failures
            .iter()
            .any(|issue| issue.field == field && issue.code == code),
        "expected {code:?} at {field}, got {failures:?}",
    );
}

#[test]
fn round_trips_the_pilot_fixture_and_hashes_exact_bytes() {
    let manifest = parse_manifest(FIXTURE).unwrap();
    assert_eq!(manifest.bytes(), FIXTURE);
    assert_eq!(manifest.revision(), ManifestRevision::of(FIXTURE));
    assert_eq!(serde_json::to_value(manifest.document()).unwrap(), sample());
    let canonical = validate_document(manifest.document().clone()).unwrap();
    assert_ne!(canonical.revision(), manifest.revision());
    assert_eq!(canonical.document(), manifest.document());
    let fixed: ManifestRevision =
        "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
            .parse()
            .unwrap();
    assert_eq!(ManifestRevision::of(b"abc"), fixed);
    assert_eq!(
        fixed.to_string(),
        serde_json::to_value(fixed).unwrap().as_str().unwrap()
    );
    assert!(
        fixed
            .to_string()
            .to_uppercase()
            .parse::<ManifestRevision>()
            .is_err()
    );
}

#[test]
fn canonical_policy_origin_preserves_the_exact_opened_manifest() {
    let mut input = sample();
    input["policy"]["grants"][0]["target"]["origin"] = json!("https://CATALOG.example.test:443/");
    let bytes = serde_json::to_vec(&input).unwrap();
    let manifest = parse_manifest(&bytes).unwrap();
    assert_eq!(manifest.bytes(), bytes);
    assert_eq!(manifest.revision(), ManifestRevision::of(&bytes));

    let saved = validate_document(manifest.into_document()).unwrap();
    assert_ne!(saved.revision(), ManifestRevision::of(&bytes));
    assert_eq!(serde_json::to_value(saved.document()).unwrap(), sample());
}

#[test]
fn dispatches_versions_before_decoding_fields_and_distinguishes_malformed_headers() {
    for (value, version) in [
        (json!(0), SchemaVersion::Signed(0)),
        (json!(-1), SchemaVersion::Signed(-1)),
        (json!(2), SchemaVersion::Signed(2)),
        (json!(u64::MAX), SchemaVersion::Unsigned(u64::MAX)),
    ] {
        let bytes = serde_json::to_vec(&json!({
            "format": "mcp-server-builder.project",
            "schemaVersion": value,
            "transport": {"kind": "stdio"},
            "futureField": true,
        }))
        .unwrap();
        assert_eq!(
            parse_manifest(&bytes).unwrap_err(),
            ManifestError::UnsupportedSchema { version }
        );
    }
    for value in [json!("1"), json!(1.0), json!(null), json!({})] {
        check_field(sample(), "/schemaVersion", value, IssueCode::Field);
    }
    check_field(sample(), "/format", json!("other-format"), IssueCode::Field);
    check_field(
        sample(),
        "/transport",
        json!("streamableHttp"),
        IssueCode::UnsupportedSetting,
    );
}

#[test]
fn rejects_duplicate_keys_even_in_arbitrary_nested_json() {
    let input = std::str::from_utf8(FIXTURE).unwrap().replace(
        r#""id": "42", "name": "Sample item""#,
        r#""id": "42", "a/b~c": 1, "a\/b~c": 2, "name": "Sample item""#,
    );
    let issues = issues(parse_manifest(input.as_bytes()).unwrap_err());
    assert_eq!(issues[0].code, IssueCode::DuplicateKey);
    assert_eq!(issues[0].field, "/scenarios/0/fixtures/0/body/a~1b~0c");
}

#[test]
fn rejects_bad_shapes_and_unknown_fields_with_safe_pointers() {
    check_field(
        sample(),
        "/server",
        json!(["catalog", "0.1.0"]),
        IssueCode::Field,
    );
    check_field(
        sample(),
        "/capabilities/0/timeoutMs",
        json!(null),
        IssueCode::Field,
    );
    check_field(
        sample(),
        "/capabilities/0/kind",
        json!("plugin"),
        IssueCode::UnsupportedSetting,
    );
    check_field(
        sample(),
        "/scenarios/0/call/arguments",
        json!([]),
        IssueCode::Field,
    );

    let mut input = sample();
    input["bindings"][0]["headers"] = json!({"Authorization": "SYNTHETIC_CANARY_VALUE"});
    let failures = issues(parse_manifest(&serde_json::to_vec(&input).unwrap()).unwrap_err());
    assert_eq!(failures[0].field, "/bindings/0/headers");
    assert!(!format!("{failures:?}").contains("SYNTHETIC_CANARY_VALUE"));

    let mut input = sample();
    input["capabilities"][0]
        .as_object_mut()
        .unwrap()
        .remove("bindingId");
    let failures = issues(parse_manifest(&serde_json::to_vec(&input).unwrap()).unwrap_err());
    assert_eq!(failures[0].field, "/capabilities/0/bindingId");
    assert_eq!(failures[0].capability_id.as_deref(), Some("lookup_item"));
}

#[test]
fn reports_policy_fields_and_rejects_settings_outside_schema_one() {
    for (field, value, code) in [
        (
            "/policy/grants/0/operation",
            json!("db.read"),
            IssueCode::UnsupportedSetting,
        ),
        (
            "/policy/grants/0/target/kind",
            json!("sqlite"),
            IssueCode::UnsupportedSetting,
        ),
        (
            "/policy/grants/0/target/origin",
            json!("https://synthetic@example.test"),
            IssueCode::Field,
        ),
        (
            "/policy/grants/0/target/method",
            json!("get"),
            IssueCode::Field,
        ),
        (
            "/policy/grants/0/limits/timeoutMs",
            json!(0),
            IssueCode::Limit,
        ),
        (
            "/policy/grants/0/limits/maxOutputBytes",
            json!(null),
            IssueCode::Field,
        ),
        (
            "/policy/grants/0/limits/maxOutputBytes",
            json!(1_048_577),
            IssueCode::Limit,
        ),
    ] {
        check_field(sample(), field, value, code);
    }

    for invalid in [json!(["10.0.0.0/33"]), json!(["10.0.0.1/24"])] {
        let mut input = sample();
        input["policy"]["grants"][0]["target"]["allowedPeerRanges"] = invalid;
        let errors = issues(parse_manifest(&serde_json::to_vec(&input).unwrap()).unwrap_err());
        assert_eq!(errors[0].field, "/policy/grants/0/target/allowedPeerRanges");
        assert_eq!(errors[0].capability_id.as_deref(), Some("lookup_item"));
        assert_eq!(errors[0].code, IssueCode::Field);
    }

    let mut input = sample();
    input["policy"]["grants"][0]["target"]["unexpected"] = json!(true);
    let errors = issues(parse_manifest(&serde_json::to_vec(&input).unwrap()).unwrap_err());
    assert_eq!(errors[0].field, "/policy/grants/0/target/unexpected");
    assert_eq!(errors[0].code, IssueCode::Field);

    let mut input = sample();
    input["policy"]["grants"][1]["id"] = input["policy"]["grants"][0]["id"].clone();
    let errors = issues(parse_manifest(&serde_json::to_vec(&input).unwrap()).unwrap_err());
    assert_eq!(errors[0].field, "/policy/grants/1/id");
    assert_eq!(errors[0].code, IssueCode::DuplicateId);
}

#[test]
fn validates_schemas_offline_without_confusing_payload_data_with_refs() {
    for invalid in [
        json!({"type": "array"}),
        json!({"type": "object", "required": "id"}),
        json!({"type": "object", "$ref": "#/$defs/absent"}),
        json!({"type": "object", "$ref": "https://127.0.0.1/schema"}),
        json!({"type": "object", "$dynamicRef": "file:///tmp/secret.json"}),
        json!({"type": "object", "$schema": "http://json-schema.org/draft-07/schema#"}),
        json!({"type": "object", "properties": {"id": {"type": "string", "x-mcp-header": "X-Item"}}}),
    ] {
        check_field(
            sample(),
            "/capabilities/0/inputSchema",
            invalid,
            IssueCode::Schema,
        );
    }
    let mut input = sample();
    input["capabilities"][0]["inputSchema"]["examples"] = json!([{"$ref": "file:///fixture-data-not-a-schema", "x-mcp-header": "ordinary example data"}]);
    assert!(parse_manifest(&serde_json::to_vec(&input).unwrap()).is_ok());
}

#[test]
fn validates_references_limits_templates_and_scenario_fields_in_stable_order() {
    let mut input = sample();
    input["server"]["version"] = json!("latest");
    input["bindings"][0]["headersFromSecrets"]["Authorization"] = json!("missing");
    input["capabilities"][0]["timeoutMs"] = json!(30_001);
    input["capabilities"][0]["bindingId"] = json!("guide_file");
    input["capabilities"][3]["messages"][0]["textTemplate"] = json!("{undeclared}");
    input["policy"]["grants"][1]["target"]["rootRef"] = json!("other_root");
    input["scenarios"][0]["assertions"][0]["path"] = json!("/bad~3");
    let actual = issues(parse_manifest(&serde_json::to_vec(&input).unwrap()).unwrap_err());
    let fields: Vec<_> = actual.iter().map(|issue| issue.field.as_str()).collect();
    assert_eq!(
        fields,
        [
            "/server/version",
            "/bindings/0/headersFromSecrets",
            "/capabilities/0/timeoutMs",
            "/capabilities/0/bindingId",
            "/capabilities/3/messages/0/textTemplate",
            "/policy/grants/0/target",
            "/policy/grants/1/target/rootRef",
            "/scenarios/0/fixtures/0/bindingId",
            "/scenarios/0/assertions/0/path",
        ]
    );
    check_field(
        sample(),
        "/policy/grants/0/operation",
        json!("http.write"),
        IssueCode::Reference,
    );
    check_field(
        sample(),
        "/policy/grants/1/target/relativePathTemplate",
        json!("{folder}/{file}"),
        IssueCode::Reference,
    );
    check_field(
        sample(),
        "/scenarios/0/version",
        json!(2),
        IssueCode::UnsupportedSetting,
    );
}

#[test]
fn rejects_ambiguous_templates_and_unsafe_paths() {
    let mut input = sample();
    input["capabilities"][1]["uri"] = json!("CATALOG://DOCS/items/%34%32");
    let failures = issues(parse_manifest(&serde_json::to_vec(&input).unwrap()).unwrap_err());
    assert!(
        failures
            .iter()
            .any(|issue| issue.field == "/capabilities/2/uriTemplate"
                && issue.code == IssueCode::AmbiguousResource)
    );
    for path in [
        "../notes",
        "notes//item",
        "NUL.txt",
        "COM¹.txt",
        "notes/aux.txt",
        "notes/a:stream",
        "/etc/passwd",
        "notes/.",
        "C:\\notes",
        "notes/name. ",
        "notes/{name}",
    ] {
        assert!(!is_portable_path(path), "{path}");
    }
    assert!(is_portable_path("notes/café.txt"));
    for path in [
        "/items/%2e%2e/{id}",
        "/items/%252f/{id}",
        "/items//{id}",
        "/items/{id}?query",
    ] {
        check_field(
            sample(),
            "/bindings/0/pathTemplate",
            json!(path),
            IssueCode::Field,
        );
    }
    check_field(
        sample(),
        "/bindings/0/origin",
        json!("https://canary@example.test"),
        IssueCode::Field,
    );
}

#[test]
fn bounds_bytes_nesting_and_invalid_utf8() {
    for (bytes, code) in [
        (vec![b' '; MAX_MANIFEST_BYTES + 1], IssueCode::Limit),
        (
            format!("{}0{}", "[".repeat(70), "]".repeat(70)).into_bytes(),
            IssueCode::Limit,
        ),
        (b"{\xff}".to_vec(), IssueCode::Json),
        (b"{".to_vec(), IssueCode::Json),
        (b"{} {}".to_vec(), IssueCode::Json),
    ] {
        assert_eq!(issues(parse_manifest(&bytes).unwrap_err())[0].code, code);
    }
}

use std::fmt;

use ipnet::IpNet;
use serde::Serialize;
use serde::de::{self, DeserializeSeed, Error, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};
use serde_path_to_error::Segment;

use crate::{
    IssueCode, MAX_MANIFEST_BYTES, ManifestRevision, PROJECT_FORMAT, PROJECT_SCHEMA_VERSION,
    ProjectDocument, ValidationIssue, syntax, validation,
};

const MAX_JSON_DEPTH: usize = 64;
const MAX_JSON_NODES: usize = 100_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ManifestError {
    Invalid(Vec<ValidationIssue>),
    UnsupportedSchema { version: SchemaVersion },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum SchemaVersion {
    Signed(i64),
    Unsigned(u64),
}

impl ManifestError {
    fn invalid(issue: ValidationIssue) -> Self {
        Self::Invalid(vec![issue])
    }
}

/// A validated schema 1 document paired with the exact bytes it came from.
#[derive(Debug)]
pub struct ValidatedManifest {
    bytes: Vec<u8>,
    document: ProjectDocument,
    revision: ManifestRevision,
}

impl ValidatedManifest {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn document(&self) -> &ProjectDocument {
        &self.document
    }

    pub fn into_document(self) -> ProjectDocument {
        self.document
    }

    pub fn revision(&self) -> ManifestRevision {
        self.revision
    }

    fn new(bytes: Vec<u8>, document: ProjectDocument) -> Self {
        let revision = ManifestRevision::of(&bytes);
        Self {
            bytes,
            document,
            revision,
        }
    }
}

pub fn parse_manifest(bytes: &[u8]) -> Result<ValidatedManifest, ManifestError> {
    Ok(ValidatedManifest::new(bytes.to_vec(), decode(bytes)?))
}

/// Validate typed edits using the same size, depth and semantic checks as an on-disk document.
pub fn validate_document(document: ProjectDocument) -> Result<ValidatedManifest, ManifestError> {
    let mut encoded = BoundedJson(Vec::new());
    serde_json::to_writer_pretty(&mut encoded, &document)
        .map_err(|_| ManifestError::invalid(size_issue()))?;
    encoded
        .extend(b"\n")
        .map_err(|_| ManifestError::invalid(size_issue()))?;
    let decoded = decode(&encoded.0)?;
    Ok(ValidatedManifest::new(encoded.0, decoded))
}

struct BoundedJson(Vec<u8>);

impl BoundedJson {
    fn extend(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        if self.0.len() + bytes.len() > MAX_MANIFEST_BYTES {
            return Err(std::io::Error::other("Manifest exceeds its byte limit."));
        }
        self.0.extend_from_slice(bytes);
        Ok(())
    }
}

impl std::io::Write for BoundedJson {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.extend(bytes)?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(crate) fn size_issue() -> ValidationIssue {
    ValidationIssue::new(IssueCode::Limit, "", "The manifest exceeds 4 MiB.")
}

fn decode(bytes: &[u8]) -> Result<ProjectDocument, ManifestError> {
    let value = parse_json(bytes)?;
    let object = value.as_object().ok_or_else(|| {
        ManifestError::invalid(ValidationIssue::new(
            IssueCode::Field,
            "",
            "A project manifest must be a JSON object.",
        ))
    })?;
    if object.get("format").and_then(Value::as_str) != Some(PROJECT_FORMAT) {
        return Err(ManifestError::invalid(ValidationIssue::new(
            IssueCode::Field,
            "/format",
            "Expected the mcp-server-builder.project format.",
        )));
    }

    // Inspect the header before selecting a version-specific deserializer.
    match object.get("schemaVersion") {
        Some(version) if version.as_u64() == Some(u64::from(PROJECT_SCHEMA_VERSION)) => {}
        version => {
            let integer = version.and_then(|version| {
                version
                    .as_i64()
                    .map(SchemaVersion::Signed)
                    .or_else(|| version.as_u64().map(SchemaVersion::Unsigned))
            });
            return Err(integer.map_or_else(
                || {
                    ManifestError::invalid(ValidationIssue::new(
                        IssueCode::Field,
                        "/schemaVersion",
                        "Expected an integer schema version.",
                    ))
                },
                |version| ManifestError::UnsupportedSchema { version },
            ));
        }
    }

    check_record_shapes(&value)?;
    check_tagged_fields(&value)?;
    let document = serde_path_to_error::deserialize::<_, ProjectDocument>(&value)
        .map_err(|error| ManifestError::invalid(deserialize_issue(&value, error)))?;
    let issues = validation::validate(&document);
    if !issues.is_empty() {
        return Err(ManifestError::Invalid(issues));
    }
    Ok(document)
}

fn check_tagged_fields(root: &Value) -> Result<(), ManifestError> {
    for (collection, discriminator, variants) in [
        (
            "/capabilities",
            "kind",
            &[
                ("tool", TOOL_FIELDS),
                ("resource", RESOURCE_FIELDS),
                ("resourceTemplate", RESOURCE_TEMPLATE_FIELDS),
                ("prompt", PROMPT_FIELDS),
            ][..],
        ),
        (
            "/bindings",
            "kind",
            &[("rest", REST_FIELDS), ("fileRead", FILE_FIELDS)][..],
        ),
    ] {
        check_tagged_array(root, collection, discriminator, variants)?;
    }
    check_policy_fields(root)?;
    if let Some(scenarios) = root.get("scenarios").and_then(Value::as_array) {
        for (index, scenario) in scenarios.iter().enumerate() {
            let base = format!("/scenarios/{index}");
            check_tagged_record(
                root,
                scenario.get("call"),
                &syntax::field(&base, "call"),
                "method",
                &[
                    ("tools/call", TOOL_CALL_FIELDS),
                    ("resources/read", RESOURCE_CALL_FIELDS),
                    ("prompts/get", PROMPT_CALL_FIELDS),
                ],
            )?;
            check_tagged_array(
                root,
                &syntax::field(&base, "fixtures"),
                "kind",
                &[
                    ("rest", REST_FIXTURE_FIELDS),
                    ("fileRead", FILE_FIXTURE_FIELDS),
                ],
            )?;
            check_tagged_array(
                root,
                &syntax::field(&base, "assertions"),
                "kind",
                &[
                    ("equals", EQUALS_FIELDS),
                    ("schema", SCHEMA_FIELDS),
                    ("errorCode", ERROR_FIELDS),
                ],
            )?;
        }
    }
    Ok(())
}

// Serde buffers internally tagged records before selecting the variant. Check
// those fields against the selected shape so type errors retain their pointer.
type FieldCheck = fn(&Value) -> bool;
type FieldRule = (&'static str, FieldCheck);
type VariantRule = (&'static str, &'static [FieldRule]);

fn check_policy_fields(root: &Value) -> Result<(), ManifestError> {
    let Some(policy) = root.get("policy") else {
        return Ok(());
    };
    check_fields(root, policy, "/policy", &[("grants", Value::is_array)], &[])?;
    let grants = policy["grants"].as_array().expect("checked grants array");
    for (index, grant) in grants.iter().enumerate() {
        let pointer = format!("/policy/grants/{index}");
        check_fields(
            root,
            grant,
            &pointer,
            &[
                ("id", Value::is_string),
                ("capabilityId", Value::is_string),
                ("operation", Value::is_string),
                ("target", Value::is_object),
            ],
            &[("limits", Value::is_object)],
        )?;
        if !matches!(
            grant["operation"].as_str(),
            Some("http.read" | "http.write" | "file.read")
        ) {
            return Err(field_issue(
                root,
                syntax::field(&pointer, "operation"),
                IssueCode::UnsupportedSetting,
                "This setting is unsupported in schema 1.",
            ));
        }

        let target = &grant["target"];
        let target_pointer = syntax::field(&pointer, "target");
        let (required, optional): (&[FieldRule], &[FieldRule]) = match target["kind"].as_str() {
            Some("rest") => (REST_TARGET_FIELDS, &[("allowedPeerRanges", cidr_list)]),
            Some("fileRead") => (
                FILE_TARGET_FIELDS,
                &[("relativePathTemplate", Value::is_string)],
            ),
            Some(_) => {
                return Err(field_issue(
                    root,
                    syntax::field(&target_pointer, "kind"),
                    IssueCode::UnsupportedSetting,
                    "This setting is unsupported in schema 1.",
                ));
            }
            None => return Err(invalid_field(root, syntax::field(&target_pointer, "kind"))),
        };
        check_fields(root, target, &target_pointer, required, optional)?;

        if let Some(limits) = grant.get("limits") {
            let limits_pointer = syntax::field(&pointer, "limits");
            check_fields(
                root,
                limits,
                &limits_pointer,
                &[],
                &[
                    ("timeoutMs", Value::is_u64),
                    ("maxOutputBytes", Value::is_u64),
                ],
            )?;
            for key in ["timeoutMs", "maxOutputBytes"] {
                if limits[key].as_u64() == Some(0) {
                    return Err(field_issue(
                        root,
                        syntax::field(&limits_pointer, key),
                        IssueCode::Limit,
                        "Use a positive limit override.",
                    ));
                }
            }
        }
    }
    Ok(())
}

// M2 deliberately sanitizes Serde errors. Check the pilot shape first so the
// editor can still locate missing, malformed or unexpected policy fields.
fn check_fields(
    root: &Value,
    record: &Value,
    pointer: &str,
    required: &[FieldRule],
    optional: &[FieldRule],
) -> Result<(), ManifestError> {
    for (key, accepts) in required {
        if !record.get(key).is_some_and(accepts) {
            return Err(invalid_field(root, syntax::field(pointer, key)));
        }
    }
    for (key, accepts) in optional {
        if record.get(key).is_some_and(|value| !accepts(value)) {
            return Err(invalid_field(root, syntax::field(pointer, key)));
        }
    }
    if let Some(key) = record.as_object().and_then(|fields| {
        fields.keys().find(|key| {
            !required
                .iter()
                .chain(optional)
                .any(|(field, _)| *field == key.as_str())
        })
    }) {
        let field = if safe_key(key) {
            syntax::field(pointer, key)
        } else {
            pointer.into()
        };
        return Err(invalid_field(root, field));
    }
    Ok(())
}

fn check_tagged_array(
    root: &Value,
    pointer: &str,
    tag: &str,
    variants: &[VariantRule],
) -> Result<(), ManifestError> {
    if let Some(records) = root.pointer(pointer).and_then(Value::as_array) {
        for (index, record) in records.iter().enumerate() {
            check_tagged_record(
                root,
                Some(record),
                &format!("{pointer}/{index}"),
                tag,
                variants,
            )?;
        }
    }
    Ok(())
}

fn check_tagged_record(
    root: &Value,
    record: Option<&Value>,
    pointer: &str,
    tag: &str,
    variants: &[VariantRule],
) -> Result<(), ManifestError> {
    let Some(record) = record else {
        return Ok(());
    };
    let Some(discriminator) = record.get(tag).and_then(Value::as_str) else {
        return Err(field_issue(
            root,
            syntax::field(pointer, tag),
            IssueCode::Field,
            "Provide the record's string discriminator.",
        ));
    };
    let Some((_, fields)) = variants.iter().find(|(value, _)| *value == discriminator) else {
        return Err(field_issue(
            root,
            syntax::field(pointer, tag),
            IssueCode::UnsupportedSetting,
            "This setting is unsupported in schema 1.",
        ));
    };
    for (key, accepts) in *fields {
        if record.get(key).is_some_and(|value| !accepts(value)) {
            return Err(invalid_field(root, syntax::field(pointer, key)));
        }
    }
    Ok(())
}

fn invalid_field(root: &Value, pointer: String) -> ManifestError {
    field_issue(
        root,
        pointer,
        IssueCode::Field,
        "The field is missing, unexpected or has an invalid type or value.",
    )
}

fn field_issue(
    root: &Value,
    pointer: String,
    code: IssueCode,
    message: &'static str,
) -> ManifestError {
    let mut issue = ValidationIssue::new(code, pointer, message);
    attach_capability(&mut issue, root);
    ManifestError::invalid(issue)
}

const TOOL_FIELDS: &[FieldRule] = &[
    ("id", Value::is_string),
    ("name", Value::is_string),
    ("description", Value::is_string),
    ("bindingId", Value::is_string),
    ("timeoutMs", Value::is_u64),
];
const RESOURCE_FIELDS: &[FieldRule] = &[
    ("id", Value::is_string),
    ("name", Value::is_string),
    ("description", Value::is_string),
    ("bindingId", Value::is_string),
    ("uri", Value::is_string),
    ("mimeType", Value::is_string),
    ("timeoutMs", Value::is_u64),
];
const RESOURCE_TEMPLATE_FIELDS: &[FieldRule] = &[
    ("id", Value::is_string),
    ("name", Value::is_string),
    ("description", Value::is_string),
    ("bindingId", Value::is_string),
    ("uriTemplate", Value::is_string),
    ("mimeType", Value::is_string),
    ("timeoutMs", Value::is_u64),
    ("variables", Value::is_object),
];
const PROMPT_FIELDS: &[FieldRule] = &[
    ("id", Value::is_string),
    ("name", Value::is_string),
    ("description", Value::is_string),
    ("arguments", |value| {
        record_items_match(
            value,
            &[
                ("name", Value::is_string),
                ("description", Value::is_string),
                ("required", Value::is_boolean),
            ],
        )
    }),
    ("messages", |value| {
        record_items_match(
            value,
            &[
                ("role", Value::is_string),
                ("textTemplate", Value::is_string),
            ],
        )
    }),
];
const REST_FIELDS: &[FieldRule] = &[
    ("id", Value::is_string),
    ("origin", Value::is_string),
    ("method", Value::is_string),
    ("pathTemplate", Value::is_string),
    ("pathParams", string_map),
    ("bodyPointer", Value::is_string),
    ("headersFromSecrets", string_map),
];
const FILE_FIELDS: &[FieldRule] = &[
    ("id", Value::is_string),
    ("rootRef", Value::is_string),
    ("relativePathTemplate", Value::is_string),
];
const REST_TARGET_FIELDS: &[FieldRule] = &[
    ("kind", Value::is_string),
    ("origin", |value| {
        value.as_str().and_then(syntax::http_origin).is_some()
    }),
    ("method", |value| {
        value.as_str().and_then(syntax::http_method).is_some()
    }),
    ("pathTemplate", Value::is_string),
];
const FILE_TARGET_FIELDS: &[FieldRule] =
    &[("kind", Value::is_string), ("rootRef", Value::is_string)];
const TOOL_CALL_FIELDS: &[FieldRule] =
    &[("name", Value::is_string), ("arguments", Value::is_object)];
const RESOURCE_CALL_FIELDS: &[FieldRule] = &[("uri", Value::is_string)];
const PROMPT_CALL_FIELDS: &[FieldRule] = &[("name", Value::is_string), ("arguments", string_map)];
const REST_FIXTURE_FIELDS: &[FieldRule] = &[
    ("bindingId", Value::is_string),
    ("method", Value::is_string),
    ("path", Value::is_string),
    ("status", |value| {
        value
            .as_u64()
            .is_some_and(|number| u16::try_from(number).is_ok())
    }),
];
const FILE_FIXTURE_FIELDS: &[FieldRule] = &[
    ("bindingId", Value::is_string),
    ("relativePath", Value::is_string),
    ("text", Value::is_string),
];
const EQUALS_FIELDS: &[FieldRule] = &[("path", Value::is_string)];
const SCHEMA_FIELDS: &[FieldRule] = &[("path", Value::is_string)];
const ERROR_FIELDS: &[FieldRule] = &[("code", Value::is_string)];

fn cidr_list(value: &Value) -> bool {
    value.as_array().is_some_and(|items| {
        items.iter().all(|item| {
            item.as_str()
                .is_some_and(|range| range.len() <= 64 && range.parse::<IpNet>().is_ok())
        })
    })
}

fn string_map(value: &Value) -> bool {
    value
        .as_object()
        .is_some_and(|fields| fields.values().all(Value::is_string))
}

fn record_items_match(value: &Value, rules: &[FieldRule]) -> bool {
    value.as_array().is_some_and(|items| {
        items.iter().all(|item| {
            rules
                .iter()
                .all(|(key, accepts)| item.get(key).is_none_or(accepts))
        })
    })
}

fn check_record_shapes(value: &Value) -> Result<(), ManifestError> {
    // The struct representation is an object even when Serde would accept a
    // positional sequence. Leave schema and scenario payload Values unconstrained.
    for pointer in ["/server", "/policy"] {
        require_object(value, value.pointer(pointer), pointer)?;
    }
    for collection in [
        "/capabilities",
        "/bindings",
        "/policy/grants",
        "/secretRefs",
        "/scenarios",
    ] {
        let Some(records) = value.pointer(collection).and_then(Value::as_array) else {
            continue;
        };
        for (index, record) in records.iter().enumerate() {
            let pointer = format!("{collection}/{index}");
            require_object(value, Some(record), &pointer)?;
            let (objects, arrays): (&[&str], &[&str]) = match collection {
                "/capabilities" => (&["variables"], &["arguments", "messages"]),
                "/bindings" => (&["pathParams", "headersFromSecrets"], &[]),
                "/policy/grants" => (&["target", "limits"], &[]),
                "/scenarios" => (&["call"], &["fixtures", "assertions"]),
                _ => (&[], &[]),
            };
            for key in objects {
                let field = syntax::field(&pointer, key);
                require_object(value, record.get(*key), &field)?;
            }
            for key in arrays {
                let field = syntax::field(&pointer, key);
                if let Some(entries) = record.get(*key).and_then(Value::as_array) {
                    for (index, entry) in entries.iter().enumerate() {
                        require_object(value, Some(entry), &format!("{field}/{index}"))?;
                    }
                }
            }
        }
    }
    Ok(())
}

fn require_object(root: &Value, value: Option<&Value>, field: &str) -> Result<(), ManifestError> {
    if value.is_none_or(Value::is_object) {
        return Ok(());
    }
    let mut issue =
        ValidationIssue::new(IssueCode::Field, field, "Use an object with named fields.");
    attach_capability(&mut issue, root);
    Err(ManifestError::invalid(issue))
}

fn deserialize_issue(
    value: &Value,
    error: serde_path_to_error::Error<serde_json::Error>,
) -> ValidationIssue {
    let mut field = String::new();
    for segment in error.path() {
        match segment {
            Segment::Seq { index } => field.push_str(&format!("/{index}")),
            Segment::Map { key } if safe_key(key) => syntax::push_pointer(&mut field, key),
            _ => break,
        }
    }

    // Internally tagged enums report discriminator and missing-field errors at their record.
    // Use Serde's field names for location only and keep its raw error text private.
    let reason = error.inner().to_string();
    let quoted_field = ["unknown field `", "missing field `"]
        .iter()
        .find_map(|prefix| reason.strip_prefix(prefix)?.split('`').next());
    let discriminator = [("unknown variant ", "kind"), ("unknown variant ", "method")]
        .iter()
        .find_map(|(prefix, tag)| {
            (reason.starts_with(prefix) && value.pointer(&field)?.get(tag).is_some())
                .then_some(*tag)
        });
    if let Some(key) = quoted_field.or(discriminator).filter(|key| {
        safe_key(key)
            && (reason.starts_with("missing field ")
                || value
                    .pointer(&field)
                    .and_then(|record| record.get(key))
                    .is_some())
    }) {
        syntax::push_pointer(&mut field, key);
    }
    let (code, message) = if reason.starts_with("unknown variant ") {
        (
            IssueCode::UnsupportedSetting,
            "This setting is unsupported in schema 1.",
        )
    } else {
        (
            IssueCode::Field,
            "The field is missing, unexpected or has an invalid type or value.",
        )
    };
    let mut issue = ValidationIssue::new(code, field, message);
    attach_capability(&mut issue, value);
    issue
}

fn attach_capability(issue: &mut ValidationIssue, value: &Value) {
    let record = |base: &str| {
        let path = issue.field.strip_prefix(base)?.strip_prefix('/')?;
        let index = path.split('/').next()?.parse::<usize>().ok()?;
        value.pointer(base)?.get(index)
    };
    if let Some(id) = record("/capabilities")
        .and_then(|capability| capability.get("id"))
        .or_else(|| record("/policy/grants").and_then(|grant| grant.get("capabilityId")))
        .and_then(Value::as_str)
        .filter(|id| syntax::is_key(id))
    {
        issue.capability_id = Some(id.into());
    }
}

fn safe_key(key: &str) -> bool {
    key.len() <= 80 && !key.chars().any(char::is_control)
}

fn parse_json(bytes: &[u8]) -> Result<Value, ManifestError> {
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(ManifestError::invalid(size_issue()));
    }
    let input = std::str::from_utf8(bytes).map_err(|_| {
        ManifestError::invalid(ValidationIssue::new(
            IssueCode::Json,
            "",
            "The manifest must be UTF-8.",
        ))
    })?;
    let mut state = JsonState::default();
    let mut deserializer = serde_json::Deserializer::from_str(input);
    let value = JsonValue(&mut state)
        .deserialize(&mut deserializer)
        .and_then(|value| deserializer.end().map(|()| value))
        .map_err(|_| {
            ManifestError::invalid(state.issue.take().unwrap_or_else(|| {
                ValidationIssue::new(IssueCode::Json, "", "The manifest contains malformed JSON.")
            }))
        })?;
    Ok(value)
}

#[derive(Default)]
struct JsonState {
    path: String,
    depth: usize,
    nodes: usize,
    issue: Option<ValidationIssue>,
}

struct JsonValue<'a>(&'a mut JsonState);

impl<'de> DeserializeSeed<'de> for JsonValue<'_> {
    type Value = Value;

    fn deserialize<D: de::Deserializer<'de>>(self, input: D) -> Result<Value, D::Error> {
        let state = self.0;
        state.nodes += 1;
        if state.depth > MAX_JSON_DEPTH || state.nodes > MAX_JSON_NODES {
            state.issue = Some(ValidationIssue::new(
                IssueCode::Limit,
                state.path.clone(),
                "The manifest exceeds its JSON depth or item limit.",
            ));
            return Err(D::Error::custom("JSON limit"));
        }
        state.depth += 1;
        let result = input.deserialize_any(JsonValue(state));
        state.depth -= 1;
        result
    }
}

impl<'de> Visitor<'de> for JsonValue<'_> {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_unit<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Value, E> {
        Ok(value.into())
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Value, E> {
        Ok(value.into())
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Value, E> {
        Ok(value.into())
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Value, E> {
        Ok(value.into())
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Value, E> {
        Ok(value.into())
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<Value, E> {
        Ok(value.into())
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Value, A::Error> {
        let mut values = Vec::new();
        let state = self.0;
        let parent = state.path.len();
        loop {
            state.path.push_str(&format!("/{}", values.len()));
            let next = sequence.next_element_seed(JsonValue(state));
            state.path.truncate(parent);
            match next? {
                Some(value) => values.push(value),
                None => return Ok(values.into()),
            }
        }
    }

    fn visit_map<A: MapAccess<'de>>(self, mut object: A) -> Result<Value, A::Error> {
        let mut values = Map::new();
        let state = self.0;
        let parent = state.path.len();
        while let Some(key) = object.next_key::<String>()? {
            if safe_key(&key) {
                syntax::push_pointer(&mut state.path, &key);
            }
            if values.contains_key(&key) {
                state.issue = Some(ValidationIssue::new(
                    IssueCode::DuplicateKey,
                    state.path.clone(),
                    "A JSON object key occurs more than once.",
                ));
                return Err(A::Error::custom("duplicate key"));
            }
            let value = object.next_value_seed(JsonValue(state));
            state.path.truncate(parent);
            values.insert(key, value?);
        }
        Ok(values.into())
    }
}

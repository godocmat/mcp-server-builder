use jsonschema::{ValidationError, Validator, draft202012};
use serde_json::Value;

const DRAFT_2020_12: &str = "https://json-schema.org/draft/2020-12/schema";

/// The shared offline JSON Schema 2020-12 compiler used by project and runtime validation.
pub fn compile_schema(schema: &Value) -> Result<Validator, ValidationError<'static>> {
    if !uses_local_schema_refs(schema) {
        return Err(ValidationError::custom(
            "Use JSON Schema 2020-12 with in-document references.",
        ));
    }
    draft202012::options().offline().build(schema)
}

pub fn compile_tool_schema(schema: &Value) -> Result<Validator, ValidationError<'static>> {
    if schema.get("type").and_then(Value::as_str) != Some("object") {
        return Err(ValidationError::custom(
            "Declare an object-root tool schema.",
        ));
    }
    if has_parameter_header(schema) {
        return Err(ValidationError::custom(
            "The pilot tool schema does not support MCP parameter headers.",
        ));
    }
    compile_schema(schema)
}

fn has_parameter_header(value: &Value) -> bool {
    match value {
        Value::Object(fields) => {
            fields.contains_key("x-mcp-header") || fields.values().any(has_parameter_header)
        }
        Value::Array(items) => items.iter().any(has_parameter_header),
        _ => false,
    }
}

fn uses_local_schema_refs(schema: &Value) -> bool {
    match schema {
        Value::Bool(_) => true,
        Value::Object(fields) => fields.iter().all(|(key, value)| match key.as_str() {
            "$schema" => matches!(
                value.as_str(),
                Some(DRAFT_2020_12 | "https://json-schema.org/draft/2020-12/schema#")
            ),
            "$ref" | "$dynamicRef" => value
                .as_str()
                .is_some_and(|reference| reference.starts_with('#')),
            "properties" | "patternProperties" | "dependentSchemas" | "$defs" | "definitions" => {
                value
                    .as_object()
                    .is_some_and(|map| map.values().all(uses_local_schema_refs))
            }
            "allOf" | "anyOf" | "oneOf" | "prefixItems" => value
                .as_array()
                .is_some_and(|values| values.iter().all(uses_local_schema_refs)),
            "items"
            | "contains"
            | "propertyNames"
            | "additionalProperties"
            | "not"
            | "if"
            | "then"
            | "else"
            | "unevaluatedItems"
            | "unevaluatedProperties"
            | "contentSchema" => uses_local_schema_refs(value),
            _ => true,
        }),
        _ => false,
    }
}

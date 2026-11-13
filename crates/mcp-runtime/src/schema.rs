use jsonschema::{ValidationError, Validator, draft202012};
use serde_json::Value;

const DRAFT_2020_12: &str = "https://json-schema.org/draft/2020-12/schema";
const MAX_SCHEMA_DEPTH: usize = 64;
const MAX_SCHEMA_BYTES: usize = 4 * 1024 * 1024;

/// The shared offline JSON Schema 2020-12 compiler used by project and runtime validation.
pub fn compile_schema(schema: &Value) -> Result<Validator, ValidationError<'static>> {
    if !schema_fields_are_supported(schema, false, MAX_SCHEMA_DEPTH) {
        return Err(ValidationError::custom(
            "Use JSON Schema 2020-12 with in-document references.",
        ));
    }
    build(schema)
}

pub fn compile_tool_schema(schema: &Value) -> Result<Validator, ValidationError<'static>> {
    if schema.get("type").and_then(Value::as_str) != Some("object") {
        return Err(ValidationError::custom(
            "Declare an object-root tool schema.",
        ));
    }
    if !schema_fields_are_supported(schema, true, MAX_SCHEMA_DEPTH) {
        return Err(ValidationError::custom(
            "Use local JSON Schema 2020-12 tool schemas without MCP parameter headers.",
        ));
    }
    build(schema)
}

fn build(schema: &Value) -> Result<Validator, ValidationError<'static>> {
    if !crate::execution::json_depth_is_bounded(schema, MAX_SCHEMA_DEPTH)
        || crate::execution::serialized_len(schema, MAX_SCHEMA_BYTES).is_none()
    {
        return Err(ValidationError::custom(
            "The JSON Schema exceeds its depth or byte limit.",
        ));
    }
    draft202012::options().offline().build(schema)
}

fn schema_fields_are_supported(schema: &Value, tool: bool, depth: usize) -> bool {
    let Some(next_depth) = depth.checked_sub(1) else {
        return false;
    };
    let child = |schema: &Value| schema_fields_are_supported(schema, tool, next_depth);
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
            "x-mcp-header" => !tool,
            "properties" | "patternProperties" | "dependentSchemas" | "$defs" | "definitions" => {
                value.as_object().is_some_and(|map| map.values().all(child))
            }
            "allOf" | "anyOf" | "oneOf" | "prefixItems" => value
                .as_array()
                .is_some_and(|values| values.iter().all(child)),
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
            | "contentSchema" => child(value),
            _ => true,
        }),
        _ => false,
    }
}

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::document::optional_field;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScenarioV1 {
    pub version: u32,
    pub id: String,
    pub name: String,
    pub mode: ScenarioMode,
    pub call: Invocation,
    #[serde(
        default,
        deserialize_with = "optional_field",
        skip_serializing_if = "Option::is_none"
    )]
    pub fixtures: Option<Vec<Fixture>>,
    pub assertions: Vec<Assertion>,
    #[serde(
        default,
        deserialize_with = "optional_field",
        skip_serializing_if = "Option::is_none"
    )]
    pub sensitive_paths: Option<Vec<String>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ScenarioMode {
    Fixture,
    Live,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", deny_unknown_fields)]
pub enum Invocation {
    #[serde(rename = "tools/call")]
    Tool {
        name: String,
        arguments: Map<String, Value>,
    },
    #[serde(rename = "resources/read")]
    Resource { uri: String },
    #[serde(rename = "prompts/get")]
    Prompt {
        name: String,
        arguments: BTreeMap<String, String>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Fixture {
    Rest {
        binding_id: String,
        method: String,
        path: String,
        status: u16,
        body: Value,
    },
    FileRead {
        binding_id: String,
        relative_path: String,
        text: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Assertion {
    Equals { path: String, expected: Value },
    Schema { path: String, schema: Value },
    ErrorCode { code: String },
}

use std::collections::BTreeMap;

use mcp_policy::RequestedPolicy;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{PROJECT_FORMAT, PROJECT_SCHEMA_VERSION, ScenarioV1};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectDocument {
    pub format: String,
    pub schema_version: u32,
    pub project_id: Uuid,
    pub name: String,
    pub server: Server,
    pub transport: Transport,
    pub capabilities: Vec<Capability>,
    pub bindings: Vec<Binding>,
    pub policy: RequestedPolicy,
    pub secret_refs: Vec<SecretRef>,
    pub scenarios: Vec<ScenarioV1>,
}

impl ProjectDocument {
    pub fn new(name: String) -> Self {
        Self {
            format: PROJECT_FORMAT.into(),
            schema_version: PROJECT_SCHEMA_VERSION,
            project_id: Uuid::new_v4(),
            name,
            server: Server {
                name: "mcp-server".into(),
                version: "0.1.0".into(),
                description: None,
                instructions: None,
            },
            transport: Transport::Stdio,
            capabilities: Vec::new(),
            bindings: Vec::new(),
            policy: RequestedPolicy::default(),
            secret_refs: Vec::new(),
            scenarios: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Server {
    pub name: String,
    pub version: String,
    #[serde(
        default,
        deserialize_with = "optional_field",
        skip_serializing_if = "Option::is_none"
    )]
    pub description: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional_field",
        skip_serializing_if = "Option::is_none"
    )]
    pub instructions: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Transport {
    Stdio,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Capability {
    Tool(Tool),
    Resource(Resource),
    ResourceTemplate(ResourceTemplate),
    Prompt(Prompt),
}

impl Capability {
    pub fn id(&self) -> &str {
        match self {
            Self::Tool(record) => &record.id,
            Self::Resource(record) => &record.id,
            Self::ResourceTemplate(record) => &record.id,
            Self::Prompt(record) => &record.id,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Tool {
    pub id: String,
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    #[serde(
        default,
        deserialize_with = "optional_field",
        skip_serializing_if = "Option::is_none"
    )]
    pub output_schema: Option<Value>,
    pub binding_id: String,
    #[serde(
        default,
        deserialize_with = "optional_field",
        skip_serializing_if = "Option::is_none"
    )]
    pub timeout_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Resource {
    pub id: String,
    pub name: String,
    pub description: String,
    pub uri: String,
    pub mime_type: String,
    pub binding_id: String,
    #[serde(
        default,
        deserialize_with = "optional_field",
        skip_serializing_if = "Option::is_none"
    )]
    pub timeout_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourceTemplate {
    pub id: String,
    pub name: String,
    pub description: String,
    pub uri_template: String,
    pub variables: BTreeMap<String, Value>,
    pub mime_type: String,
    pub binding_id: String,
    #[serde(
        default,
        deserialize_with = "optional_field",
        skip_serializing_if = "Option::is_none"
    )]
    pub timeout_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Prompt {
    pub id: String,
    pub name: String,
    pub description: String,
    pub arguments: Vec<PromptArgument>,
    pub messages: Vec<PromptMessage>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PromptArgument {
    pub name: String,
    #[serde(
        default,
        deserialize_with = "optional_field",
        skip_serializing_if = "Option::is_none"
    )]
    pub description: Option<String>,
    pub required: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PromptMessage {
    pub role: PromptRole,
    pub text_template: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PromptRole {
    User,
    Assistant,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Binding {
    Rest(RestBinding),
    FileRead(FileReadBinding),
}

impl Binding {
    pub fn id(&self) -> &str {
        match self {
            Self::Rest(record) => &record.id,
            Self::FileRead(record) => &record.id,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RestBinding {
    pub id: String,
    pub origin: String,
    pub method: String,
    pub path_template: String,
    pub path_params: BTreeMap<String, String>,
    #[serde(
        default,
        deserialize_with = "optional_field",
        skip_serializing_if = "Option::is_none"
    )]
    pub body_pointer: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional_field",
        skip_serializing_if = "Option::is_none"
    )]
    pub headers_from_secrets: Option<BTreeMap<String, String>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileReadBinding {
    pub id: String,
    pub root_ref: String,
    pub relative_path_template: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SecretRef {
    pub id: String,
    pub purpose: String,
}

pub(crate) fn optional_field<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

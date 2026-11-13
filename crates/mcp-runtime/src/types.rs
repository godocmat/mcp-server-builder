use std::collections::BTreeMap;
use std::num::NonZeroU64;

use mcp_policy::{DenyReason, Effect, Operation, RequestedPolicy};
use serde::Serialize;
use serde_json::{Map, Value};
use uuid::Uuid;

/// The trusted builder or generated server supplies an inert catalog and requested grants.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerDefinition {
    pub info: ServerInfo,
    pub requested_policy: RequestedPolicy,
    pub tools: Vec<ToolDefinition>,
    pub resources: Vec<ResourceDefinition>,
    pub resource_templates: Vec<ResourceTemplateDefinition>,
    pub prompts: Vec<PromptDefinition>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterBinding {
    pub id: String,
    pub operation: Operation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<NonZeroU64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ToolDefinition {
    pub id: String,
    pub info: ToolInfo,
    pub binding: AdapterBinding,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ResourceDefinition {
    pub id: String,
    pub info: ResourceInfo,
    pub binding: AdapterBinding,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ResourceTemplateDefinition {
    pub id: String,
    pub info: ResourceTemplateInfo,
    pub binding: AdapterBinding,
    pub variables: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PromptDefinition {
    pub id: String,
    pub info: PromptInfo,
    pub messages: Vec<PromptTemplateMessage>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptTemplateMessage {
    pub role: Role,
    pub text_template: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub tools: bool,
    pub resources: bool,
    pub prompts: bool,
}

#[derive(Debug)]
pub struct Page<'a, T> {
    pub items: Vec<&'a T>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceInfo {
    pub name: String,
    pub description: String,
    pub uri: String,
    pub mime_type: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceTemplateInfo {
    pub name: String,
    pub description: String,
    pub uri_template: String,
    pub mime_type: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptInfo {
    pub name: String,
    pub description: String,
    pub arguments: Vec<PromptArgument>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptArgument {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub required: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextContent {
    pub text: String,
}

impl Serialize for TextContent {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Content<'a> {
            #[serde(rename = "type")]
            kind: &'static str,
            text: &'a str,
        }

        Content {
            kind: "text",
            text: &self.text,
        }
        .serialize(serializer)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResult {
    pub content: Vec<TextContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub structured_content: Option<Map<String, Value>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceContent {
    pub uri: String,
    pub mime_type: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ResourceResult {
    pub contents: Vec<ResourceContent>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PromptMessage {
    pub role: Role,
    pub content: TextContent,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PromptResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub messages: Vec<PromptMessage>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    UnknownTool,
    UnknownResource,
    UnknownPrompt,
    InvalidArguments,
    InvalidCursor,
    Denied,
    InvalidOutput,
    OutputLimit,
    AdapterFailed,
    CredentialMissing,
    Cancelled,
    TimedOut,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeError {
    pub kind: ErrorKind,
    pub correlation_id: Uuid,
    pub effect: Effect,
    pub denial: Option<DenyReason>,
}

impl RuntimeError {
    pub const fn code(&self) -> &'static str {
        match self.kind {
            ErrorKind::UnknownTool => "mcp.unknown_tool",
            ErrorKind::UnknownResource => "mcp.unknown_resource",
            ErrorKind::UnknownPrompt => "mcp.unknown_prompt",
            ErrorKind::InvalidArguments => "input.invalid",
            ErrorKind::InvalidCursor => "input.invalid_cursor",
            ErrorKind::Denied => "policy.denied",
            ErrorKind::InvalidOutput => "mcp.invalid_output",
            ErrorKind::OutputLimit => "runtime.output_limit",
            ErrorKind::AdapterFailed => "adapter.failed",
            ErrorKind::CredentialMissing => "credential.missing",
            ErrorKind::Cancelled => "run.cancelled",
            ErrorKind::TimedOut => "runtime.timeout",
        }
    }

    pub const fn message(&self) -> &'static str {
        match self.kind {
            ErrorKind::UnknownTool => "Choose a tool available to this caller.",
            ErrorKind::UnknownResource => "Choose a resource available to this caller.",
            ErrorKind::UnknownPrompt => "Choose a prompt available to this caller.",
            ErrorKind::InvalidArguments => "Correct the arguments and retry the request.",
            ErrorKind::InvalidCursor => "Restart the list without a cursor.",
            ErrorKind::Denied => "The approved policy does not permit this operation.",
            ErrorKind::InvalidOutput => "The adapter result does not match the declared output.",
            ErrorKind::OutputLimit => "The result exceeds the allowed output size.",
            ErrorKind::AdapterFailed => "The adapter could not complete the operation.",
            ErrorKind::CredentialMissing => {
                "Supply the credential configured for this integration."
            }
            ErrorKind::Cancelled => "The operation was cancelled.",
            ErrorKind::TimedOut => "The operation exceeded its deadline.",
        }
    }
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{} ({})", self.code(), self.correlation_id)
    }
}

impl std::error::Error for RuntimeError {}

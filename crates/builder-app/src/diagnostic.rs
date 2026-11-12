use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum DiagnosticCode {
    #[serde(rename = "cli.usage")]
    CliUsage,
    #[serde(rename = "input.invalid")]
    InvalidInput,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Serialize)]
#[serde(untagged, rename_all_fields = "camelCase")]
pub enum DiagnosticDetails {
    SupportedVersions { supported_versions: Vec<u32> },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub code: DiagnosticCode,
    pub message: String,
    pub severity: Severity,
    pub correlation_id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<DiagnosticDetails>,
}

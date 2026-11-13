//! The portable project format and its offline validation.

mod document;
mod parse;
mod projection;
mod revision;
mod scenario;
mod syntax;
mod validation;

pub use document::*;
pub use mcp_runtime::compile_schema;
pub use mcp_runtime::syntax::is_portable_path;
pub use parse::{
    ManifestError, SchemaVersion, ValidatedManifest, parse_manifest, validate_document,
};
pub use projection::runtime_definition;
pub use revision::{InvalidRevision, ManifestRevision};
pub use scenario::*;

pub const MANIFEST_NAME: &str = "mcp-project.json";
pub const PROJECT_FORMAT: &str = "mcp-server-builder.project";
pub const PROJECT_SCHEMA_VERSION: u32 = 1;
pub const MAX_MANIFEST_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IssueCode {
    Json,
    DuplicateKey,
    Field,
    UnsupportedSetting,
    DuplicateId,
    Reference,
    Schema,
    AmbiguousResource,
    Limit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidationIssue {
    pub code: IssueCode,
    /// RFC 6901 pointer. The empty pointer denotes the document itself.
    pub field: String,
    pub capability_id: Option<String>,
    pub message: &'static str,
}

impl ValidationIssue {
    fn new(code: IssueCode, field: impl Into<String>, message: &'static str) -> Self {
        Self {
            code,
            field: field.into(),
            capability_id: None,
            message,
        }
    }
}

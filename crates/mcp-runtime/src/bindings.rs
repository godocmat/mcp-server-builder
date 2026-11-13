//! Portable adapter bindings. Operator roots, trust and credentials are supplied separately.

use std::collections::BTreeMap;

use http::header::HeaderName;
use mcp_policy::{HttpMethod, HttpOrigin};

#[derive(Clone, Debug)]
pub enum StarterBinding {
    Rest(RestBinding),
    FileRead(FileReadBinding),
}

#[derive(Clone, Debug)]
pub struct RestBinding {
    pub origin: HttpOrigin,
    pub method: HttpMethod,
    pub path_template: String,
    pub path_params: BTreeMap<String, String>,
    pub body_pointer: Option<String>,
    pub headers_from_secrets: Vec<(HeaderName, String)>,
}

#[derive(Clone, Debug)]
pub struct FileReadBinding {
    pub root_ref: String,
    pub relative_path_template: String,
}

/// Parse a credential header shared by manifest validation and live dispatch.
pub fn credential_header(value: &str) -> Option<HeaderName> {
    if value.len() > 128 {
        return None;
    }
    let name = value.parse::<HeaderName>().ok()?;
    (!matches!(
        name.as_str(),
        "host"
            | "content-length"
            | "transfer-encoding"
            | "connection"
            | "keep-alive"
            | "upgrade"
            | "te"
            | "trailer"
            | "proxy-authorization"
            | "proxy-connection"
            | "expect"
    ))
    .then_some(name)
}

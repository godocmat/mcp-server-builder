use axum::http::{HeaderMap, StatusCode, header};
use axum::response::Response;
use base64::{Engine, prelude::BASE64_STANDARD};
use rmcp::model::{ClientJsonRpcMessage, ClientNotification, ClientRequest, ErrorCode, GetMeta};
use rmcp::transport::common::http_header::{BASE64_HEADER_PREFIX, BASE64_HEADER_SUFFIX};
use serde_json::Value;

use crate::handler::{LEGACY, MODERN, SUPPORTED};

use super::{LoopbackProfile, SESSION_HEADER, response};

pub(super) const MAX_BODY_BYTES: usize = 1024 * 1024;
pub(super) const VERSION_HEADER: &str = "mcp-protocol-version";
const VERSION_META: &str = "/params/_meta/io.modelcontextprotocol~1protocolVersion";
const METHOD_HEADER: &str = "mcp-method";
const NAME_HEADER: &str = "mcp-name";
const MAX_ID_BYTES: usize = 1024;

pub(super) struct Routed {
    pub id: Option<Value>,
    pub initialize: bool,
    pub stream: Stream,
    pub message: ClientJsonRpcMessage,
}

#[derive(PartialEq, Eq)]
pub(super) enum Stream {
    Modern,
    Legacy,
    None,
}

#[derive(Debug)]
pub(super) struct Rejection {
    status: StatusCode,
    id: Option<Value>,
    code: ErrorCode,
    message: &'static str,
    requested: Option<String>,
    profile: LoopbackProfile,
}

impl Rejection {
    pub(super) fn into_response(self) -> Response {
        let data = self.requested.map(|requested| {
            let versions = if self.profile.allows_legacy() {
                SUPPORTED.as_slice()
            } else {
                &SUPPORTED[..1]
            };
            serde_json::json!({"requested": requested, "supported": versions})
        });
        response::rpc(self.status, self.id, self.code, self.message, data)
    }
}

pub(super) fn one_header<'a>(headers: &'a HeaderMap, name: &str) -> Result<Option<&'a str>, ()> {
    let mut values = headers.get_all(name).iter();
    let first = values.next();
    if values.next().is_some() {
        return Err(());
    }
    first
        .map(|value| value.to_str().map_err(|_| ()))
        .transpose()
}

fn name_matches(value: &str, expected: &str) -> bool {
    match value
        .strip_prefix(BASE64_HEADER_PREFIX)
        .and_then(|inner| inner.strip_suffix(BASE64_HEADER_SUFFIX))
    {
        Some(encoded) => BASE64_STANDARD
            .decode(encoded)
            .is_ok_and(|decoded| decoded == expected.as_bytes()),
        None => value == expected,
    }
}

pub(super) fn accepts_responses(headers: &HeaderMap) -> bool {
    let mut json = false;
    let mut sse = false;
    for value in headers.get_all(header::ACCEPT) {
        let Ok(value) = value.to_str() else { continue };
        for media_type in split_unquoted(value, ',') {
            let mut parts = split_unquoted(media_type, ';');
            let essence = parts.next().unwrap_or_default().trim();
            let allowed = parts.all(|parameter| {
                !parameter.split_once('=').is_some_and(|(key, weight)| {
                    key.trim().eq_ignore_ascii_case("q") && zero_quality(weight.trim())
                })
            });
            if allowed {
                json |= essence.eq_ignore_ascii_case("application/json");
                sse |= essence.eq_ignore_ascii_case("text/event-stream");
            }
        }
    }
    json && sse
}

fn zero_quality(value: &str) -> bool {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    whole == "0" && fraction.len() <= 3 && fraction.bytes().all(|digit| digit == b'0')
}

fn split_unquoted(value: &str, delimiter: char) -> impl Iterator<Item = &str> {
    let mut quoted = false;
    let mut escaped = false;
    value.split(move |character| {
        if escaped {
            escaped = false;
        } else if quoted && character == '\\' {
            escaped = true;
        } else if character == '"' {
            quoted = !quoted;
        } else if !quoted && character == delimiter {
            return true;
        }
        false
    })
}

pub(super) fn valid_session_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_ID_BYTES
        && value.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
}

fn valid_id(id: &Value) -> bool {
    id.as_i64().is_some() || id.as_str().is_some_and(|id| id.len() <= MAX_ID_BYTES)
}

fn is_revision_date(version: &str) -> bool {
    version.len() == 10
        && version
            .bytes()
            .enumerate()
            .all(|(index, byte)| match index {
                4 | 7 => byte == b'-',
                _ => byte.is_ascii_digit(),
            })
}

pub(super) fn validate(
    headers: &HeaderMap,
    bytes: &[u8],
    profile: LoopbackProfile,
) -> Result<Routed, Rejection> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| Rejection {
        status: StatusCode::BAD_REQUEST,
        id: None,
        code: ErrorCode::PARSE_ERROR,
        message: "Invalid JSON.",
        requested: None,
        profile,
    })?;
    let id = value.get("id").filter(|id| valid_id(id)).cloned();
    let reject = |status, code, message| Rejection {
        status,
        id: id.clone(),
        code,
        message,
        requested: None,
        profile,
    };
    let invalid = || {
        reject(
            StatusCode::BAD_REQUEST,
            ErrorCode::INVALID_REQUEST,
            "Invalid MCP request.",
        )
    };
    let invalid_params = || {
        reject(
            StatusCode::BAD_REQUEST,
            ErrorCode::INVALID_PARAMS,
            "Invalid MCP parameters.",
        )
    };
    let mismatch = || {
        reject(
            StatusCode::BAD_REQUEST,
            ErrorCode::HEADER_MISMATCH,
            "Invalid or mismatched MCP routing header.",
        )
    };
    let unsupported = |requested: &str| Rejection {
        requested: Some(requested.to_owned()),
        ..reject(
            StatusCode::BAD_REQUEST,
            ErrorCode::UNSUPPORTED_PROTOCOL_VERSION,
            "Unsupported MCP protocol version.",
        )
    };

    if value.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
        || (value.get("id").is_some() && id.is_none())
        || value.get("result").is_some()
        || value.get("error").is_some()
    {
        return Err(invalid());
    }
    let method = value
        .get("method")
        .and_then(Value::as_str)
        .ok_or_else(invalid)?;
    let header_version = one_header(headers, VERSION_HEADER).map_err(|()| mismatch())?;
    let body_version = value.pointer(VERSION_META).and_then(Value::as_str);
    let initialize = method == "initialize";
    let modern = !initialize
        && (method == "server/discover"
            || body_version.is_some()
            || header_version.is_some_and(|version| version >= MODERN.as_str())
            || !profile.allows_legacy());
    if modern {
        let header_version = header_version.ok_or_else(mismatch)?;
        if !is_revision_date(header_version)
            || body_version.is_some_and(|version| version != header_version)
        {
            return Err(mismatch());
        }
        if header_version != MODERN.as_str() {
            return Err(unsupported(header_version));
        }
        if one_header(headers, METHOD_HEADER).map_err(|()| mismatch())? != Some(method)
            || matches!(method, "tools/call" | "resources/read" | "prompts/get")
                && one_header(headers, NAME_HEADER)
                    .map_err(|()| mismatch())?
                    .is_none()
        {
            return Err(mismatch());
        }
    } else if initialize {
        let offered = value
            .pointer("/params/protocolVersion")
            .and_then(Value::as_str)
            .filter(|version| is_revision_date(version))
            .ok_or_else(invalid_params)?;
        if !profile.allows_legacy() {
            return Err(unsupported(offered));
        }
        if header_version.is_some_and(|version| version != offered)
            || one_header(headers, METHOD_HEADER)
                .map_err(|()| mismatch())?
                .is_some_and(|method| method != "initialize")
        {
            return Err(mismatch());
        }
    } else if header_version != Some(LEGACY.as_str()) {
        return Err(mismatch());
    }

    if !modern && profile.issues_sessions() {
        let session = one_header(headers, SESSION_HEADER).map_err(|()| invalid())?;
        if initialize && session.is_some() || !initialize && !session.is_some_and(valid_session_id)
        {
            return Err(invalid());
        }
    }

    if method == "notifications/cancelled"
        && !value.pointer("/params/requestId").is_some_and(valid_id)
    {
        return Err(invalid_params());
    }
    let message =
        serde_json::from_value::<ClientJsonRpcMessage>(value).map_err(|_| invalid_params())?;
    let stream = match &message {
        ClientJsonRpcMessage::Request(request) => {
            let method = request.request.method();
            if !is_supported_method(method, modern) {
                return Err(reject(
                    if modern {
                        StatusCode::NOT_FOUND
                    } else {
                        StatusCode::OK
                    },
                    ErrorCode::METHOD_NOT_FOUND,
                    "Unknown MCP method.",
                ));
            }
            if matches!(request.request, ClientRequest::CustomRequest(_)) {
                return Err(invalid_params());
            }
            let meta = request.request.get_meta();
            if modern && !meta.missing_required_keys(&MODERN).is_empty() {
                return Err(invalid_params());
            }
            let name = match &request.request {
                ClientRequest::CallToolRequest(call) => Some(call.params.name.as_ref()),
                ClientRequest::ReadResourceRequest(read) => Some(read.params.uri.as_ref()),
                ClientRequest::GetPromptRequest(get) => Some(get.params.name.as_ref()),
                _ => None,
            };
            if let Some(name) = name
                && modern
                && !one_header(headers, NAME_HEADER)
                    .map_err(|()| mismatch())?
                    .is_some_and(|value| name_matches(value, name))
            {
                return Err(mismatch());
            }
            if meta.contains_key("progressToken")
                && !meta.get("progressToken").is_some_and(valid_id)
            {
                return Err(invalid_params());
            }
            match (method, modern) {
                ("tools/call" | "resources/read", true) => Stream::Modern,
                ("tools/call" | "resources/read", false) => Stream::Legacy,
                _ => Stream::None,
            }
        }
        ClientJsonRpcMessage::Notification(notification)
            if !modern
                && matches!(
                    notification.notification,
                    ClientNotification::InitializedNotification(_)
                        | ClientNotification::CancelledNotification(_)
                ) =>
        {
            Stream::None
        }
        _ => return Err(invalid()),
    };
    Ok(Routed {
        id,
        initialize,
        stream,
        message,
    })
}

fn is_supported_method(method: &str, modern: bool) -> bool {
    matches!(
        method,
        "tools/list"
            | "tools/call"
            | "resources/list"
            | "resources/templates/list"
            | "resources/read"
            | "prompts/list"
            | "prompts/get"
    ) || if modern {
        method == "server/discover"
    } else {
        matches!(method, "initialize" | "ping")
    }
}

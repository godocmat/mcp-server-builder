use std::fmt;

use mcp_policy::Effect;
use mcp_runtime::{ErrorKind, RuntimeError};
use rmcp::model::{ErrorCode, ErrorData};
use rmcp::service::{ClientInitializeError, ServiceError};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use super::wire::{INVALID_FRAME, WireError};

const BUILDER_DIAGNOSTIC: &str = "mcp-server-builder/diagnostic";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientError {
    code: &'static str,
    message: &'static str,
    pub correlation_id: Uuid,
}

impl ClientError {
    pub fn code(&self) -> &'static str {
        self.code
    }

    pub fn message(&self) -> &'static str {
        self.message
    }

    pub(super) fn new(correlation_id: Uuid, failure: Failure) -> Self {
        let (code, message) = match failure {
            Failure::Disconnected => ("transport.disconnected", "The MCP connection closed."),
            Failure::InvalidMessage => (
                "transport.invalid_message",
                "The peer sent an invalid MCP response.",
            ),
            Failure::FrameLimit => (
                "transport.frame_limit",
                "An MCP message exceeds the 1 MiB frame limit.",
            ),
            Failure::Busy => (
                "transport.busy",
                "The MCP connection is busy. Retry the request.",
            ),
            Failure::Timeout => ("runtime.timeout", "The operation exceeded its deadline."),
            Failure::Cancelled => ("run.cancelled", "The operation was cancelled."),
            Failure::UnsupportedProtocol => (
                "mcp.unsupported_protocol",
                "The server could not negotiate a supported MCP revision.",
            ),
            Failure::UnsupportedCapability => (
                "mcp.unsupported_capability",
                "The server and client do not share the required capability.",
            ),
            Failure::InvalidInput => ("input.invalid", "The MCP request has invalid parameters."),
            Failure::UnknownMethod => (
                "mcp.unknown_method",
                "The server does not provide the requested MCP method.",
            ),
            Failure::ToolError => ("mcp.tool_error", "The server reported a tool error."),
            Failure::RpcError => ("mcp.rpc_error", "The server reported an MCP error."),
            Failure::DiscoveryLimit => (
                "mcp.discovery_limit",
                "The server's catalog exceeds the playground discovery limit.",
            ),
        };
        Self {
            code,
            message,
            correlation_id,
        }
    }

    pub(super) fn startup(correlation_id: Uuid, error: ClientInitializeError) -> Self {
        match error {
            ClientInitializeError::LegacyFallbackFailed { fallback, .. } => {
                Self::startup(correlation_id, *fallback)
            }
            ClientInitializeError::JsonRpcError(error) => Self::rpc(correlation_id, error, false),
            ClientInitializeError::NoCompatibleProtocolVersion { .. } => {
                Self::new(correlation_id, Failure::UnsupportedProtocol)
            }
            ClientInitializeError::Cancelled => Self::new(correlation_id, Failure::Cancelled),
            ClientInitializeError::ConnectionClosed(_) => {
                Self::new(correlation_id, Failure::Disconnected)
            }
            ClientInitializeError::TransportError { error, .. } => Self::wire(
                correlation_id,
                error.error.downcast_ref::<WireError>().copied(),
            ),
            _ => Self::new(correlation_id, Failure::InvalidMessage),
        }
    }

    pub(super) fn service(correlation_id: Uuid, error: ServiceError, trusted: bool) -> Self {
        match error {
            ServiceError::McpError(error) => Self::rpc(correlation_id, error, trusted),
            ServiceError::TransportSend(error) => Self::wire(
                correlation_id,
                error.error.downcast_ref::<WireError>().copied(),
            ),
            ServiceError::TransportClosed => Self::new(correlation_id, Failure::Disconnected),
            ServiceError::Cancelled { .. } => Self::new(correlation_id, Failure::Cancelled),
            ServiceError::Timeout { .. } => Self::new(correlation_id, Failure::Timeout),
            _ => Self::new(correlation_id, Failure::InvalidMessage),
        }
    }

    fn wire(correlation_id: Uuid, error: Option<WireError>) -> Self {
        let failure = match error {
            Some(WireError::FrameLimit) => Failure::FrameLimit,
            Some(WireError::Busy) => Failure::Busy,
            Some(WireError::InvalidMessage) => Failure::InvalidMessage,
            _ => Failure::Disconnected,
        };
        Self::new(correlation_id, failure)
    }

    fn rpc(correlation_id: Uuid, error: ErrorData, trusted: bool) -> Self {
        if trusted
            && let Some(diagnostic) = error
                .data
                .as_ref()
                .and_then(|data| data.get(BUILDER_DIAGNOSTIC))
                .and_then(|metadata| Self::builder(correlation_id, metadata))
        {
            return diagnostic;
        }
        let failure = match error.code {
            INVALID_FRAME => Failure::InvalidMessage,
            ErrorCode::UNSUPPORTED_PROTOCOL_VERSION => Failure::UnsupportedProtocol,
            ErrorCode::MISSING_REQUIRED_CLIENT_CAPABILITY => Failure::UnsupportedCapability,
            ErrorCode::INVALID_PARAMS => Failure::InvalidInput,
            ErrorCode::METHOD_NOT_FOUND => Failure::UnknownMethod,
            ErrorCode::HEADER_MISMATCH | ErrorCode::INVALID_REQUEST | ErrorCode::PARSE_ERROR => {
                Failure::InvalidMessage
            }
            _ => Failure::RpcError,
        };
        Self::new(correlation_id, failure)
    }

    pub(super) fn tool(correlation_id: Uuid, metadata: Option<&Value>, trusted: bool) -> Self {
        if trusted
            && let Some(diagnostic) = metadata.and_then(|data| Self::builder(correlation_id, data))
        {
            return diagnostic;
        }
        Self::new(correlation_id, Failure::ToolError)
    }

    fn builder(correlation_id: Uuid, metadata: &Value) -> Option<Self> {
        let code = metadata.get("code")?.as_str()?;
        // Verify the trusted producer's shape and keep the local correlation ID
        // that connects the operation to its trace.
        Uuid::parse_str(metadata.get("correlationId")?.as_str()?).ok()?;
        let kind = match code {
            "mcp.unknown_tool" => ErrorKind::UnknownTool,
            "mcp.unknown_resource" => ErrorKind::UnknownResource,
            "mcp.unknown_prompt" => ErrorKind::UnknownPrompt,
            "input.invalid" => ErrorKind::InvalidArguments,
            "input.invalid_cursor" => ErrorKind::InvalidCursor,
            "policy.denied" => ErrorKind::Denied,
            "mcp.invalid_output" => ErrorKind::InvalidOutput,
            "runtime.output_limit" => ErrorKind::OutputLimit,
            "adapter.failed" => ErrorKind::AdapterFailed,
            "credential.missing" => ErrorKind::CredentialMissing,
            "credential.unavailable" => ErrorKind::CredentialUnavailable,
            "credential.invalid" => ErrorKind::CredentialInvalid,
            "run.cancelled" => ErrorKind::Cancelled,
            "runtime.timeout" => ErrorKind::TimedOut,
            "fixture.missing" => ErrorKind::FixtureMissing,
            _ => return None,
        };
        let runtime_error = RuntimeError {
            kind,
            correlation_id,
            effect: Effect::None,
            denial: None,
        };
        Some(Self {
            code: runtime_error.code(),
            message: runtime_error.message(),
            correlation_id,
        })
    }
}

impl fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}

impl std::error::Error for ClientError {}

#[derive(Clone, Copy)]
pub(super) enum Failure {
    Disconnected,
    InvalidMessage,
    FrameLimit,
    Busy,
    Timeout,
    Cancelled,
    UnsupportedProtocol,
    UnsupportedCapability,
    InvalidInput,
    UnknownMethod,
    ToolError,
    RpcError,
    DiscoveryLimit,
}

pub(super) fn tool_diagnostic(result: &rmcp::model::CallToolResult) -> Option<&Value> {
    result.meta.as_ref()?.get(BUILDER_DIAGNOSTIC)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn only_managed_diagnostic_codes_with_valid_shape_keep_their_class() {
        let operation = Uuid::new_v4();
        let peer = Uuid::new_v4();
        const PRIVATE: &str = "peer-diagnostic-private-fd82";
        for code in [
            "mcp.unknown_tool",
            "mcp.unknown_resource",
            "mcp.unknown_prompt",
            "input.invalid",
            "input.invalid_cursor",
            "policy.denied",
            "mcp.invalid_output",
            "runtime.output_limit",
            "adapter.failed",
            "credential.missing",
            "credential.unavailable",
            "credential.invalid",
            "run.cancelled",
            "runtime.timeout",
            "fixture.missing",
        ] {
            let metadata = json!({ "code": code, "correlationId": peer, "message": PRIVATE });
            let rpc = || {
                ErrorData::new(
                    ErrorCode::INTERNAL_ERROR,
                    PRIVATE,
                    Some(json!({ BUILDER_DIAGNOSTIC: metadata })),
                )
            };
            let result = ClientError::rpc(operation, rpc(), true);
            assert_eq!(result.code(), code);
            let public = serde_json::to_value(result).unwrap();
            assert_eq!(public["correlationId"], json!(operation));
            assert!(!public.to_string().contains(PRIVATE));
            assert!(!public.to_string().contains(&peer.to_string()));
            assert!(!result.to_string().contains(PRIVATE));
            assert_eq!(
                ClientError::rpc(operation, rpc(), false).code(),
                "mcp.rpc_error"
            );
            assert_eq!(
                ClientError::tool(operation, Some(&metadata), false).code(),
                "mcp.tool_error"
            );
        }
        for metadata in [
            json!({ "code": "policy.denied", "correlationId": "malformed" }),
            json!({ "code": "peer.custom_error", "correlationId": peer }),
        ] {
            assert_eq!(
                ClientError::tool(operation, Some(&metadata), true).code(),
                "mcp.tool_error"
            );
        }
    }

    #[test]
    fn generic_protocol_errors_do_not_expose_peer_messages_or_data() {
        const PRIVATE: &str = "raw-protocol-message-4a61";
        let operation = Uuid::new_v4();
        for (wire, expected) in [
            (INVALID_FRAME, "transport.invalid_message"),
            (ErrorCode::INVALID_REQUEST, "transport.invalid_message"),
            (ErrorCode::PARSE_ERROR, "transport.invalid_message"),
            (ErrorCode::HEADER_MISMATCH, "transport.invalid_message"),
            (
                ErrorCode::UNSUPPORTED_PROTOCOL_VERSION,
                "mcp.unsupported_protocol",
            ),
            (
                ErrorCode::MISSING_REQUIRED_CLIENT_CAPABILITY,
                "mcp.unsupported_capability",
            ),
            (ErrorCode::INVALID_PARAMS, "input.invalid"),
            (ErrorCode::METHOD_NOT_FOUND, "mcp.unknown_method"),
        ] {
            let error = ErrorData::new(wire, PRIVATE, Some(json!({ "private": PRIVATE })));
            let result = ClientError::service(operation, ServiceError::McpError(error), false);
            assert_eq!(result.code(), expected);
            assert_eq!(result.correlation_id, operation);
            assert!(!serde_json::to_string(&result).unwrap().contains(PRIVATE));
        }
        let error = ServiceError::Cancelled {
            reason: Some(PRIVATE.into()),
        };
        let result = ClientError::service(operation, error, false);
        assert_eq!(result.code(), "run.cancelled");
        assert!(!result.to_string().contains(PRIVATE));
    }
}

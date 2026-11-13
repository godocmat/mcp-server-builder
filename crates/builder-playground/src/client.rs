//! Approved local MCP stdio clients over supervisor-owned process pipes.

mod diagnostic;
mod wire;

use std::sync::Arc;
use std::time::Duration;

use builder_project::Invocation;
use mcp_policy::{ExecutionLimits, RedactionError, Redactor};
use rmcp::model::{
    CallToolRequest, CallToolRequestParams, ClientCapabilities, ClientRequest, ErrorData,
    GetExtensions, GetPromptRequest, GetPromptRequestParams, Implementation,
    InitializeRequestParams, ListPromptsRequest, ListResourceTemplatesRequest,
    ListResourcesRequest, ListToolsRequest, PaginatedRequestParams, ProtocolVersion,
    ReadResourceRequest, ReadResourceRequestParams, ServerCapabilities, ServerResult,
};
use rmcp::service::{
    ClientInitializeError, ClientLifecycleMode, Peer, PeerRequestOptions, RequestContext,
    serve_client_with_lifecycle_and_ct,
};
use rmcp::{ClientHandler, RoleClient};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{Semaphore, watch};
use tokio::time::{Instant, sleep_until, timeout};
use tokio_util::sync::CancellationToken;
use tracing::instrument::WithSubscriber;
use uuid::Uuid;

use crate::trace::{
    MAX_ENTRY_BYTES, TraceBuffer, TraceContext, TraceDirection, TraceKind, TracePayload,
    TraceSnapshot, TraceSummary,
};
use diagnostic::{Failure, tool_diagnostic};
use wire::{
    MAX_ACTIVE_REQUESTS, MAX_FRAME_BYTES, OutboundTrace, PipeTransport, StderrDrain, serialized_len,
};

pub use diagnostic::ClientError;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(15);
const STDIN_CLOSE_TIMEOUT: Duration = Duration::from_millis(250);
const MAX_DISCOVERY_BYTES: usize = 16 * MAX_FRAME_BYTES;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum ProtocolPreference {
    #[default]
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "2026-07-28")]
    Modern,
    #[serde(rename = "2025-11-25")]
    Legacy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ProtocolRevision {
    #[serde(rename = "2026-07-28")]
    Modern,
    #[serde(rename = "2025-11-25")]
    Legacy,
}

impl ProtocolRevision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Modern => "2026-07-28",
            Self::Legacy => "2025-11-25",
        }
    }
}

#[derive(Clone, Default)]
pub struct ClientOptions {
    pub protocol: ProtocolPreference,
    pub capture_payloads: bool,
    /// Enable only for an authenticated builder-owned launch.
    pub trust_builder_diagnostics: bool,
    pub redactor: Arc<Redactor>,
}

#[derive(Clone)]
pub struct RequestOptions {
    pub correlation_id: Uuid,
    pub timeout: Duration,
    pub cancellation: CancellationToken,
    /// The operation's additional redaction policy, including scenario paths.
    pub redactor: Option<Arc<Redactor>>,
}

impl Default for RequestOptions {
    fn default() -> Self {
        Self {
            correlation_id: Uuid::new_v4(),
            timeout: ExecutionLimits::DEFAULT_TIMEOUT,
            cancellation: CancellationToken::new(),
            redactor: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VisibleCapabilities {
    pub tools: bool,
    pub resources: bool,
    pub prompts: bool,
}

impl From<&ServerCapabilities> for VisibleCapabilities {
    fn from(capabilities: &ServerCapabilities) -> Self {
        Self {
            tools: capabilities.tools.is_some(),
            resources: capabilities.resources.is_some(),
            prompts: capabilities.prompts.is_some(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Discovery {
    pub protocol_revision: ProtocolRevision,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_info: Option<Value>,
    pub capabilities: VisibleCapabilities,
    pub supported_methods: Vec<&'static str>,
    pub tools: Vec<Value>,
    pub resources: Vec<Value>,
    pub resource_templates: Vec<Value>,
    pub prompts: Vec<Value>,
}

pub struct StdioClient {
    peer: Peer<RoleClient>,
    revision: ProtocolRevision,
    capabilities: VisibleCapabilities,
    server_info: Option<Value>,
    trust_builder_diagnostics: bool,
    redactor: Arc<Redactor>,
    trace: Arc<TraceBuffer>,
    connection: TraceContext,
    closing: CancellationToken,
    writer_done: watch::Receiver<bool>,
    admission: Arc<Semaphore>,
    _stderr: StderrDrain,
}

impl StdioClient {
    /// Negotiate over an already-approved process's pipes. The cancellation
    /// token applies to startup. Use request options to cancel individual calls.
    ///
    /// Host tracing layers must exclude SDK diagnostic events, since rmcp's
    /// spawned tasks may format untrusted peer data. Use the shared
    /// `mcp_transport::sdk_diagnostic_filter` when configuring those layers.
    pub async fn connect(
        stdout: impl AsyncRead + Send + Unpin + 'static,
        stdin: impl AsyncWrite + Send + Unpin + 'static,
        stderr: impl AsyncRead + Send + Unpin + 'static,
        options: ClientOptions,
        cancellation: &CancellationToken,
    ) -> Result<Self, ClientError> {
        let connection = TraceContext::new(Uuid::new_v4(), None);
        let error = |failure| ClientError::new(connection.correlation_id, failure);
        let closing = CancellationToken::new();
        let trace = Arc::new(TraceBuffer::new(options.capture_payloads));
        let stderr = StderrDrain::start(stderr, trace.clone(), connection);
        let (transport, writer_done) =
            PipeTransport::new(stdout, stdin, trace.clone(), connection, closing.clone());
        let lifecycle = match options.protocol {
            ProtocolPreference::Auto => ClientLifecycleMode::Auto {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                legacy_version: Some(ProtocolVersion::V_2025_11_25),
            },
            ProtocolPreference::Modern => ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
            ProtocolPreference::Legacy => ClientLifecycleMode::Initialize,
        };
        let startup =
            serve_client_with_lifecycle_and_ct(Handler, transport, lifecycle, closing.clone())
                .with_subscriber(tracing::subscriber::NoSubscriber::default());
        let running = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(error(Failure::Cancelled)),
            result = timeout(STARTUP_TIMEOUT, startup) => match result {
                Ok(Ok(running)) => running,
                Ok(Err(ClientInitializeError::Cancelled)) if closing.is_cancelled() => {
                    return Err(error(Failure::Disconnected));
                }
                Ok(Err(failure)) => return Err(ClientError::startup(connection.correlation_id, failure)),
                Err(_) => return Err(error(Failure::Timeout)),
            }
        };
        let peer_info = running
            .peer_info()
            .ok_or_else(|| error(Failure::InvalidMessage))?;
        let revision = match peer_info.protocol_version.as_str() {
            "2026-07-28" if options.protocol != ProtocolPreference::Legacy => {
                ProtocolRevision::Modern
            }
            "2025-11-25" if options.protocol != ProtocolPreference::Modern => {
                ProtocolRevision::Legacy
            }
            _ => return Err(error(Failure::UnsupportedProtocol)),
        };
        let capabilities = (&peer_info.capabilities).into();
        let server_info = peer_info.server_info.as_ref().and_then(|info| {
            options
                .redactor
                .redact_json(
                    json!({"name": info.name, "version": info.version}),
                    MAX_ENTRY_BYTES,
                )
                .ok()
        });
        let peer = running.peer().clone();
        // The SDK service owns the reader. The independent writer task can
        // close the child pipe immediately even if the SDK is still draining.
        tokio::spawn(running.waiting());
        Ok(Self {
            peer,
            revision,
            capabilities,
            server_info,
            trust_builder_diagnostics: options.trust_builder_diagnostics,
            redactor: options.redactor,
            trace,
            connection,
            closing,
            writer_done,
            admission: Arc::new(Semaphore::new(MAX_ACTIVE_REQUESTS)),
            _stderr: stderr,
        })
    }

    pub fn protocol_revision(&self) -> ProtocolRevision {
        self.revision
    }

    pub async fn discover(&self, options: RequestOptions) -> Result<Discovery, ClientError> {
        let deadline = operation_deadline(&options);
        let error = |failure| ClientError::new(options.correlation_id, failure);
        self.check_open(&options, deadline)?;
        let mut remaining_bytes = MAX_DISCOVERY_BYTES;
        let mut discovery = Discovery {
            protocol_revision: self.revision,
            server_info: self.server_info.clone(),
            capabilities: self.capabilities,
            supported_methods: Vec::new(),
            tools: Vec::new(),
            resources: Vec::new(),
            resource_templates: Vec::new(),
            prompts: Vec::new(),
        };
        if self.revision == ProtocolRevision::Modern {
            discovery.supported_methods.push("server/discover");
        } else {
            discovery.supported_methods.push("ping");
        }
        for list in [
            List::Tools,
            List::Resources,
            List::ResourceTemplates,
            List::Prompts,
        ] {
            if !list.visible(self.capabilities) {
                continue;
            }
            discovery.supported_methods.push(list.method());
            let items = match list {
                List::Tools => {
                    discovery.supported_methods.push("tools/call");
                    &mut discovery.tools
                }
                List::Resources => {
                    discovery.supported_methods.push("resources/read");
                    &mut discovery.resources
                }
                List::ResourceTemplates => &mut discovery.resource_templates,
                List::Prompts => {
                    discovery.supported_methods.push("prompts/get");
                    &mut discovery.prompts
                }
            };
            let mut cursor = None;
            loop {
                let request = list.request(cursor.clone());
                let (response, context) = self
                    .request(
                        request,
                        list.method(),
                        &options,
                        deadline,
                        TracePayload::None,
                    )
                    .await?;
                let normalized = list.page(response).map_err(error);
                let (value, next) = normalized?;
                let size = serialized_len(&value, remaining_bytes)
                    .ok_or_else(|| error(Failure::DiscoveryLimit))?;
                remaining_bytes -= size;
                let value = self.sanitize_catalog(value, &options, size)?;
                self.trace_result(context, Some(&value), &options);
                if let Value::Array(page) = value {
                    items.extend(page);
                }
                if next.is_none() {
                    break;
                }
                if cursor == next {
                    return Err(error(Failure::InvalidMessage));
                }
                cursor = next;
            }
        }
        Ok(discovery)
    }

    /// The result remains internal until scenario assertions have evaluated it
    /// and the caller has redacted any transient display value.
    pub(crate) async fn invoke(
        &self,
        call: &Invocation,
        options: RequestOptions,
    ) -> Result<Value, ClientError> {
        let deadline = operation_deadline(&options);
        self.check_open(&options, deadline)?;
        let (request, method, supported) = match call {
            Invocation::Tool { name, arguments } => (
                CallToolRequest::new(
                    CallToolRequestParams::new(name.clone()).with_arguments(arguments.clone()),
                )
                .into(),
                "tools/call",
                self.capabilities.tools,
            ),
            Invocation::Resource { uri } => (
                ReadResourceRequest::new(ReadResourceRequestParams::new(uri.clone())).into(),
                "resources/read",
                self.capabilities.resources,
            ),
            Invocation::Prompt { name, arguments } => (
                GetPromptRequest::new(
                    GetPromptRequestParams::new(name.clone()).with_arguments(
                        arguments
                            .iter()
                            .map(|(name, value)| (name.clone(), Value::String(value.clone())))
                            .collect(),
                    ),
                )
                .into(),
                "prompts/get",
                self.capabilities.prompts,
            ),
        };
        let error = |failure| ClientError::new(options.correlation_id, failure);
        if !supported {
            return Err(error(Failure::UnsupportedCapability));
        }
        let payload = if self.trace.captures_payloads() {
            match serde_json::to_value(call) {
                Ok(value) => self.redact_payload(&value, &options).into(),
                Err(_) => TracePayload::Omitted,
            }
        } else {
            TracePayload::None
        };
        let (response, context) = self
            .request(request, method, &options, deadline, payload)
            .await?;
        let normalized = match (call, response) {
            (Invocation::Tool { .. }, ServerResult::CallToolResult(result)) => {
                if result.is_error == Some(true) {
                    Err(ClientError::tool(
                        options.correlation_id,
                        tool_diagnostic(&result),
                        self.trust_builder_diagnostics,
                    ))
                } else {
                    let mut value = json!({"content": result.content});
                    strip_protocol_metadata(&mut value);
                    if let Some(structured) = result.structured_content {
                        value["structuredContent"] = structured;
                    }
                    Ok(value)
                }
            }
            (Invocation::Resource { .. }, ServerResult::ReadResourceResult(result)) => {
                let mut value = json!({"contents": result.contents});
                strip_protocol_metadata(&mut value);
                Ok(value)
            }
            (Invocation::Prompt { .. }, ServerResult::GetPromptResult(result)) => {
                let mut value = json!({"messages": result.messages});
                strip_protocol_metadata(&mut value);
                if let Some(description) = result.description {
                    value["description"] = Value::String(description);
                }
                Ok(value)
            }
            (_, ServerResult::InputRequiredResult(_)) | (_, ServerResult::CreateTaskResult(_)) => {
                Err(error(Failure::UnsupportedCapability))
            }
            _ => Err(error(Failure::InvalidMessage)),
        };
        self.trace_result(context, normalized.as_ref().ok(), &options);
        normalized
    }

    pub fn trace(&self) -> TraceSnapshot {
        self.trace.snapshot()
    }

    pub fn trace_summary(&self) -> TraceSummary {
        self.trace.summary()
    }

    pub async fn close(&self) -> Result<(), ClientError> {
        self.closing.cancel();
        self.trace.close_payloads();
        let mut done = self.writer_done.clone();
        timeout(STDIN_CLOSE_TIMEOUT, done.wait_for(|closed| *closed))
            .await
            .map_err(|_| ClientError::new(self.connection.correlation_id, Failure::Timeout))?
            .map_err(|_| ClientError::new(self.connection.correlation_id, Failure::Disconnected))?;
        Ok(())
    }

    async fn request(
        &self,
        mut request: ClientRequest,
        method: &'static str,
        options: &RequestOptions,
        deadline: Instant,
        payload: TracePayload,
    ) -> Result<(ServerResult, TraceContext), ClientError> {
        self.check_open(options, deadline)?;
        let error = |failure| ClientError::new(options.correlation_id, failure);
        let permit = self
            .admission
            .clone()
            .try_acquire_owned()
            .map_err(|_| error(Failure::Busy))?;
        let mut context = TraceContext::new(options.correlation_id, Some(method));
        let retired = CancellationToken::new();
        request.extensions_mut().insert(OutboundTrace {
            context,
            payload,
            retired: retired.clone(),
        });
        let handle = tokio::select! {
            biased;
            _ = options.cancellation.cancelled() => return Err(error(Failure::Cancelled)),
            _ = self.closing.cancelled() => return Err(error(Failure::Disconnected)),
            _ = sleep_until(deadline) => return Err(error(Failure::Timeout)),
            handle = self.peer.send_cancellable_request(request, PeerRequestOptions::no_options()) => {
                handle.map_err(|failure| ClientError::service(options.correlation_id, failure, self.trust_builder_diagnostics))?
            }
        };
        if let rmcp::model::NumberOrString::Number(id) = handle.id {
            context.message_id = Some(id);
        }
        let mut pending = PendingRequest {
            handle: Some(handle),
            retired,
            permit: Some(permit),
            closing: self.closing.clone(),
        };
        let response = tokio::select! {
            biased;
            _ = options.cancellation.cancelled() => Err((Failure::Cancelled, TraceKind::Cancelled)),
            _ = self.closing.cancelled() => Err((Failure::Disconnected, TraceKind::Error)),
            _ = sleep_until(deadline) => Err((Failure::Timeout, TraceKind::TimedOut)),
            response = &mut pending.handle.as_mut().expect("active MCP request").rx => Ok(response),
        };
        match response {
            Ok(response) => {
                pending.handle.take();
                match response {
                    Ok(Ok(result)) => Ok((result, context)),
                    Ok(Err(failure)) => {
                        self.trace_result(context, None, options);
                        Err(ClientError::service(
                            options.correlation_id,
                            failure,
                            self.trust_builder_diagnostics,
                        ))
                    }
                    Err(_) => {
                        self.trace_result(context, None, options);
                        Err(error(Failure::Disconnected))
                    }
                }
            }
            Err((failure, kind)) => {
                self.trace
                    .record(context, TraceDirection::Received, kind, TracePayload::None);
                Err(error(failure))
            }
        }
    }

    fn check_open(&self, options: &RequestOptions, deadline: Instant) -> Result<(), ClientError> {
        let failure = if options.cancellation.is_cancelled() {
            Failure::Cancelled
        } else if self.closing.is_cancelled() {
            Failure::Disconnected
        } else if Instant::now() >= deadline {
            Failure::Timeout
        } else {
            return Ok(());
        };
        Err(ClientError::new(options.correlation_id, failure))
    }

    fn trace_result(&self, context: TraceContext, value: Option<&Value>, options: &RequestOptions) {
        let kind = if value.is_some() {
            TraceKind::Response
        } else {
            TraceKind::Error
        };
        let payload = match value {
            Some(value) if self.trace.captures_payloads() => {
                self.redact_payload(value, options).into()
            }
            _ => TracePayload::None,
        };
        self.trace
            .record(context, TraceDirection::Received, kind, payload);
    }

    fn redact_payload(
        &self,
        value: &Value,
        options: &RequestOptions,
    ) -> Result<Value, RedactionError> {
        let sanitized = redact_result(value, &self.redactor, MAX_ENTRY_BYTES)?;
        match &options.redactor {
            Some(redactor) => redact_result(&sanitized, redactor, MAX_ENTRY_BYTES),
            None => Ok(sanitized),
        }
    }

    fn sanitize_catalog(
        &self,
        value: Value,
        options: &RequestOptions,
        frame_bytes: usize,
    ) -> Result<Value, ClientError> {
        let sanitize = |value, redactor: &Redactor| {
            redactor.redact_json(value, frame_bytes.max(MAX_FRAME_BYTES))
        };
        let value = sanitize(value, &self.redactor).and_then(|value| match &options.redactor {
            Some(redactor) => sanitize(value, redactor),
            None => Ok(value),
        });
        value.map_err(|_| ClientError::new(options.correlation_id, Failure::DiscoveryLimit))
    }
}

impl Drop for StdioClient {
    fn drop(&mut self) {
        self.closing.cancel();
        self.trace.close_payloads();
    }
}

struct PendingRequest {
    handle: Option<rmcp::service::RequestHandle<RoleClient>>,
    permit: Option<tokio::sync::OwnedSemaphorePermit>,
    retired: CancellationToken,
    closing: CancellationToken,
}

impl Drop for PendingRequest {
    fn drop(&mut self) {
        self.retired.cancel();
        if let Some(handle) = self.handle.take() {
            let permit = self.permit.take();
            let closing = self.closing.clone();
            // Cancellation is scoped by the SDK request ID. Keep its admission
            // slot until the SDK retires it, including when stdin is backlogged.
            tokio::spawn(async move {
                let _permit = permit;
                tokio::select! {
                    _ = closing.cancelled() => {}
                    _ = handle.cancel(None) => {}
                }
            });
        }
    }
}

fn operation_deadline(options: &RequestOptions) -> Instant {
    Instant::now() + options.timeout.min(ExecutionLimits::MAX_TIMEOUT)
}

/// Successful managed tools can mirror structured JSON in text content. If
/// redaction changes the structured form, omit the duplicate content from
/// evidence while preserving the sanitized structured result.
pub(crate) fn redact_result(
    value: &Value,
    redactor: &Redactor,
    max_bytes: usize,
) -> Result<Value, RedactionError> {
    let Some(structured) = value
        .get("structuredContent")
        .filter(|_| value.get("content").is_some())
    else {
        return redactor.redact_json(value.clone(), max_bytes);
    };
    let mut structured_only = json!({"structuredContent": structured});
    let redacted = redactor.redact_json(structured_only.clone(), MAX_FRAME_BYTES)?;
    if redacted.get("structuredContent") != Some(structured) {
        structured_only["content"] = Value::Array(Vec::new());
        return redactor.redact_json(structured_only, max_bytes);
    }
    redactor.redact_json(value.clone(), max_bytes)
}

fn strip_protocol_metadata(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            fields.remove("_meta");
            for field in fields.values_mut() {
                strip_protocol_metadata(field);
            }
        }
        Value::Array(items) => {
            for item in items {
                strip_protocol_metadata(item);
            }
        }
        _ => {}
    }
}

struct Handler;

impl ClientHandler for Handler {
    fn get_info(&self) -> InitializeRequestParams {
        InitializeRequestParams::new(
            ClientCapabilities::default(),
            Implementation::new("mcp-server-builder", env!("CARGO_PKG_VERSION")),
        )
        .with_protocol_version(ProtocolVersion::V_2025_11_25)
    }

    async fn on_custom_request(
        &self,
        _request: rmcp::model::CustomRequest,
        _context: RequestContext<RoleClient>,
    ) -> Result<rmcp::model::CustomResult, ErrorData> {
        Err(ErrorData::new(
            rmcp::model::ErrorCode::METHOD_NOT_FOUND,
            "Unknown MCP method",
            None,
        ))
    }
}

#[derive(Clone, Copy)]
enum List {
    Tools,
    Resources,
    ResourceTemplates,
    Prompts,
}

impl List {
    fn method(self) -> &'static str {
        match self {
            Self::Tools => "tools/list",
            Self::Resources => "resources/list",
            Self::ResourceTemplates => "resources/templates/list",
            Self::Prompts => "prompts/list",
        }
    }

    fn visible(self, capabilities: VisibleCapabilities) -> bool {
        match self {
            Self::Tools => capabilities.tools,
            Self::Resources | Self::ResourceTemplates => capabilities.resources,
            Self::Prompts => capabilities.prompts,
        }
    }

    fn request(self, cursor: Option<String>) -> ClientRequest {
        let params = PaginatedRequestParams::default().with_cursor(cursor);
        match self {
            Self::Tools => ListToolsRequest::with_param(params).into(),
            Self::Resources => ListResourcesRequest::with_param(params).into(),
            Self::ResourceTemplates => ListResourceTemplatesRequest::with_param(params).into(),
            Self::Prompts => ListPromptsRequest::with_param(params).into(),
        }
    }

    fn page(self, result: ServerResult) -> Result<(Value, Option<String>), Failure> {
        let (value, cursor) = match (self, result) {
            (Self::Tools, ServerResult::ListToolsResult(result)) => {
                (serde_json::to_value(result.tools), result.next_cursor)
            }
            (Self::Resources, ServerResult::ListResourcesResult(result)) => {
                (serde_json::to_value(result.resources), result.next_cursor)
            }
            (Self::ResourceTemplates, ServerResult::ListResourceTemplatesResult(result)) => (
                serde_json::to_value(result.resource_templates),
                result.next_cursor,
            ),
            (Self::Prompts, ServerResult::ListPromptsResult(result)) => {
                (serde_json::to_value(result.prompts), result.next_cursor)
            }
            _ => return Err(Failure::InvalidMessage),
        };
        let mut value = value.map_err(|_| Failure::InvalidMessage)?;
        strip_protocol_metadata(&mut value);
        Ok((value, cursor))
    }
}

use std::borrow::Cow;
use std::sync::Arc;

use mcp_runtime::{
    BoxFuture, ErrorKind, ExecutionContext, Principal, Progress, ProgressSink, Runtime,
    RuntimeError,
};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, GetPromptRequestParams,
    GetPromptResponse, GetPromptResult, Implementation, ListPromptsResult,
    ListResourceTemplatesResult, ListResourcesResult, ListToolsResult, MetaObject,
    PaginatedRequestParams, ProgressNotificationParam, Prompt, PromptArgument, PromptMessage,
    ProtocolVersion, ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, Resource,
    ResourceContents, ResourceTemplate, Role, ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler};
use serde_json::{Value, json};

pub(super) const MODERN: ProtocolVersion = ProtocolVersion::V_2026_07_28;
pub(super) const LEGACY: ProtocolVersion = ProtocolVersion::V_2025_11_25;
pub(super) const SUPPORTED: [ProtocolVersion; 2] = [MODERN, LEGACY];

/// Exclude SDK events from each tracing layer that records diagnostics or evidence.
/// SDK debug fields and cancellation traces can contain raw request data.
pub fn sdk_diagnostic_filter(metadata: &tracing::Metadata<'_>) -> bool {
    metadata.target() != "rmcp" && !metadata.target().starts_with("rmcp::")
}

#[derive(Clone)]
pub(super) struct LocalHandler {
    runtime: Arc<Runtime>,
    operator: Arc<str>,
    versions: &'static [ProtocolVersion],
}

impl LocalHandler {
    pub(super) fn new(runtime: Arc<Runtime>, operator: String) -> Self {
        Self {
            runtime,
            operator: operator.into(),
            versions: &SUPPORTED,
        }
    }

    #[cfg(feature = "server-http")]
    pub(super) fn modern_only(mut self) -> Self {
        self.versions = &[MODERN];
        self
    }

    fn principal(&self) -> Principal<'_> {
        Principal::Local {
            operator: &self.operator,
        }
    }
}

impl ServerHandler for LocalHandler {
    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(self.versions)
    }

    fn get_info(&self) -> ServerConfig {
        let info = self.runtime.info();
        let visible = self.runtime.capabilities(&self.principal());
        let mut capabilities = ServerCapabilities::default();
        capabilities.tools = visible.tools.then(Default::default);
        capabilities.resources = visible.resources.then(Default::default);
        capabilities.prompts = visible.prompts.then(Default::default);
        let mut server = Implementation::new(info.name.clone(), info.version.clone());
        server.description = info.description.clone();
        let mut config = ServerConfig::new(capabilities)
            .with_protocol_version(LEGACY)
            .with_server_info(server);
        config.instructions = info.instructions.clone();
        config
    }

    async fn list_tools(
        &self,
        params: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let page = self
            .runtime
            .list_tools(&self.principal(), cursor(params.as_ref()))
            .map_err(protocol_error)?;
        let tools =
            page.items
                .into_iter()
                .map(|item| {
                    let schema = item.input_schema.as_object().ok_or_else(|| {
                        ErrorData::internal_error("Invalid tool input schema.", None)
                    })?;
                    let mut tool =
                        Tool::new(item.name.clone(), item.description.clone(), schema.clone());
                    tool.output_schema = item
                        .output_schema
                        .as_ref()
                        .and_then(Value::as_object)
                        .cloned()
                        .map(Arc::new);
                    Ok(tool)
                })
                .collect::<Result<Vec<_>, ErrorData>>()?;
        let mut result = ListToolsResult::with_all_items(tools);
        result.next_cursor = page.next_cursor;
        Ok(result)
    }

    async fn list_resources(
        &self,
        params: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        let page = self
            .runtime
            .list_resources(&self.principal(), cursor(params.as_ref()))
            .map_err(protocol_error)?;
        let mut result = ListResourcesResult::with_all_items(
            page.items
                .into_iter()
                .map(|item| {
                    Resource::new(item.uri.clone(), item.name.clone())
                        .with_description(item.description.clone())
                        .with_mime_type(item.mime_type.clone())
                })
                .collect(),
        );
        result.next_cursor = page.next_cursor;
        Ok(result)
    }

    async fn list_resource_templates(
        &self,
        params: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        let page = self
            .runtime
            .list_resource_templates(&self.principal(), cursor(params.as_ref()))
            .map_err(protocol_error)?;
        let mut result = ListResourceTemplatesResult::with_all_items(
            page.items
                .into_iter()
                .map(|item| {
                    ResourceTemplate::new(item.uri_template.clone(), item.name.clone())
                        .with_description(item.description.clone())
                        .with_mime_type(item.mime_type.clone())
                })
                .collect(),
        );
        result.next_cursor = page.next_cursor;
        Ok(result)
    }

    async fn list_prompts(
        &self,
        params: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        let page = self
            .runtime
            .list_prompts(&self.principal(), cursor(params.as_ref()))
            .map_err(protocol_error)?;
        let mut result = ListPromptsResult::with_all_items(
            page.items
                .into_iter()
                .map(|item| {
                    let arguments = item
                        .arguments
                        .iter()
                        .map(|argument| {
                            let mut dto = PromptArgument::new(argument.name.clone())
                                .with_required(argument.required);
                            dto.description = argument.description.clone();
                            dto
                        })
                        .collect();
                    Prompt::new(
                        item.name.clone(),
                        Some(item.description.clone()),
                        Some(arguments),
                    )
                })
                .collect(),
        );
        result.next_cursor = page.next_cursor;
        Ok(result)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let principal = self.principal();
        let sink = ProgressReporter(&context);
        let execution = ExecutionContext::new(&principal, context.ct.clone()).with_progress(&sink);
        match self
            .runtime
            .call_tool(
                &request.name,
                request.arguments.unwrap_or_default(),
                &execution,
            )
            .await
        {
            Ok(output) => {
                let mut result = CallToolResult::success(
                    output
                        .content
                        .into_iter()
                        .map(|content| ContentBlock::text(content.text))
                        .collect(),
                );
                result.structured_content = output.structured_content.map(Value::Object);
                Ok(result.into())
            }
            Err(error) if error.kind == ErrorKind::UnknownTool => Err(protocol_error(error)),
            Err(error) => {
                let mut result = CallToolResult::error(vec![ContentBlock::text(error.message())]);
                result.meta = Some(MetaObject::from(error_details(&error)));
                Ok(result.into())
            }
        }
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        let principal = self.principal();
        let sink = ProgressReporter(&context);
        let execution = ExecutionContext::new(&principal, context.ct.clone()).with_progress(&sink);
        let result = self
            .runtime
            .read_resource(&request.uri, &execution)
            .await
            .map_err(protocol_error)?;
        Ok(ReadResourceResult::new(
            result
                .contents
                .into_iter()
                .map(|content| {
                    ResourceContents::text(content.text, content.uri)
                        .with_mime_type(content.mime_type)
                })
                .collect(),
        )
        .into())
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, ErrorData> {
        let principal = self.principal();
        let execution = ExecutionContext::new(&principal, context.ct.clone());
        let arguments = request
            .arguments
            .unwrap_or_default()
            .into_iter()
            .map(|(name, value)| match value {
                Value::String(value) => Ok((name, value)),
                _ => Err(ErrorData::invalid_params(
                    "Prompt arguments must be strings.",
                    None,
                )),
            })
            .collect::<Result<_, _>>()?;
        let output = self
            .runtime
            .get_prompt(&request.name, arguments, &execution)
            .await
            .map_err(protocol_error)?;
        let mut result = GetPromptResult::new(
            output
                .messages
                .into_iter()
                .map(|message| {
                    let role = match message.role {
                        mcp_runtime::Role::User => Role::User,
                        mcp_runtime::Role::Assistant => Role::Assistant,
                    };
                    PromptMessage::new_text(role, message.content.text)
                })
                .collect(),
        );
        result.description = output.description;
        Ok(result.into())
    }
}

fn cursor(params: Option<&PaginatedRequestParams>) -> Option<&str> {
    params.and_then(|params| params.cursor.as_deref())
}

fn error_details(error: &RuntimeError) -> serde_json::Map<String, Value> {
    [(
        "mcp-server-builder/diagnostic".into(),
        json!({"code":error.code(),"correlationId":error.correlation_id}),
    )]
    .into_iter()
    .collect()
}

fn protocol_error(error: RuntimeError) -> ErrorData {
    let message = error.message();
    let details = Some(Value::Object(error_details(&error)));
    match error.kind {
        ErrorKind::UnknownResource => ErrorData::resource_not_found(message, details),
        ErrorKind::UnknownTool
        | ErrorKind::UnknownPrompt
        | ErrorKind::InvalidArguments
        | ErrorKind::InvalidCursor
        | ErrorKind::Denied => ErrorData::invalid_params(message, details),
        _ => ErrorData::internal_error(message, details),
    }
}

struct ProgressReporter<'a>(&'a RequestContext<RoleServer>);

impl ProgressSink for ProgressReporter<'_> {
    fn send(&self, event: Progress) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let Some(token) = self.0.meta.get_progress_token() else {
                return;
            };
            let mut progress = ProgressNotificationParam::new(token, event.progress);
            progress.total = event.total;
            tokio::select! {
                biased;
                _ = self.0.ct.cancelled() => {},
                _ = self.0.peer.notify_progress(progress) => {}
            }
        })
    }
}

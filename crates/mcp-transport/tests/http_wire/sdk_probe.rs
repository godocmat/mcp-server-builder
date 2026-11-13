use std::borrow::Cow;
use std::sync::Arc;

use axum::Router;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ClientCapabilities, ProtocolVersion,
};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData, RoleServer, ServerHandler};
use serde_json::json;
use tokio::net::TcpListener;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use super::assert_error;
use super::client::{post, send, tool};

struct Probe;

impl ServerHandler for Probe {
    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(&[ProtocolVersion::V_2026_07_28])
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let required = ClientCapabilities::builder().enable_elicitation().build();
        if !context
            .client_capabilities()
            .is_some_and(|caps| caps.elicitation.is_some())
        {
            return Err(ErrorData::missing_required_client_capability(required));
        }
        let value = request.arguments.unwrap()["id"].clone();
        Ok(CallToolResult::structured(value).into())
    }
}

#[tokio::test]
async fn sdk_only_modern_capability_error_and_non_object_results() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let shutdown = CancellationToken::new();
    let sdk = StreamableHttpService::new(
        || Ok(Probe),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default()
            .with_legacy_session_mode(false)
            .with_stateless_protocol_metadata_required(true)
            .with_json_response(true)
            .with_allowed_hosts([address.to_string()])
            .with_cancellation_token(shutdown.child_token()),
    );
    let router = Router::new().route_service("/mcp", sdk);
    let server_shutdown = shutdown.clone();
    let task = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(server_shutdown.cancelled_owned())
            .await
    });
    let missing = tool(30, json!([1, 2]), true, None);
    let reply = send(address, post(address, &missing, true, None)).await;
    let missing = assert_error(reply, 400, -32021, Some(30)).await;
    assert_eq!(
        missing["error"]["data"]["requiredCapabilities"],
        json!({"elicitation":{}})
    );

    for (id, value) in [(31, json!([1, 2])), (32, json!(42))] {
        let mut request = tool(id, value.clone(), true, None);
        request["params"]["_meta"]["io.modelcontextprotocol/clientCapabilities"] =
            json!({"elicitation":{}});
        let mut reply = send(address, post(address, &request, true, None)).await;
        assert_eq!(reply.status(), 200);
        let terminal = reply.terminal().await;
        assert_eq!(terminal["id"], id);
        assert_eq!(terminal["result"]["resultType"], "complete");
        assert_eq!(terminal["result"]["structuredContent"], value);
        assert_eq!(terminal["result"]["content"][0]["text"], value.to_string());
    }
    shutdown.cancel();
    timeout(std::time::Duration::from_secs(4), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

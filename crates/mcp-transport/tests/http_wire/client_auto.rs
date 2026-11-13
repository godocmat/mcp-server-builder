use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use mcp_transport::http::LoopbackProfile;
use rmcp::model::{CallToolRequestParams, ErrorCode, ProtocolVersion};
use rmcp::service::{ClientInitializeError, RunningService};
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::{ClientLifecycleMode, ClientServiceExt, RoleClient};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::time::timeout;

use super::client::{LEGACY, MODERN};
use super::fixture::Server;

const FUTURE: &str = "2099-01-01";
const WAIT: Duration = Duration::from_secs(5);

#[tokio::test]
async fn auto_retries_the_real_modern_endpoint_at_a_supported_version() {
    let server = Server::start(LoopbackProfile::ModernOnly, None).await;
    let client = connect(server.address, vec![version(FUTURE), version(MODERN)])
        .await
        .unwrap();
    assert_eq!(
        client.peer_info().unwrap().protocol_version.as_str(),
        MODERN
    );
    let listed = timeout(WAIT, client.list_tools(None))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(listed.tools[0].name, "lookup_item");
    let request = CallToolRequestParams::new("lookup_item")
        .with_arguments([("id".to_owned(), json!("42"))].into_iter().collect());
    let result = timeout(WAIT, client.call_tool(request))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.structured_content, Some(json!({"id":"42"})));
    close(client).await;

    let error = connect(server.address, vec![version(FUTURE)])
        .await
        .unwrap_err();
    let ClientInitializeError::NoCompatibleProtocolVersion {
        client_supported,
        server_supported,
    } = error
    else {
        panic!("expected incompatible modern versions, got {error:?}");
    };
    assert_eq!(client_supported, [version(FUTURE)]);
    assert_eq!(server_supported, [version(MODERN)]);
    server.stop().await;
}

#[derive(Clone, Copy)]
enum ProbeReply {
    Empty,
    Unrecognized,
    HeaderMismatch,
    MissingCapability,
}

#[tokio::test]
async fn auto_falls_back_for_legacy_400s_and_surfaces_recognized_modern_errors() {
    for scenario in [
        ProbeReply::Empty,
        ProbeReply::Unrecognized,
        ProbeReply::HeaderMismatch,
        ProbeReply::MissingCapability,
    ] {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let router = Router::new()
            .route("/mcp", any(raw_peer))
            .with_state((scenario, requests.clone()));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, router).await });
        let result = connect(address, vec![version(MODERN)]).await;
        match scenario {
            ProbeReply::Empty | ProbeReply::Unrecognized => {
                let client = result.unwrap();
                assert_eq!(
                    client.peer_info().unwrap().protocol_version.as_str(),
                    LEGACY
                );
                assert!(
                    timeout(WAIT, client.list_tools(None))
                        .await
                        .unwrap()
                        .unwrap()
                        .tools
                        .is_empty()
                );
                close(client).await;
                assert_eq!(
                    requests.lock().unwrap().as_slice(),
                    &[
                        "server/discover",
                        "initialize",
                        "notifications/initialized",
                        "tools/list"
                    ],
                );
            }
            ProbeReply::HeaderMismatch | ProbeReply::MissingCapability => {
                let code = match scenario {
                    ProbeReply::HeaderMismatch => ErrorCode::HEADER_MISMATCH,
                    _ => ErrorCode::MISSING_REQUIRED_CLIENT_CAPABILITY,
                };
                let error = result.unwrap_err();
                let ClientInitializeError::JsonRpcError(error) = error else {
                    panic!("expected a recognized modern rejection, got {error:?}");
                };
                assert_eq!(error.code, code);
                if code == ErrorCode::MISSING_REQUIRED_CLIENT_CAPABILITY {
                    assert_eq!(
                        error.data.unwrap()["requiredCapabilities"],
                        json!({"sampling":{}})
                    );
                }
                assert_eq!(requests.lock().unwrap().as_slice(), &["server/discover"]);
            }
        }
        server.abort();
        assert!(server.await.unwrap_err().is_cancelled());
    }
}

fn version(value: &str) -> ProtocolVersion {
    serde_json::from_value(json!(value)).unwrap()
}

async fn connect(
    address: SocketAddr,
    preferred_versions: Vec<ProtocolVersion>,
) -> Result<RunningService<RoleClient, ()>, ClientInitializeError> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .pool_max_idle_per_host(0)
        .timeout(WAIT)
        .build()
        .unwrap();
    let config = StreamableHttpClientTransportConfig::with_uri(format!("http://{address}/mcp"));
    let transport = StreamableHttpClientTransport::with_client(client, config);
    timeout(
        WAIT,
        ().serve_with_lifecycle(
            transport,
            ClientLifecycleMode::Auto {
                preferred_versions,
                legacy_version: Some(version(LEGACY)),
            },
        ),
    )
    .await
    .expect("HTTP client startup timed out")
}

async fn close(mut client: RunningService<RoleClient, ()>) {
    assert!(client.close_with_timeout(WAIT).await.unwrap().is_some());
}

type RawState = (ProbeReply, Arc<Mutex<Vec<String>>>);

async fn raw_peer(State((scenario, requests)): State<RawState>, request: Request) -> Response {
    if request.method() != axum::http::Method::POST {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let (parts, body) = request.into_parts();
    let bytes = axum::body::to_bytes(body, 1024 * 1024).await.unwrap();
    let request: Value = serde_json::from_slice(&bytes).unwrap();
    let method = request["method"].as_str().unwrap();
    requests.lock().unwrap().push(method.to_owned());
    let headers = parts.headers;
    match method {
        "server/discover" => {
            assert_eq!(headers["mcp-protocol-version"], MODERN);
            assert_eq!(headers["mcp-method"], method);
            assert_eq!(
                request["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"],
                MODERN
            );
            assert!(
                request["params"]["_meta"]["io.modelcontextprotocol/clientCapabilities"]
                    .is_object()
            );
            let error = match scenario {
                ProbeReply::Empty => return StatusCode::BAD_REQUEST.into_response(),
                ProbeReply::Unrecognized => json!({"code":-32601,"message":"Unknown method"}),
                ProbeReply::HeaderMismatch => json!({"code":-32020,"message":"Routing mismatch"}),
                ProbeReply::MissingCapability => json!({
                    "code":-32021,"message":"Sampling required",
                    "data":{"requiredCapabilities":{"sampling":{}}}
                }),
            };
            json_reply(
                StatusCode::BAD_REQUEST,
                json!({"jsonrpc":"2.0","id":request["id"],"error":error}),
            )
        }
        "initialize" => {
            assert_eq!(request["params"]["protocolVersion"], LEGACY);
            json_reply(
                StatusCode::OK,
                json!({
                    "jsonrpc":"2.0","id":request["id"],"result":{
                        "protocolVersion":LEGACY,"capabilities":{"tools":{}},
                        "serverInfo":{"name":"legacy-fixture","version":"1.0.0"}
                    }
                }),
            )
        }
        "notifications/initialized" => {
            assert_eq!(headers["mcp-protocol-version"], LEGACY);
            StatusCode::ACCEPTED.into_response()
        }
        "tools/list" => {
            assert_eq!(headers["mcp-protocol-version"], LEGACY);
            json_reply(
                StatusCode::OK,
                json!({"jsonrpc":"2.0","id":request["id"],"result":{"tools":[]}}),
            )
        }
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

fn json_reply(status: StatusCode, value: Value) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "application/json")],
        value.to_string(),
    )
        .into_response()
}

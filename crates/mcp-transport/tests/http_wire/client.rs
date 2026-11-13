use std::net::SocketAddr;
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, Method, Request, Response, header};
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::task::JoinHandle;

pub(super) const MODERN: &str = "2026-07-28";
pub(super) const LEGACY: &str = "2025-11-25";
pub(super) const VERSION: &str = "mcp-protocol-version";
pub(super) const SESSION: &str = "mcp-session-id";
const WAIT: Duration = Duration::from_secs(4);
const MAX_BODY: usize = 1024 * 1024;

pub(super) fn rpc(id: impl Into<Value>, method: &str, mut params: Value, modern: bool) -> Value {
    if modern {
        params["_meta"]["io.modelcontextprotocol/protocolVersion"] = json!(MODERN);
        params["_meta"]["io.modelcontextprotocol/clientCapabilities"] = json!({});
    }
    json!({"jsonrpc":"2.0","id":id.into(),"method":method,"params":params})
}

pub(super) fn tool(
    id: impl Into<Value>,
    item: Value,
    modern: bool,
    progress: Option<&str>,
) -> Value {
    let mut request = rpc(
        id,
        "tools/call",
        json!({"name":"lookup_item","arguments":{"id":item}}),
        modern,
    );
    if let Some(progress) = progress {
        request["params"]["_meta"]["progressToken"] = json!(progress);
    }
    request
}

pub(super) fn initialize() -> Value {
    rpc(
        1,
        "initialize",
        json!({
            "protocolVersion": LEGACY,
            "capabilities": {},
            "clientInfo": {"name":"independent-http-fixture","version":"1.0.0"}
        }),
        false,
    )
}

pub(super) fn notification(method: &str, params: Option<Value>) -> Value {
    let mut request = json!({"jsonrpc":"2.0","method":method});
    if let Some(params) = params {
        request["params"] = params;
    }
    request
}

pub(super) fn request(address: SocketAddr, method: Method, body: Body) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri("/mcp")
        .header(header::HOST, address.to_string())
        .header(header::ACCEPT, "application/json, text/event-stream")
        .header(header::CONTENT_TYPE, "application/json")
        .body(body)
        .unwrap()
}

pub(super) fn post(
    address: SocketAddr,
    message: &Value,
    modern: bool,
    session: Option<&str>,
) -> Request<Body> {
    let mut request = request(
        address,
        Method::POST,
        Body::from(serde_json::to_vec(message).unwrap()),
    );
    let headers = request.headers_mut();
    if modern {
        headers.insert(VERSION, MODERN.parse().unwrap());
        let method = message["method"].as_str().unwrap();
        headers.insert("mcp-method", method.parse().unwrap());
        let name = match method {
            "tools/call" | "prompts/get" => message["params"]["name"].as_str(),
            "resources/read" => message["params"]["uri"].as_str(),
            _ => None,
        };
        if let Some(name) = name {
            headers.insert("mcp-name", name.parse().unwrap());
        }
    } else if message["method"] != "initialize" {
        headers.insert(VERSION, LEGACY.parse().unwrap());
    }
    if let Some(session) = session {
        headers.insert(SESSION, session.parse().unwrap());
    }
    request
}

pub(super) struct Reply {
    message: Response<Incoming>,
    connection: JoinHandle<Result<(), hyper::Error>>,
    events: Vec<u8>,
}

impl Drop for Reply {
    fn drop(&mut self) {
        self.connection.abort();
    }
}

pub(super) async fn send(address: SocketAddr, request: Request<Body>) -> Reply {
    let tcp = tokio::time::timeout(WAIT, TcpStream::connect(address))
        .await
        .unwrap()
        .unwrap();
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(tcp))
        .await
        .unwrap();
    let connection = tokio::spawn(connection);
    let message = tokio::time::timeout(WAIT, sender.send_request(request))
        .await
        .unwrap()
        .unwrap();
    Reply {
        message,
        connection,
        events: Vec::new(),
    }
}

impl Reply {
    pub(super) fn status(&self) -> u16 {
        self.message.status().as_u16()
    }

    pub(super) fn headers(&self) -> &HeaderMap {
        self.message.headers()
    }

    pub(super) fn is_sse(&self) -> bool {
        self.headers()
            .get(header::CONTENT_TYPE)
            .is_some_and(|value| value == "text/event-stream")
    }

    async fn frame(&mut self) -> Option<Bytes> {
        loop {
            let frame = tokio::time::timeout(WAIT, self.message.body_mut().frame())
                .await
                .expect("response frame timed out")?
                .unwrap();
            if let Ok(bytes) = frame.into_data() {
                return Some(bytes);
            }
        }
    }

    pub(super) async fn bytes(&mut self) -> Vec<u8> {
        let mut body = Vec::new();
        while let Some(frame) = self.frame().await {
            assert!(
                frame.len() <= MAX_BODY - body.len(),
                "HTTP response exceeded test bound"
            );
            body.extend_from_slice(&frame);
        }
        body
    }

    pub(super) async fn event(&mut self) -> Option<Value> {
        loop {
            if let Some(boundary) = self.events.windows(2).position(|pair| pair == b"\n\n") {
                let event = std::str::from_utf8(&self.events[..boundary]).unwrap();
                let data = event
                    .lines()
                    .filter_map(|line| {
                        line.strip_prefix("data: ")
                            .or_else(|| line.strip_prefix("data:"))
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                self.events.drain(..boundary + 2);
                if !data.is_empty() {
                    return Some(serde_json::from_str(&data).unwrap());
                }
                continue;
            }
            let Some(frame) = self.frame().await else {
                assert!(self.events.is_empty(), "incomplete SSE event");
                return None;
            };
            assert!(
                frame.len() <= MAX_BODY - self.events.len(),
                "SSE event exceeded test bound"
            );
            self.events.extend_from_slice(&frame);
        }
    }

    pub(super) async fn terminal(&mut self) -> Value {
        if !self.is_sse() {
            return serde_json::from_slice(&self.bytes().await).unwrap();
        }
        while let Some(event) = self.event().await {
            if event.get("id").is_some() {
                return event;
            }
        }
        panic!("SSE ended before terminal result");
    }
}

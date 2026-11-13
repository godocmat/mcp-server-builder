use std::convert::Infallible;

use axum::body::{Body, Bytes};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::Response;
use futures_util::StreamExt;
use rmcp::model::ErrorCode;
use serde_json::{Value, json};

use super::LoopbackProfile;
use super::request::MAX_BODY_BYTES;

pub(super) fn plain(status: StatusCode) -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = status;
    response
}

pub(super) fn method_not_allowed(profile: LoopbackProfile) -> Response {
    let mut response = plain(StatusCode::METHOD_NOT_ALLOWED);
    let allowed = if profile.issues_sessions() {
        "POST, DELETE"
    } else {
        "POST"
    };
    response
        .headers_mut()
        .insert(header::ALLOW, HeaderValue::from_static(allowed));
    response
}

pub(super) fn rpc(
    status: StatusCode,
    id: Option<Value>,
    code: ErrorCode,
    message: &str,
    data: Option<Value>,
) -> Response {
    let bytes = rpc_bytes(id, code, message, data);
    let mut response = Response::new(Body::from(bytes));
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

fn rpc_bytes(id: Option<Value>, code: ErrorCode, message: &str, data: Option<Value>) -> Vec<u8> {
    let mut payload = json!({"jsonrpc": "2.0", "error": {"code": code, "message": message}});
    if let Some(id) = id {
        payload["id"] = id;
    }
    if let Some(data) = data {
        payload["error"]["data"] = data;
    }
    serde_json::to_vec(&payload).expect("fixed error is serializable")
}

const TOO_LARGE: &str = "MCP response exceeds its byte limit.";

pub(super) async fn bounded<B>(reply: Response<B>, id: Option<Value>) -> Response
where
    B: axum::body::HttpBody<Data = Bytes> + Send + 'static,
    B::Error: Into<axum::BoxError>,
{
    let (mut parts, body) = reply.into_parts();
    let body = Body::new(body);
    match parts
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
    {
        Some("application/json") => {
            let Ok(bytes) = axum::body::to_bytes(body, MAX_BODY_BYTES).await else {
                return rpc(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    id,
                    ErrorCode::INTERNAL_ERROR,
                    TOO_LARGE,
                    None,
                );
            };
            // The SDK can quote invalid routing values. Leave the shared runtime's
            // namespaced diagnostics untouched and replace the transport message.
            if parts.status == StatusCode::BAD_REQUEST
                && let Ok(message) = serde_json::from_slice::<Value>(&bytes)
                && message["error"]["code"].as_i64() == Some(-32020)
            {
                let replacement = rpc(
                    parts.status,
                    id,
                    ErrorCode::HEADER_MISMATCH,
                    "Invalid or mismatched MCP routing header.",
                    None,
                );
                parts.headers.remove(header::CONTENT_LENGTH);
                return Response::from_parts(parts, replacement.into_body());
            }
            Response::from_parts(parts, Body::from(bytes))
        }
        Some("text/event-stream") => {
            // The pinned SDK emits one complete SSE event per body data frame.
            // Keep the body unbuffered so disconnect reaches the SDK stream guard.
            let chunks = body.into_data_stream();
            let body = futures_util::stream::unfold(Some((chunks, id)), |state| async move {
                let (mut chunks, id) = state?;
                match chunks.next().await? {
                    Ok(frame) if frame.len() <= MAX_BODY_BYTES => {
                        Some((Ok::<_, Infallible>(frame), Some((chunks, id))))
                    }
                    Ok(_) => {
                        let json = rpc_bytes(id, ErrorCode::INTERNAL_ERROR, TOO_LARGE, None);
                        let mut event = b"data: ".to_vec();
                        event.extend(json);
                        event.extend(b"\n\n");
                        Some((Ok(event.into()), None))
                    }
                    Err(_) => None,
                }
            });
            parts.headers.remove(header::CONTENT_LENGTH);
            Response::from_parts(parts, Body::from_stream(body))
        }
        _ => Response::from_parts(parts, Body::empty()),
    }
}

//! Streamable HTTP engineering server bound to an ephemeral IPv4 loopback port.

mod request;
mod response;

use std::convert::Infallible;
use std::future::{Future, ready};
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{Method, StatusCode, header};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use futures_util::{StreamExt, stream};
use mcp_runtime::Runtime;
use rmcp::RoleServer;
use rmcp::model::{InitializeRequestParams, ServerJsonRpcMessage};
use rmcp::service::serve_directly_with_ct;
use rmcp::transport::OneshotTransport;
use rmcp::transport::streamable_http_server::session::SessionManager;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::handler::{LEGACY, LocalHandler, MODERN};
use request::{MAX_BODY_BYTES, Routed, Stream, VERSION_HEADER, one_header};

const SESSION_HEADER: &str = "mcp-session-id";
const SESSION_LIMIT: usize = 128;
const DEFAULT_IDLE: Duration = Duration::from_secs(15 * 60);
const BODY_READ_TIMEOUT: Duration = Duration::from_secs(10);

/// The two supported server profiles and the legacy comparison used in M4.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoopbackProfile {
    DualRevision,
    ModernOnly,
    SessionlessComparison,
}

impl LoopbackProfile {
    fn issues_sessions(self) -> bool {
        self == Self::DualRevision
    }

    fn allows_legacy(self) -> bool {
        self != Self::ModernOnly
    }
}

/// Loopback settings. The default dual-revision profile uses issued legacy sessions.
#[derive(Clone, Copy, Debug)]
pub struct LoopbackOptions {
    pub profile: LoopbackProfile,
    pub legacy_idle_timeout: Duration,
}

impl Default for LoopbackOptions {
    fn default() -> Self {
        Self {
            profile: LoopbackProfile::DualRevision,
            legacy_idle_timeout: DEFAULT_IDLE,
        }
    }
}

/// Bind `127.0.0.1:0` and return its address and server future for the `/mcp` endpoint.
///
/// The caller supplies an approved runtime and trusted fixture operator. Apply
/// [`crate::sdk_diagnostic_filter`] to tracing layers before polling the server.
pub async fn bind_loopback(
    runtime: Arc<Runtime>,
    operator: String,
    options: LoopbackOptions,
    shutdown: CancellationToken,
) -> io::Result<(
    SocketAddr,
    impl Future<Output = io::Result<()>> + Send + use<>,
)> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let address = listener.local_addr()?;
    let shutdown = shutdown.child_token();
    let state = Endpoint::new(runtime, operator, options, address, shutdown.clone());
    let router = Router::new()
        .route("/mcp", any(handle))
        .with_state(Arc::new(state));
    Ok((address, async move {
        let _stop = shutdown.clone().drop_guard();
        axum::serve(listener, router)
            .with_graceful_shutdown(shutdown.cancelled_owned())
            .await
    }))
}

type SdkService = StreamableHttpService<LocalHandler, LocalSessionManager>;

struct Endpoint {
    profile: LoopbackProfile,
    handler: LocalHandler,
    sdk: SdkService,
    sessions: Arc<LocalSessionManager>,
    initialize: Mutex<()>,
    authority: String,
    origin: String,
    shutdown: CancellationToken,
}

impl Endpoint {
    fn new(
        runtime: Arc<Runtime>,
        operator: String,
        options: LoopbackOptions,
        address: SocketAddr,
        shutdown: CancellationToken,
    ) -> Self {
        let mut sessions = LocalSessionManager::default();
        sessions.session_config.keep_alive = Some(options.legacy_idle_timeout);
        sessions.session_config.sse_retry = None;
        sessions.session_config.completed_cache_ttl = Duration::ZERO;
        let sessions = Arc::new(sessions);
        let mut handler = LocalHandler::new(runtime, operator);
        if options.profile == LoopbackProfile::ModernOnly {
            handler = handler.modern_only();
        }
        let authority = address.to_string();
        let origin = format!("http://{authority}");
        let config = StreamableHttpServerConfig::default()
            .with_allowed_hosts([authority.clone()])
            .with_allowed_origins([origin.clone()])
            .with_legacy_session_mode(options.profile.issues_sessions())
            .with_stateless_protocol_metadata_required(
                options.profile == LoopbackProfile::ModernOnly,
            )
            .with_json_response(true)
            .with_max_request_body_bytes(MAX_BODY_BYTES)
            .with_sse_retry(None)
            .with_cancellation_token(shutdown.clone());
        let service_handler = handler.clone();
        let sdk = StreamableHttpService::new(
            move || Ok(service_handler.clone()),
            sessions.clone(),
            config,
        );
        Self {
            profile: options.profile,
            handler,
            sdk,
            sessions,
            initialize: Mutex::new(()),
            authority,
            origin,
            shutdown,
        }
    }

    async fn close_session(&self, request: Request) -> Response {
        let (parts, _) = request.into_parts();
        let headers = &parts.headers;
        let Ok(Some(version)) = one_header(headers, VERSION_HEADER) else {
            return response::plain(StatusCode::BAD_REQUEST);
        };
        if version == MODERN.as_str() {
            return response::method_not_allowed(self.profile);
        }
        if version != LEGACY.as_str() {
            return response::plain(StatusCode::BAD_REQUEST);
        }
        let Ok(Some(id)) = one_header(headers, SESSION_HEADER) else {
            return response::plain(StatusCode::BAD_REQUEST);
        };
        if !request::valid_session_id(id) {
            return response::plain(StatusCode::BAD_REQUEST);
        }
        let id = id.to_owned().into();
        if !self.sessions.has_session(&id).await.unwrap_or(false) {
            return response::plain(StatusCode::NOT_FOUND);
        }
        if self.sessions.close_session(&id).await.is_err() {
            return response::plain(StatusCode::INTERNAL_SERVER_ERROR);
        }
        response::plain(StatusCode::ACCEPTED)
    }

    async fn post(&self, request: Request) -> Response {
        let (parts, body) = request.into_parts();
        if !request::accepts_responses(&parts.headers) {
            return response::plain(StatusCode::NOT_ACCEPTABLE);
        }
        if !matches!(
            one_header(&parts.headers, header::CONTENT_TYPE.as_str()),
            Ok(Some(value))
                if value.split(';').next().is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"))
        ) {
            return response::plain(StatusCode::UNSUPPORTED_MEDIA_TYPE);
        }
        let bytes = match tokio::time::timeout(
            BODY_READ_TIMEOUT,
            axum::body::to_bytes(body, MAX_BODY_BYTES),
        )
        .await
        {
            Ok(Ok(bytes)) => bytes,
            Ok(Err(error))
                if std::error::Error::source(&error)
                    .is_some_and(|source| source.is::<http_body_util::LengthLimitError>()) =>
            {
                return response::plain(StatusCode::PAYLOAD_TOO_LARGE);
            }
            Ok(Err(_)) => return response::plain(StatusCode::BAD_REQUEST),
            Err(_) => return response::plain(StatusCode::REQUEST_TIMEOUT),
        };
        let routed = match request::validate(&parts.headers, &bytes, self.profile) {
            Ok(routed) => routed,
            Err(error) => return error.into_response(),
        };
        if routed.stream == Stream::Modern {
            return self.stream_modern(routed).await;
        }

        // Serialize only session creation, leaving existing sessions and stateless work independent.
        let _initialize = if routed.initialize && self.profile.issues_sessions() {
            let guard = self.initialize.lock().await;
            if self.sessions.sessions.read().await.len() >= SESSION_LIMIT {
                return response::plain(StatusCode::SERVICE_UNAVAILABLE);
            }
            Some(guard)
        } else {
            None
        };
        let mut sdk = self.sdk.clone();
        sdk.config.json_response = routed.stream == Stream::None;
        let message =
            serde_json::to_vec(&routed.message).expect("typed MCP message is serializable");
        let mut parts = parts;
        parts.headers.remove(header::CONTENT_LENGTH);
        let request = Request::from_parts(parts, Body::from(message));
        let reply = sdk.handle(request).await;
        response::bounded(reply, routed.id).await
    }

    async fn stream_modern(&self, routed: Routed) -> Response {
        // The negotiated HTTP service waits for the first MCP message to choose
        // HTTP status. For validated adapter requests the stream can open now,
        // including when the client did not ask for progress.
        let (transport, receiver) = OneshotTransport::<RoleServer>::new(routed.message);
        let peer_info = InitializeRequestParams::new(Default::default(), Default::default())
            .with_protocol_version(MODERN);
        let service = serve_directly_with_ct(
            self.handler.clone(),
            transport,
            Some(peer_info),
            self.shutdown.child_token(),
        );
        // RunningService cancels its request when the response body drops.
        let messages = stream::unfold(Some((receiver, service)), |state| async move {
            let (mut receiver, service) = state?;
            let message = receiver.recv().await?;
            let terminal = matches!(
                &message,
                ServerJsonRpcMessage::Response(_) | ServerJsonRpcMessage::Error(_)
            );
            let data = serde_json::to_string(&message).expect("typed MCP response is serializable");
            Some((
                Ok::<_, Infallible>(Event::default().data(data)),
                (!terminal).then_some((receiver, service)),
            ))
        });
        let comment = stream::once(ready(Ok(Event::default().comment(""))));
        let mut reply = Sse::new(comment.chain(messages)).into_response();
        reply.headers_mut().insert(
            "x-accel-buffering",
            axum::http::HeaderValue::from_static("no"),
        );
        response::bounded(reply, routed.id).await
    }
}

async fn handle(State(endpoint): State<Arc<Endpoint>>, request: Request) -> Response {
    let headers = request.headers();
    let origin_is_allowed = match one_header(headers, header::ORIGIN.as_str()) {
        Ok(None) => true,
        Ok(Some(origin)) => origin == endpoint.origin,
        Err(()) => false,
    };
    if !matches!(one_header(headers, header::HOST.as_str()), Ok(Some(host)) if host == endpoint.authority)
        || !origin_is_allowed
    {
        return response::plain(StatusCode::FORBIDDEN);
    }
    tokio::select! {
        biased;
        _ = endpoint.shutdown.cancelled() => response::plain(StatusCode::SERVICE_UNAVAILABLE),
        response = async {
            match *request.method() {
                Method::POST => endpoint.post(request).await,
                Method::DELETE if endpoint.profile.issues_sessions() => endpoint.close_session(request).await,
                _ => response::method_not_allowed(endpoint.profile),
            }
        } => response,
    }
}

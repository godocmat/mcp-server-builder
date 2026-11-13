use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
use http::header::{ACCEPT_ENCODING, CONNECTION, CONTENT_TYPE, HOST};
use http::{HeaderMap, Request, Response};
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::client::conn::http1;
use hyper_util::rt::TokioIo;
use mcp_policy::{Effect, HttpOrigin, RestRoute};
use mcp_runtime::AdapterError;
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, RootCertStore};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::task::JoinHandle;
use tokio_rustls::TlsConnector;
use tracing::instrument::WithSubscriber;

use crate::{SetupError, failed};

pub(super) struct Trust {
    public: Arc<ClientConfig>,
    private: Vec<(HttpOrigin, Arc<ClientConfig>)>,
}

impl Trust {
    pub(super) fn new(private: Vec<(HttpOrigin, Vec<Vec<u8>>)>) -> Result<Self, SetupError> {
        let mut origins = Vec::new();
        for (origin, roots) in private {
            if origin.scheme() != "https"
                || roots.is_empty()
                || origins.iter().any(|(known, _)| known == &origin)
            {
                return Err(SetupError::InvalidTrust);
            }
            let mut store = RootCertStore {
                roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
            };
            for certificate in roots {
                store
                    .add(CertificateDer::from(certificate))
                    .map_err(|_| SetupError::InvalidTrust)?;
            }
            origins.push((origin, tls_config(store)?));
        }
        Ok(Self {
            public: tls_config(RootCertStore {
                roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
            })?,
            private: origins,
        })
    }

    pub(super) async fn connect(
        &self,
        socket: TcpStream,
        origin: &HttpOrigin,
    ) -> Result<Connection, AdapterError> {
        if origin.scheme() == "http" {
            return Connection::handshake(socket).await;
        }
        let config = self
            .private
            .iter()
            .find(|(known, _)| known == origin)
            .map_or(&self.public, |(_, config)| config);
        let name =
            ServerName::try_from(origin.host().to_owned()).map_err(|_| failed(Effect::None))?;
        let socket = TlsConnector::from(Arc::clone(config))
            .connect(name, socket)
            .await
            .map_err(|_| failed(Effect::None))?;
        Connection::handshake(socket).await
    }
}

fn tls_config(roots: RootCertStore) -> Result<Arc<ClientConfig>, SetupError> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|_| SetupError::InvalidTrust)?
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    config.resumption = rustls::client::Resumption::disabled();
    Ok(Arc::new(config))
}

pub(super) async fn dial(peer: SocketAddr) -> io::Result<(TcpStream, SocketAddr)> {
    let socket = TcpStream::connect(peer).await?;
    let actual = socket.peer_addr()?;
    if actual.ip().to_canonical() != peer.ip().to_canonical()
        || actual.port() != peer.port()
        || matches!((actual, peer), (SocketAddr::V6(actual), SocketAddr::V6(expected)) if actual.scope_id() != expected.scope_id())
    {
        return Err(io::Error::other("unexpected connected address"));
    }
    Ok((socket, actual))
}

pub(super) struct Connection {
    sender: http1::SendRequest<Full<Bytes>>,
    driver: JoinHandle<()>,
}

impl Connection {
    async fn handshake(
        socket: impl AsyncRead + AsyncWrite + Unpin + Send + 'static,
    ) -> Result<Self, AdapterError> {
        let (sender, connection) = http1::Builder::new()
            .max_headers(64)
            .max_header_size(16 * 1024)
            .max_buf_size(32 * 1024)
            .handshake(TokioIo::new(socket))
            .await
            .map_err(|_| failed(Effect::None))?;
        // Hyper tracing can be unified by the embedding server. Raw upstream
        // headers and parse failures are outside the evidence boundary.
        let driver = tokio::spawn(
            async move {
                let _ = connection.await;
            }
            .with_subscriber(tracing::subscriber::NoSubscriber::default()),
        );
        Ok(Self { sender, driver })
    }

    pub(super) async fn send(
        &mut self,
        request: Request<Full<Bytes>>,
    ) -> Result<Response<Incoming>, hyper::Error> {
        self.sender.send_request(request).await
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.driver.abort();
    }
}

pub(super) fn request(
    route: &RestRoute,
    body: Option<Bytes>,
    credentials: &HeaderMap,
) -> Result<Request<Full<Bytes>>, AdapterError> {
    let mut request = Request::builder()
        .method(route.method.as_str())
        .uri(&route.path)
        .header(HOST, route.origin.authority())
        .header(CONNECTION, "close")
        .header(ACCEPT_ENCODING, "identity");
    if body.is_some() && !credentials.contains_key(CONTENT_TYPE) {
        request = request.header(CONTENT_TYPE, "application/json");
    }
    let mut request = request
        .body(Full::new(body.unwrap_or_default()))
        .map_err(|_| failed(Effect::None))?;
    request.headers_mut().extend(credentials.clone());
    Ok(request)
}

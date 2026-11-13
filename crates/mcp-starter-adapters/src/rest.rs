mod connection;
mod credentials;

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Instant;

use bytes::Bytes;
use fluent_uri::UriRef;
use http::header::{CONTENT_ENCODING, CONTENT_TYPE, LOCATION};
use http::{HeaderMap, Response, StatusCode, Uri};
use http_body_util::BodyExt;
use hyper::body::{Body, Incoming};
use mcp_policy::{
    DenyReason, Effect, ExecutionLimits, HttpMethod, HttpOrigin, RestRoute, VerifiedTarget,
};
use mcp_runtime::bindings::{RestBinding, credential_header};
use mcp_runtime::syntax::{SegmentTemplate, expand_path, render_path, segments, variables};
use mcp_runtime::{
    AdapterContext, AdapterError, AdapterOutput, BoxFuture, PreparedCall, PreparedTarget,
};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::{Map, Value};
use tokio::sync::Semaphore;
use uuid::Uuid;

use crate::{
    CancelPending, SecretRequest, SecretResolver, SetupError, denied, failed, invalid_arguments,
    output_limit,
};

const MAX_PATH_BYTES: usize = 2048;
const MAX_PEERS: usize = 16;
const MAX_REDIRECTS: usize = 4;
const SEGMENT_ESCAPE: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

pub(super) struct Binding {
    config: RestBinding,
    path: Vec<SegmentTemplate>,
}

impl Binding {
    pub(super) fn new(config: RestBinding) -> Result<Self, SetupError> {
        let path = segments(&config.path_template)
            .filter(|parts| {
                safe_path(&expand_path(parts))
                    && variables(parts)
                        .into_iter()
                        .eq(config.path_params.keys().map(String::as_str))
            })
            .ok_or(SetupError::InvalidBinding)?;
        let mut names = BTreeSet::new();
        if !config.headers_from_secrets.is_empty() && config.origin.scheme() != "https" {
            return Err(SetupError::CredentialRequiresHttps);
        }
        if config.headers_from_secrets.len() > 512
            || !config.headers_from_secrets.iter().all(|(name, reference)| {
                credential_header(name.as_str()).is_some()
                    && names.insert(name.as_str())
                    && mcp_runtime::syntax::is_key(reference)
            })
        {
            return Err(SetupError::InvalidBinding);
        }
        Ok(Self { config, path })
    }
}

pub(super) struct Client {
    trust: connection::Trust,
    secrets: Option<Arc<dyn SecretResolver>>,
    dns_workers: Arc<Semaphore>,
    #[cfg(test)]
    resolver: Option<fn(&str, u16) -> Vec<SocketAddr>>,
}

impl Client {
    pub(super) fn new(
        private_cas: Vec<(HttpOrigin, Vec<Vec<u8>>)>,
        secrets: Option<Arc<dyn SecretResolver>>,
    ) -> Result<Self, SetupError> {
        Ok(Self {
            trust: connection::Trust::new(private_cas)?,
            secrets,
            dns_workers: Arc::new(Semaphore::new(8)),
            #[cfg(test)]
            resolver: None,
        })
    }

    pub(super) fn prepare<'a>(
        &'a self,
        project_id: Uuid,
        capability_id: &'a str,
        binding_id: &'a str,
        binding: &'a Binding,
        arguments: &Map<String, Value>,
    ) -> Result<Box<dyn PreparedCall + 'a>, AdapterError> {
        let input = Value::Object(arguments.clone());
        let path = render_path(
            &binding.path,
            |name| {
                let value = input.pointer(binding.config.path_params.get(name)?)?;
                let value = match value {
                    Value::String(value) => Cow::Borrowed(value.as_str()),
                    Value::Number(_) | Value::Bool(_) => Cow::Owned(value.to_string()),
                    _ => return None,
                };
                if value.len() > MAX_PATH_BYTES {
                    return None;
                }
                Some(Cow::Owned(
                    utf8_percent_encode(&value, SEGMENT_ESCAPE).to_string(),
                ))
            },
            MAX_PATH_BYTES,
        )
        .filter(|path| safe_path(path))
        .ok_or_else(invalid_arguments)?;
        let body = binding
            .config
            .body_pointer
            .as_deref()
            .map(|pointer| {
                let value = input.pointer(pointer).ok_or_else(invalid_arguments)?;
                let bytes = serde_json::to_vec(value).map_err(|_| invalid_arguments())?;
                if bytes.len() as u64 > ExecutionLimits::MAX_INPUT_BYTES {
                    return Err(invalid_arguments());
                }
                Ok::<Bytes, AdapterError>(bytes.into())
            })
            .transpose()?;
        Ok(Box::new(Call {
            client: self,
            project_id,
            capability_id,
            binding_id,
            binding,
            route: RestRoute {
                origin: binding.config.origin.clone(),
                method: binding.config.method,
                path,
            },
            body,
        }))
    }

    async fn resolve(
        &self,
        origin: &HttpOrigin,
        context: &AdapterContext<'_>,
    ) -> Result<Vec<SocketAddr>, AdapterError> {
        if let Some(ip) = origin.literal_ip() {
            return Ok(vec![SocketAddr::new(ip, origin.port())]);
        }
        if matches!(origin.host(), "localhost" | "localhost.") {
            return Ok(vec![
                SocketAddr::from((Ipv4Addr::LOCALHOST, origin.port())),
                SocketAddr::from((Ipv6Addr::LOCALHOST, origin.port())),
            ]);
        }
        #[cfg(test)]
        if let Some(resolve) = self.resolver {
            return validate_addresses(resolve(origin.host(), origin.port()), origin.port());
        }
        let permit = Arc::clone(&self.dns_workers)
            .acquire_owned()
            .await
            .map_err(|_| failed(Effect::None))?;
        let host = origin.host().to_owned();
        let port = origin.port();
        let cancellation = context.cancellation().clone();
        let deadline = context.deadline();
        // A system lookup in progress cannot be interrupted. Keep the worker
        // slot until it completes and discard its late answers with the call.
        let lookup = tokio::task::spawn_blocking(move || {
            use std::net::ToSocketAddrs as _;

            let _permit = permit;
            if cancellation.is_cancelled() || Instant::now() >= deadline {
                return Err(failed(Effect::None));
            }
            // Keep resolver search suffixes outside the configured origin.
            let host = if host.ends_with('.') {
                host
            } else {
                format!("{host}.")
            };
            let answers = (host.as_str(), port)
                .to_socket_addrs()
                .map_err(|_| failed(Effect::None))?;
            validate_addresses(answers, port)
        });
        let _abort = CancelPending::new(&lookup);
        lookup.await.map_err(|_| failed(Effect::None))?
    }
}

fn validate_addresses(
    answers: impl IntoIterator<Item = SocketAddr>,
    port: u16,
) -> Result<Vec<SocketAddr>, AdapterError> {
    let mut peers = Vec::new();
    for answer in answers {
        let address = match answer {
            SocketAddr::V6(address) if address.ip().to_ipv4_mapped().is_some() => {
                SocketAddr::new(answer.ip().to_canonical(), answer.port())
            }
            _ => answer,
        };
        if !peers.contains(&address) {
            if peers.len() >= MAX_PEERS || address.port() != port {
                return Err(failed(Effect::None));
            }
            peers.push(address);
        }
    }
    if peers.is_empty() {
        return Err(failed(Effect::None));
    }
    Ok(peers)
}

struct Call<'a> {
    client: &'a Client,
    project_id: Uuid,
    capability_id: &'a str,
    binding_id: &'a str,
    binding: &'a Binding,
    route: RestRoute,
    body: Option<Bytes>,
}

impl PreparedCall for Call<'_> {
    fn target(&self) -> PreparedTarget<'_> {
        PreparedTarget::RestRoute(&self.route)
    }

    fn execute<'a>(
        self: Box<Self>,
        context: &'a AdapterContext<'_>,
    ) -> BoxFuture<'a, Result<AdapterOutput, AdapterError>>
    where
        Self: 'a,
    {
        Box::pin(async move { self.run(context).await })
    }
}

impl Call<'_> {
    async fn run(&self, context: &AdapterContext<'_>) -> Result<AdapterOutput, AdapterError> {
        let mut route = self.route.clone();
        let mut credentials = None;
        for hop in 0..=MAX_REDIRECTS {
            let redirected = hop != 0;
            let peers = self.client.resolve(&route.origin, context).await?;
            // A mixed answer fails closed. No socket is opened until all answers
            // satisfy the same route and peer policy.
            for peer in &peers {
                authorize(context, &route, *peer, redirected)?;
            }
            let mut stream = None;
            for peer in peers {
                if let Ok((socket, actual_peer)) = connection::dial(peer).await {
                    authorize(context, &route, actual_peer, redirected)?;
                    stream = Some((socket, actual_peer));
                    break;
                }
            }
            let (socket, peer) = stream.ok_or_else(|| failed(Effect::None))?;
            let mut connection = self.client.trust.connect(socket, &route.origin).await?;
            authorize(context, &route, peer, redirected)?;
            let credentials = match &mut credentials {
                Some(credentials) => credentials,
                None => credentials.insert(
                    credentials::resolve(
                        self.client.secrets.as_deref(),
                        self.binding.config.headers_from_secrets.as_slice(),
                        |reference| SecretRequest {
                            project_id: self.project_id,
                            principal: context.principal(),
                            capability_id: self.capability_id,
                            binding_id: self.binding_id,
                            origin: &self.route.origin,
                            reference,
                        },
                    )
                    .await?,
                ),
            };
            authorize(context, &route, peer, redirected)?;
            let request = connection::request(&route, self.body.clone(), &credentials.headers)?;
            let effect = if route.method.is_read() {
                Effect::None
            } else {
                Effect::Unknown
            };
            let response = connection.send(request).await.map_err(|_| failed(effect))?;
            let status = response.status();
            if matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308) {
                // Replaying a write or changing its method needs an idempotency
                // contract. Only unchanged reads may follow a redirect.
                if !route.method.is_read() || self.body.is_some() || hop == MAX_REDIRECTS {
                    return Err(denied(DenyReason::RedirectDenied, effect));
                }
                let Some(path) = redirect_path(&route, response.headers()) else {
                    return Err(denied(DenyReason::RedirectDenied, effect));
                };
                route.path = path;
                context
                    .preflight_redirect(&route)
                    .map_err(|_| failed(effect))?;
                continue;
            }
            if !status.is_success() {
                return Err(failed(effect));
            }
            return response_output(
                response,
                context,
                &credentials.redactor,
                route.method,
                effect,
            )
            .await;
        }
        Err(failed(Effect::None))
    }
}

fn authorize(
    context: &AdapterContext<'_>,
    route: &RestRoute,
    peer: SocketAddr,
    redirected: bool,
) -> Result<(), AdapterError> {
    if peer.port() != route.origin.port() {
        return Err(failed(Effect::None));
    }
    context
        .authorize_target(&VerifiedTarget::Rest {
            origin: &route.origin,
            method: route.method,
            path: &route.path,
            peer: peer.ip(),
            redirected,
        })
        .map_err(|_| failed(Effect::None))?;
    Ok(())
}

// Validate the exact origin-form bytes handed to HTTP. Validate before any URL
// normalization so dot segments and ambiguous encodings cannot be rewritten.
fn safe_path(path: &str) -> bool {
    path.len() <= MAX_PATH_BYTES
        && RestRoute::is_valid_path(path)
        && UriRef::parse(path).is_ok()
        && path
            .parse::<Uri>()
            .is_ok_and(|uri| uri.authority().is_none() && uri.path() == path)
}

fn redirect_path(route: &RestRoute, headers: &HeaderMap) -> Option<String> {
    let mut locations = headers.get_all(LOCATION).iter();
    let location = locations.next()?.to_str().ok()?;
    if locations.next().is_some() || location.is_empty() || location.len() > MAX_PATH_BYTES {
        return None;
    }
    let uri = UriRef::parse(location).ok()?;
    if uri.query().is_some() || uri.fragment().is_some() || uri.path().as_str().is_empty() {
        return None;
    }
    let path = uri.path().as_str();
    match (uri.scheme(), uri.authority()) {
        (Some(scheme), Some(authority)) => {
            let origin: HttpOrigin = format!("{scheme}://{authority}").parse().ok()?;
            (origin == route.origin && safe_path(path)).then(|| path.to_owned())
        }
        (None, None) => {
            let path = if path.starts_with('/') {
                path.to_owned()
            } else {
                let (directory, _) = route.path.rsplit_once('/')?;
                format!("{directory}/{path}")
            };
            safe_path(&path).then_some(path)
        }
        _ => None,
    }
}

async fn response_output(
    response: Response<Incoming>,
    context: &AdapterContext<'_>,
    redactor: &mcp_policy::Redactor,
    method: HttpMethod,
    effect: Effect,
) -> Result<AdapterOutput, AdapterError> {
    let no_body = method == HttpMethod::Head
        || matches!(
            response.status(),
            StatusCode::NO_CONTENT | StatusCode::RESET_CONTENT
        );
    if !no_body && response.headers().contains_key(CONTENT_ENCODING) {
        return Err(failed(effect));
    }
    let is_json = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            let media_type = value
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase();
            media_type == "application/json" || media_type.ends_with("+json")
        });
    let mut body = response.into_body();
    let mut budget = context.limits().output_budget();
    if !no_body
        && body
            .size_hint()
            .upper()
            .is_some_and(|bytes| bytes > budget.remaining())
    {
        return Err(output_limit(effect));
    }
    let mut bytes = Vec::new();
    if !no_body {
        while let Some(frame) = body.frame().await {
            let frame = frame.map_err(|_| failed(effect))?;
            if let Ok(chunk) = frame.into_data() {
                budget
                    .consume(chunk.len())
                    .map_err(|_| output_limit(effect))?;
                bytes.extend_from_slice(&chunk);
            }
        }
    }
    let max = context.limits().max_output_bytes() as usize;
    let (text, structured_content) = if is_json && !bytes.is_empty() {
        let parsed: Value = serde_json::from_slice(&bytes).map_err(|_| failed(effect))?;
        let sanitized = redactor
            .redact_json(parsed, max)
            .map_err(|error| match error {
                mcp_policy::RedactionError::ByteLimitExceeded => output_limit(effect),
                _ => failed(effect),
            })?;
        match sanitized {
            Value::Object(fields) => (None, Some(fields)),
            value => (Some(value.to_string()), None),
        }
    } else {
        let text = std::str::from_utf8(&bytes).map_err(|_| failed(effect))?;
        let safe = redactor.redact_text(text, max);
        if safe.truncated {
            return Err(output_limit(effect));
        }
        (Some(safe.text), None)
    };
    Ok(AdapterOutput::Tool {
        text,
        structured_content,
        effect,
    })
}

#[cfg(test)]
mod tests;

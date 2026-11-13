use std::borrow::Cow;
use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr};

use builder_project::Fixture;
use mcp_policy::{Effect, HttpMethod, HttpOrigin, RestRoute, VerifiedTarget};
use mcp_runtime::bindings::{RestBinding, StarterBinding};
use mcp_runtime::syntax::{is_portable_path, is_portable_segment, render_path, segments};
use mcp_runtime::{
    Adapter, AdapterContext, AdapterError, AdapterErrorKind, AdapterOutput, BoxFuture, Invocation,
    InvocationInput, PreparationContext, PreparedCall, PreparedTarget,
};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::{Map, Value};

use super::json_fits;

const MAX_REST_PATH_BYTES: usize = 2048;
const MAX_FILE_PATH_BYTES: usize = 4096;
const SEGMENT_ESCAPE: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

pub(super) struct FixtureAdapter {
    pub(super) bindings: BTreeMap<String, StarterBinding>,
    pub(super) fixtures: Vec<Fixture>,
}

impl Adapter for FixtureAdapter {
    fn prepare<'a>(
        &'a self,
        invocation: Invocation<'a>,
        _context: &'a PreparationContext,
    ) -> BoxFuture<'a, Result<Box<dyn PreparedCall + 'a>, AdapterError>> {
        Box::pin(async move {
            let target = match (self.bindings.get(invocation.binding_id), invocation.input) {
                (Some(StarterBinding::Rest(binding)), InvocationInput::Tool(arguments)) => {
                    FixtureTarget::Rest {
                        origin: &binding.origin,
                        method: binding.method,
                        path: rest_path(binding, arguments)
                            .ok_or_else(|| failure(AdapterErrorKind::InvalidArguments))?,
                    }
                }
                (
                    Some(StarterBinding::FileRead(binding)),
                    InvocationInput::Resource { variables, .. },
                ) => {
                    let path = segments(&binding.relative_path_template)
                        .and_then(|parts| {
                            render_path(
                                &parts,
                                |name| {
                                    variables
                                        .get(name)
                                        .filter(|value| is_portable_segment(value))
                                        .map(|value| Cow::Borrowed(value.as_str()))
                                },
                                MAX_FILE_PATH_BYTES,
                            )
                        })
                        .filter(|path| is_portable_path(path))
                        .ok_or_else(|| failure(AdapterErrorKind::InvalidArguments))?;
                    FixtureTarget::File {
                        root_ref: &binding.root_ref,
                        path,
                    }
                }
                _ => return Err(failure(AdapterErrorKind::Failed)),
            };
            Ok(Box::new(FixtureCall {
                binding_id: invocation.binding_id,
                fixtures: &self.fixtures,
                target,
            }) as Box<dyn PreparedCall>)
        })
    }
}

fn rest_path(binding: &RestBinding, arguments: &Map<String, Value>) -> Option<String> {
    let arguments = Value::Object(arguments.clone());
    if let Some(pointer) = &binding.body_pointer {
        // The runtime has already bounded the whole argument object.
        arguments.pointer(pointer)?;
    }
    let parts = segments(&binding.path_template)?;
    render_path(
        &parts,
        |name| {
            let value = match arguments.pointer(binding.path_params.get(name)?)? {
                Value::String(value) => Cow::Borrowed(value.as_str()),
                value @ (Value::Number(_) | Value::Bool(_)) => Cow::Owned(value.to_string()),
                _ => return None,
            };
            if value.len() > MAX_REST_PATH_BYTES {
                return None;
            }
            Some(Cow::Owned(
                utf8_percent_encode(&value, SEGMENT_ESCAPE).to_string(),
            ))
        },
        MAX_REST_PATH_BYTES,
    )
    .filter(|path| RestRoute::is_valid_path(path))
}

enum FixtureTarget<'a> {
    Rest {
        origin: &'a HttpOrigin,
        method: HttpMethod,
        path: String,
    },
    File {
        root_ref: &'a str,
        path: String,
    },
}

struct FixtureCall<'a> {
    binding_id: &'a str,
    fixtures: &'a [Fixture],
    target: FixtureTarget<'a>,
}

impl PreparedCall for FixtureCall<'_> {
    fn target(&self) -> PreparedTarget<'_> {
        PreparedTarget::Verified(match &self.target {
            FixtureTarget::Rest {
                origin,
                method,
                path,
            } => VerifiedTarget::Rest {
                origin,
                method: *method,
                path,
                peer: synthetic_peer(origin),
                redirected: false,
            },
            FixtureTarget::File { root_ref, path } => VerifiedTarget::FileRead {
                root_ref,
                relative_path: path,
            },
        })
    }

    fn execute<'a>(
        self: Box<Self>,
        context: &'a AdapterContext<'_>,
    ) -> BoxFuture<'a, Result<AdapterOutput, AdapterError>>
    where
        Self: 'a,
    {
        Box::pin(async move {
            let fixture = self
                .fixtures
                .iter()
                .find(|fixture| match (&self.target, fixture) {
                    (
                        FixtureTarget::Rest { method, path, .. },
                        Fixture::Rest {
                            binding_id,
                            method: fixture_method,
                            path: fixture_path,
                            ..
                        },
                    ) => {
                        binding_id == self.binding_id
                            && fixture_method == method.as_str()
                            && fixture_path == path
                    }
                    (
                        FixtureTarget::File { path, .. },
                        Fixture::FileRead {
                            binding_id,
                            relative_path,
                            ..
                        },
                    ) => binding_id == self.binding_id && relative_path == path,
                    _ => false,
                })
                .ok_or_else(|| failure(AdapterErrorKind::FixtureMissing))?;
            let byte_limit = context.limits().max_output_bytes();
            match fixture {
                Fixture::Rest {
                    method,
                    status,
                    body,
                    ..
                } => rest_output(method, *status, body, byte_limit),
                Fixture::FileRead { text, .. } if text.len() as u64 <= byte_limit => {
                    Ok(AdapterOutput::Resource { text: text.clone() })
                }
                Fixture::FileRead { .. } => Err(failure(AdapterErrorKind::OutputLimit)),
            }
        })
    }
}

fn synthetic_peer(origin: &HttpOrigin) -> IpAddr {
    // A fixed modeled public peer for named origins retains peer restrictions
    // without resolving names. Numeric origins and localhost retain their class.
    origin.literal_ip().unwrap_or_else(|| {
        if matches!(origin.host(), "localhost" | "localhost.") {
            Ipv4Addr::LOCALHOST.into()
        } else {
            Ipv4Addr::new(1, 0, 0, 1).into()
        }
    })
}

fn rest_output(
    method: &str,
    status: u16,
    body: &Value,
    byte_limit: u64,
) -> Result<AdapterOutput, AdapterError> {
    if !(200..300).contains(&status) {
        return Err(failure(AdapterErrorKind::Failed));
    }
    // The saved body represents an application/json response. HEAD, 204 and 205
    // have the same empty text result as the live starter adapter.
    let (text, structured_content) = if method == "HEAD" || matches!(status, 204 | 205) {
        (Some(String::new()), None)
    } else {
        if !json_fits(body, byte_limit) {
            return Err(failure(AdapterErrorKind::OutputLimit));
        }
        match body {
            Value::Object(fields) => (None, Some(fields.clone())),
            value => (Some(value.to_string()), None),
        }
    };
    Ok(AdapterOutput::Tool {
        text,
        structured_content,
        effect: Effect::None,
    })
}

fn failure(kind: AdapterErrorKind) -> AdapterError {
    AdapterError {
        kind,
        effect: Effect::None,
    }
}

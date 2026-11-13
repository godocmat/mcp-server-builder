//! The live REST and file-read adapters for the transport-neutral runtime.

mod filesystem;
mod rest;

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use mcp_policy::{DenyReason, Effect, HttpOrigin};
use mcp_runtime::bindings::StarterBinding;
use mcp_runtime::{
    Adapter, AdapterError, AdapterErrorKind, BoxFuture, Invocation, InvocationInput,
    PreparationContext, PreparedCall, Principal,
};
use uuid::Uuid;
use zeroize::Zeroizing;

/// Context for lookup by an operator-owned credential provider. The provider must
/// enforce its selected local registration or deployment profile.
pub struct SecretRequest<'a> {
    pub project_id: Uuid,
    pub principal: &'a Principal<'a>,
    pub capability_id: &'a str,
    pub binding_id: &'a str,
    pub origin: &'a HttpOrigin,
    pub reference: &'a str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SecretError {
    Missing,
    Unavailable,
}

pub trait SecretResolver: Send + Sync {
    fn resolve<'a>(
        &'a self,
        request: SecretRequest<'a>,
    ) -> BoxFuture<'a, Result<Zeroizing<String>, SecretError>>;
}

/// Trusted inputs supplied by a reviewed launch or standalone runtime config.
#[derive(Default)]
pub struct LocalBindings {
    pub file_roots: BTreeMap<String, PathBuf>,
    /// Additional DER CA certificates for an exact HTTPS origin.
    pub private_cas: Vec<(HttpOrigin, Vec<Vec<u8>>)>,
    pub secrets: Option<Arc<dyn SecretResolver>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SetupError {
    InvalidBinding,
    InvalidTrust,
    FileRootUnavailable,
    CredentialRequiresHttps,
}

impl fmt::Display for SetupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidBinding => "adapter.invalid_binding",
            Self::InvalidTrust => "adapter.invalid_trust",
            Self::FileRootUnavailable => "adapter.file_root_unavailable",
            Self::CredentialRequiresHttps => "credential.https_required",
        })
    }
}

impl std::error::Error for SetupError {}

pub struct StarterAdapter {
    project_id: Uuid,
    bindings: BTreeMap<String, LiveBinding>,
    rest: rest::Client,
    files: filesystem::Files,
}

enum LiveBinding {
    Rest(Box<rest::Binding>),
    File(filesystem::Binding),
}

impl StarterAdapter {
    /// Construct an immutable live adapter after operator review. This pins any
    /// mapped roots. File entries, upstreams and secrets are accessed during calls.
    pub fn new(
        project_id: Uuid,
        bindings: BTreeMap<String, StarterBinding>,
        local: LocalBindings,
    ) -> Result<Self, SetupError> {
        Self::new_with_roots(project_id, bindings, local, None)
    }

    /// Reopen the reviewed mappings and check the identity of each pinned root before use.
    pub fn new_reviewed(
        project_id: Uuid,
        bindings: BTreeMap<String, StarterBinding>,
        local: LocalBindings,
        file_root_identities: &BTreeMap<String, [u8; 24]>,
    ) -> Result<Self, SetupError> {
        Self::new_with_roots(project_id, bindings, local, Some(file_root_identities))
    }

    fn new_with_roots(
        project_id: Uuid,
        bindings: BTreeMap<String, StarterBinding>,
        local: LocalBindings,
        file_root_identities: Option<&BTreeMap<String, [u8; 24]>>,
    ) -> Result<Self, SetupError> {
        let bindings = bindings
            .into_iter()
            .map(|(id, binding)| {
                if !mcp_runtime::syntax::is_key(&id) {
                    return Err(SetupError::InvalidBinding);
                }
                let binding = match binding {
                    StarterBinding::Rest(binding) => {
                        LiveBinding::Rest(Box::new(rest::Binding::new(binding)?))
                    }
                    StarterBinding::FileRead(binding) => {
                        LiveBinding::File(filesystem::Binding::new(binding)?)
                    }
                };
                Ok((id, binding))
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            project_id,
            bindings,
            rest: rest::Client::new(local.private_cas, local.secrets)?,
            files: filesystem::Files::new(local.file_roots, file_root_identities)?,
        })
    }
}

/// Resolve a directory's identity without reading entries or file contents.
pub fn review_file_root(path: &std::path::Path) -> Result<[u8; 24], SetupError> {
    filesystem::review_root(path)
}

/// Known encodings of a credential for sanitizing operator-facing evidence.
pub fn credential_redaction_variants(value: &str) -> Vec<Zeroizing<String>> {
    rest::secret_variants(value)
}

impl Adapter for StarterAdapter {
    fn prepare<'a>(
        &'a self,
        invocation: Invocation<'a>,
        _context: &'a PreparationContext,
    ) -> BoxFuture<'a, Result<Box<dyn PreparedCall + 'a>, AdapterError>> {
        Box::pin(async move {
            let binding = self
                .bindings
                .get(invocation.binding_id)
                .ok_or_else(|| failed(Effect::None))?;
            match (binding, invocation.input) {
                (LiveBinding::Rest(binding), InvocationInput::Tool(arguments)) => {
                    self.rest.prepare(
                        self.project_id,
                        invocation.capability_id,
                        invocation.binding_id,
                        binding,
                        arguments,
                    )
                }
                (LiveBinding::File(binding), InvocationInput::Resource { variables, .. }) => {
                    self.files.prepare(binding, variables)
                }
                _ => Err(failed(Effect::None)),
            }
        })
    }
}

fn failed(effect: Effect) -> AdapterError {
    AdapterError {
        kind: AdapterErrorKind::Failed,
        effect,
    }
}

fn invalid_arguments() -> AdapterError {
    AdapterError {
        kind: AdapterErrorKind::InvalidArguments,
        effect: Effect::None,
    }
}

fn output_limit(effect: Effect) -> AdapterError {
    AdapterError {
        kind: AdapterErrorKind::OutputLimit,
        effect,
    }
}

fn denied(reason: DenyReason, effect: Effect) -> AdapterError {
    AdapterError {
        kind: AdapterErrorKind::Denied(reason),
        effect,
    }
}

// Abort a blocking job if it is still queued when its caller is cancelled.
// Jobs already running retain their worker slot and check cancellation at use.
struct CancelPending(tokio::task::AbortHandle);

impl CancelPending {
    fn new<T>(task: &tokio::task::JoinHandle<T>) -> Self {
        Self(task.abort_handle())
    }
}

impl Drop for CancelPending {
    fn drop(&mut self) {
        self.0.abort();
    }
}

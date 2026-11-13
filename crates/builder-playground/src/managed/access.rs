use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

use builder_project::{Capability, ValidatedManifest};
use mcp_policy::{
    ApprovedGrant, CallerGrant, CallerIdentity, GrantTarget, HttpOrigin, LimitOverrides,
    OperatorPolicy, Redactor,
};
use mcp_runtime::{BoxFuture, Principal, bindings::StarterBinding};
use mcp_starter_adapters::{
    LocalBindings, SecretError, SecretRequest, SecretResolver, credential_redaction_variants,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zeroize::Zeroizing;

use super::{PreviewError, PreviewMode};

const MAX_CREDENTIAL_BYTES: usize = 128 * 1024;
const MAX_VALUE_BYTES: usize = 16 * 1024;

/// One source chosen by the local operator. Session values are held only in memory.
pub enum CredentialSource {
    Environment(String),
    Session(Zeroizing<String>),
}

/// Effective access selected by the operator. Empty access denies all capabilities.
#[derive(Default)]
pub struct PreviewAccess {
    pub approved_grant_ids: BTreeSet<String>,
    pub approved_prompt_ids: BTreeSet<String>,
    pub file_roots: BTreeMap<String, PathBuf>,
    pub credentials: BTreeMap<String, CredentialSource>,
    pub limits: LimitOverrides,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub(super) enum SourceReference {
    Environment { name: String },
    Session,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct FileRoot {
    pub path: PathBuf,
    pub identity: [u8; 24],
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Access {
    pub grant_ids: BTreeSet<String>,
    pub prompt_ids: BTreeSet<String>,
    pub file_roots: BTreeMap<String, FileRoot>,
    pub credential_sources: BTreeMap<String, SourceReference>,
    pub limits: LimitOverrides,
}

pub(super) type CredentialValues = BTreeMap<String, Zeroizing<String>>;
pub(super) type CredentialSources = BTreeMap<String, CredentialSource>;

impl Access {
    pub(super) fn review(
        input: PreviewAccess,
        mode: &PreviewMode,
        manifest: &ValidatedManifest,
    ) -> Result<(Self, CredentialSources), PreviewError> {
        if mode.is_fixture() && !(input.file_roots.is_empty() && input.credentials.is_empty()) {
            return Err(PreviewError::InvalidAccess);
        }
        let credential_sources = input
            .credentials
            .iter()
            .map(|(reference, source)| {
                let source = match source {
                    CredentialSource::Environment(name) => {
                        SourceReference::Environment { name: name.clone() }
                    }
                    CredentialSource::Session(_) => SourceReference::Session,
                };
                (reference.clone(), source)
            })
            .collect();
        let mut file_roots = BTreeMap::new();
        for (reference, path) in input.file_roots {
            if !path.is_absolute() || path.to_str().is_none() {
                return Err(PreviewError::InvalidAccess);
            }
            let identity = mcp_starter_adapters::review_file_root(&path)
                .map_err(|_| PreviewError::FileRootUnavailable)?;
            file_roots.insert(reference, FileRoot { path, identity });
        }
        let access = Self {
            grant_ids: input.approved_grant_ids,
            prompt_ids: input.approved_prompt_ids,
            file_roots,
            credential_sources,
            limits: input.limits,
        };
        let bindings = builder_project::runtime_bindings(manifest);
        access.project(manifest, &bindings, mode, super::OPERATOR)?;
        Ok((access, input.credentials))
    }

    pub(super) fn project(
        &self,
        manifest: &ValidatedManifest,
        bindings: &BTreeMap<String, StarterBinding>,
        mode: &PreviewMode,
        operator: &str,
    ) -> Result<ProjectedAccess, PreviewError> {
        let document = manifest.document();
        let definition = builder_project::runtime_definition(manifest);
        let mut capability_ids = self.prompt_ids.clone();
        let mut approved_grants = Vec::new();
        let mut roots = BTreeSet::new();
        let mut scopes = BTreeMap::new();
        let grants: BTreeMap<_, _> = document.policy.grants.iter().map(|g| (&g.id, g)).collect();
        let tools: BTreeMap<_, _> = definition.tools.iter().map(|t| (&t.id, t)).collect();
        let prompts: BTreeSet<_> = document
            .capabilities
            .iter()
            .filter_map(|item| match item {
                Capability::Prompt(prompt) => Some(&prompt.id),
                _ => None,
            })
            .collect();
        if !self.prompt_ids.iter().all(|id| prompts.contains(id)) {
            return Err(PreviewError::InvalidAccess);
        }
        for id in &self.grant_ids {
            let grant = grants.get(id).copied().ok_or(PreviewError::InvalidAccess)?;
            capability_ids.insert(grant.capability_id.clone());
            approved_grants.push(ApprovedGrant::new(grant.clone()));
            match &grant.target {
                GrantTarget::FileRead { root_ref, .. } => {
                    roots.insert(root_ref);
                }
                GrantTarget::Rest { .. } => {
                    let tool = tools
                        .get(&grant.capability_id)
                        .ok_or(PreviewError::InvalidAccess)?;
                    let Some(StarterBinding::Rest(binding)) = bindings.get(&tool.binding.id) else {
                        return Err(PreviewError::InvalidAccess);
                    };
                    scopes
                        .entry(grant.capability_id.clone())
                        .or_insert_with(|| SecretScope {
                            binding_id: tool.binding.id.clone(),
                            origin: binding.origin.clone(),
                            references: binding
                                .headers_from_secrets
                                .iter()
                                .map(|(_, id)| id.clone())
                                .collect(),
                        });
                }
                _ => return Err(PreviewError::InvalidAccess),
            }
        }

        if mode.is_fixture() {
            if !(self.file_roots.is_empty() && self.credential_sources.is_empty()) {
                return Err(PreviewError::InvalidAccess);
            }
        } else if self.file_roots.keys().collect::<BTreeSet<_>>() != roots {
            return Err(PreviewError::InvalidAccess);
        }
        let allowed_references: BTreeSet<_> = scopes
            .values()
            .flat_map(|scope| scope.references.iter())
            .collect();
        if self.credential_sources.iter().any(|(id, source)| {
            !allowed_references.contains(id)
                || matches!(source, SourceReference::Environment { name } if !environment_name(name))
        }) {
            return Err(PreviewError::InvalidAccess);
        }
        Ok(ProjectedAccess {
            policy: OperatorPolicy {
                approved_grants,
                approved_prompt_ids: self.prompt_ids.clone(),
                caller_grants: vec![CallerGrant {
                    identity: CallerIdentity::Local {
                        operator: operator.into(),
                    },
                    capability_ids,
                    grant_ids: self.grant_ids.clone(),
                    required_scopes: BTreeSet::new(),
                    limits: LimitOverrides::default(),
                }],
                limits: self.limits,
            },
            scopes,
        })
    }

    pub(super) fn local_bindings(
        self,
        credentials: CredentialValues,
        scopes: BTreeMap<String, SecretScope>,
        project_id: Uuid,
    ) -> Result<(LocalBindings, BTreeMap<String, [u8; 24]>), PreviewError> {
        if !credentials.keys().eq(self.credential_sources.keys()) {
            return Err(PreviewError::InvalidAccess);
        }
        check_value_sizes(&credentials)?;
        let (file_roots, identities) = self
            .file_roots
            .into_iter()
            .map(|(id, root)| ((id.clone(), root.path), (id, root.identity)))
            .unzip();
        let secrets = (!credentials.is_empty()).then(|| {
            Arc::new(ScopedSecrets {
                project_id,
                scopes,
                credentials,
            }) as Arc<dyn SecretResolver>
        });
        Ok((
            LocalBindings {
                file_roots,
                secrets,
                ..LocalBindings::default()
            },
            identities,
        ))
    }
}

fn environment_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    name.len() <= 256
        && matches!(bytes.next(), Some(b'A'..=b'Z' | b'a'..=b'z' | b'_'))
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

pub(super) fn resolve_values(sources: CredentialSources) -> Result<CredentialValues, PreviewError> {
    let credentials = sources
        .into_iter()
        .map(|(reference, source)| {
            let value = match source {
                CredentialSource::Environment(name) => Zeroizing::new(
                    std::env::var(name).map_err(|_| PreviewError::CredentialUnavailable)?,
                ),
                CredentialSource::Session(value) => value,
            };
            Ok((reference, value))
        })
        .collect::<Result<_, _>>()?;
    check_value_sizes(&credentials)?;
    Ok(credentials)
}

fn check_value_sizes(credentials: &CredentialValues) -> Result<(), PreviewError> {
    let mut total = 0;
    for value in credentials.values() {
        if value.is_empty() || value.len() > MAX_VALUE_BYTES {
            return Err(PreviewError::CredentialUnavailable);
        }
        total += value.len();
        if total > MAX_CREDENTIAL_BYTES {
            return Err(PreviewError::CredentialUnavailable);
        }
    }
    Ok(())
}

pub(super) fn redactor(
    credentials: &CredentialValues,
    sensitive_paths: &[String],
) -> Result<Arc<Redactor>, PreviewError> {
    let values = credentials
        .values()
        .flat_map(|value| credential_redaction_variants(value));
    Redactor::new(values, sensitive_paths)
        .map(Arc::new)
        .map_err(|_| PreviewError::Redaction)
}

pub(super) struct ProjectedAccess {
    pub policy: OperatorPolicy,
    pub scopes: BTreeMap<String, SecretScope>,
}

pub(super) struct SecretScope {
    binding_id: String,
    origin: HttpOrigin,
    references: BTreeSet<String>,
}

struct ScopedSecrets {
    project_id: Uuid,
    scopes: BTreeMap<String, SecretScope>,
    credentials: CredentialValues,
}

impl SecretResolver for ScopedSecrets {
    fn resolve<'a>(
        &'a self,
        request: SecretRequest<'a>,
    ) -> BoxFuture<'a, Result<Zeroizing<String>, SecretError>> {
        Box::pin(async move {
            if request.project_id != self.project_id
                || !matches!(request.principal, Principal::Local { operator } if *operator == super::OPERATOR)
                || !self.scopes.get(request.capability_id).is_some_and(|scope| {
                    scope.binding_id == request.binding_id
                        && &scope.origin == request.origin
                        && scope.references.contains(request.reference)
                })
            {
                return Err(SecretError::Unavailable);
            }
            self.credentials
                .get(request.reference)
                .cloned()
                .ok_or(SecretError::Missing)
        })
    }
}

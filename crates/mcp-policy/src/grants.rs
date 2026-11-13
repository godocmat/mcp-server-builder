use std::collections::BTreeSet;
use std::fmt;

use ipnet::IpNet;
use serde::{Deserialize, Serialize};

use crate::limits::LimitOverrides;
use crate::paths::{PathKind, PathTemplate, concrete_path, is_entry, is_key};
use crate::targets::{
    DatabaseAccess, GitOperation, HttpMethod, HttpOrigin, TargetFacts, VerifiedTarget, allows_peer,
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Operation {
    #[serde(rename = "http.read")]
    HttpRead,
    #[serde(rename = "http.write")]
    HttpWrite,
    #[serde(rename = "file.read")]
    FileRead,
    #[serde(rename = "db.read")]
    DbRead,
    #[serde(rename = "db.write")]
    DbWrite,
    #[serde(rename = "git.read")]
    GitRead,
    #[serde(rename = "exec.run")]
    ExecRun,
    #[serde(rename = "custom.invoke")]
    CustomInvoke,
}

impl From<&VerifiedTarget<'_>> for Operation {
    fn from(target: &VerifiedTarget<'_>) -> Self {
        match target {
            VerifiedTarget::Rest { method, .. } if method.is_read() => Self::HttpRead,
            VerifiedTarget::Rest { .. } => Self::HttpWrite,
            VerifiedTarget::FileRead { .. } => Self::FileRead,
            VerifiedTarget::Sqlite { access, .. } | VerifiedTarget::Postgres { access, .. } => {
                match access {
                    DatabaseAccess::Read => Self::DbRead,
                    DatabaseAccess::Write => Self::DbWrite,
                }
            }
            VerifiedTarget::Git { .. } => Self::GitRead,
            VerifiedTarget::Executable { .. } => Self::ExecRun,
            VerifiedTarget::CustomRust { .. } => Self::CustomInvoke,
        }
    }
}

/// The manifest's inert access requests, with the same field names as the portable contract.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RequestedPolicy {
    pub grants: Vec<Grant>,
}

impl<'de> Deserialize<'de> for RequestedPolicy {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fields {
            grants: Vec<Grant>,
        }
        let Fields { grants } = crate::deserialize_policy(deserializer)?;
        Ok(Self { grants })
    }
}

impl RequestedPolicy {
    /// Validate policy fields and unique grant IDs. Project reference checks belong to M1.
    pub fn validate(&self) -> Result<(), PolicyInputError> {
        let mut ids = BTreeSet::new();
        for (index, grant) in self.grants.iter().enumerate() {
            grant
                .validate()
                .map_err(|error| error.at_grant("/grants", index))?;
            if !ids.insert(&grant.id) {
                return Err(PolicyInputError::new("/id", "policy.duplicate_grant")
                    .at_grant("/grants", index));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Grant {
    pub id: String,
    pub capability_id: String,
    pub operation: Operation,
    pub target: GrantTarget,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limits: Option<LimitOverrides>,
}

impl<'de> Deserialize<'de> for Grant {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Fields {
            id: String,
            capability_id: String,
            operation: Operation,
            target: GrantTarget,
            #[serde(default, deserialize_with = "crate::deserialize_present")]
            limits: Option<LimitOverrides>,
        }
        let Fields {
            id,
            capability_id,
            operation,
            target,
            limits,
        } = crate::deserialize_policy(deserializer)?;
        Ok(Self {
            id,
            capability_id,
            operation,
            target,
            limits,
        })
    }
}

impl Grant {
    pub fn validate(&self) -> Result<(), PolicyInputError> {
        validate_key(&self.id, "/id")?;
        validate_key(&self.capability_id, "/capabilityId")?;
        if !self.target.accepts_operation(self.operation) {
            return Err(PolicyInputError::new(
                "/operation",
                "policy.operation_target_mismatch",
            ));
        }
        self.target.validate()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum GrantTarget {
    Rest {
        origin: HttpOrigin,
        method: HttpMethod,
        path_template: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        allowed_peer_ranges: Option<Vec<IpNet>>,
    },
    FileRead {
        root_ref: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        relative_path_template: Option<String>,
    },
    Sqlite {
        target_ref: String,
        binding_id: String,
    },
    Postgres {
        target_ref: String,
        binding_id: String,
    },
    Git {
        repository_ref: String,
        operation: GitOperation,
    },
    Executable {
        executable_ref: String,
        cwd_ref: String,
        binding_id: String,
    },
    CustomRust {
        source: String,
        entry: String,
    },
}

impl<'de> Deserialize<'de> for GrantTarget {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(
            tag = "kind",
            rename_all = "camelCase",
            rename_all_fields = "camelCase",
            deny_unknown_fields
        )]
        enum Fields {
            Rest {
                origin: HttpOrigin,
                method: HttpMethod,
                path_template: String,
                #[serde(default, deserialize_with = "crate::deserialize_present")]
                allowed_peer_ranges: Option<Vec<IpNet>>,
            },
            FileRead {
                root_ref: String,
                #[serde(default, deserialize_with = "crate::deserialize_present")]
                relative_path_template: Option<String>,
            },
            Sqlite {
                target_ref: String,
                binding_id: String,
            },
            Postgres {
                target_ref: String,
                binding_id: String,
            },
            Git {
                repository_ref: String,
                operation: GitOperation,
            },
            Executable {
                executable_ref: String,
                cwd_ref: String,
                binding_id: String,
            },
            CustomRust {
                source: String,
                entry: String,
            },
        }

        Ok(match crate::deserialize_policy(deserializer)? {
            Fields::Rest {
                origin,
                method,
                path_template,
                allowed_peer_ranges,
            } => Self::Rest {
                origin,
                method,
                path_template,
                allowed_peer_ranges,
            },
            Fields::FileRead {
                root_ref,
                relative_path_template,
            } => Self::FileRead {
                root_ref,
                relative_path_template,
            },
            Fields::Sqlite {
                target_ref,
                binding_id,
            } => Self::Sqlite {
                target_ref,
                binding_id,
            },
            Fields::Postgres {
                target_ref,
                binding_id,
            } => Self::Postgres {
                target_ref,
                binding_id,
            },
            Fields::Git {
                repository_ref,
                operation,
            } => Self::Git {
                repository_ref,
                operation,
            },
            Fields::Executable {
                executable_ref,
                cwd_ref,
                binding_id,
            } => Self::Executable {
                executable_ref,
                cwd_ref,
                binding_id,
            },
            Fields::CustomRust { source, entry } => Self::CustomRust { source, entry },
        })
    }
}

impl GrantTarget {
    fn accepts_operation(&self, operation: Operation) -> bool {
        match self {
            Self::Rest { method, .. } if method.is_read() => operation == Operation::HttpRead,
            Self::Rest { .. } => operation == Operation::HttpWrite,
            Self::FileRead { .. } => operation == Operation::FileRead,
            Self::Sqlite { .. } | Self::Postgres { .. } => {
                matches!(operation, Operation::DbRead | Operation::DbWrite)
            }
            Self::Git { .. } => operation == Operation::GitRead,
            Self::Executable { .. } => operation == Operation::ExecRun,
            Self::CustomRust { .. } => operation == Operation::CustomInvoke,
        }
    }

    pub(crate) fn validate(&self) -> Result<(), PolicyInputError> {
        match self {
            Self::Rest {
                path_template,
                allowed_peer_ranges,
                ..
            } => {
                validate_template(path_template, PathKind::Http, "/target/pathTemplate")?;
                if allowed_peer_ranges.as_ref().is_some_and(|ranges| {
                    ranges.iter().any(|range| {
                        *range != range.trunc() || range.addr().to_canonical() != range.addr()
                    })
                }) {
                    return Err(PolicyInputError::new(
                        "/target/allowedPeerRanges",
                        "policy.invalid_peer_range",
                    ));
                }
                Ok(())
            }
            Self::FileRead {
                root_ref,
                relative_path_template,
            } => {
                validate_key(root_ref, "/target/rootRef")?;
                if let Some(template) = relative_path_template {
                    validate_template(template, PathKind::File, "/target/relativePathTemplate")?;
                }
                Ok(())
            }
            Self::Sqlite {
                target_ref,
                binding_id,
            }
            | Self::Postgres {
                target_ref,
                binding_id,
            } => {
                validate_key(target_ref, "/target/targetRef")?;
                validate_key(binding_id, "/target/bindingId")
            }
            Self::Git { repository_ref, .. } => {
                validate_key(repository_ref, "/target/repositoryRef")
            }
            Self::Executable {
                executable_ref,
                cwd_ref,
                binding_id,
            } => {
                validate_key(executable_ref, "/target/executableRef")?;
                validate_key(cwd_ref, "/target/cwdRef")?;
                validate_key(binding_id, "/target/bindingId")
            }
            Self::CustomRust { source, entry } => {
                concrete_path(source, PathKind::File)
                    .map_err(|_| PolicyInputError::new("/target/source", "policy.invalid_path"))?;
                if !is_entry(entry) {
                    return Err(PolicyInputError::new(
                        "/target/entry",
                        "policy.invalid_entry",
                    ));
                }
                Ok(())
            }
        }
    }

    pub(crate) fn matches(&self, facts: &TargetFacts<'_, '_>) -> bool {
        match (self, facts.target) {
            (
                Self::Rest {
                    origin,
                    method,
                    path_template,
                    allowed_peer_ranges,
                },
                VerifiedTarget::Rest {
                    origin: actual_origin,
                    method: actual_method,
                    peer,
                    ..
                },
            ) => {
                origin == *actual_origin
                    && method == actual_method
                    && matches_path(path_template, &facts.path, PathKind::Http)
                    && allows_peer(allowed_peer_ranges.as_deref(), *peer)
            }
            (
                Self::FileRead {
                    root_ref,
                    relative_path_template,
                },
                VerifiedTarget::FileRead {
                    root_ref: actual_root,
                    ..
                },
            ) => {
                root_ref == actual_root
                    && relative_path_template
                        .as_deref()
                        .is_none_or(|template| matches_path(template, &facts.path, PathKind::File))
            }
            (
                Self::Sqlite {
                    target_ref,
                    binding_id,
                },
                VerifiedTarget::Sqlite {
                    target_ref: actual_ref,
                    binding_id: actual_binding,
                    ..
                },
            )
            | (
                Self::Postgres {
                    target_ref,
                    binding_id,
                },
                VerifiedTarget::Postgres {
                    target_ref: actual_ref,
                    binding_id: actual_binding,
                    ..
                },
            ) => target_ref == actual_ref && binding_id == actual_binding,
            (
                Self::Git {
                    repository_ref,
                    operation,
                },
                VerifiedTarget::Git {
                    repository_ref: actual_repo,
                    operation: actual_operation,
                },
            ) => repository_ref == actual_repo && operation == actual_operation,
            (
                Self::Executable {
                    executable_ref,
                    cwd_ref,
                    binding_id,
                },
                VerifiedTarget::Executable {
                    executable_ref: actual_executable,
                    cwd_ref: actual_cwd,
                    binding_id: actual_binding,
                },
            ) => {
                executable_ref == actual_executable
                    && cwd_ref == actual_cwd
                    && binding_id == actual_binding
            }
            (
                Self::CustomRust { source, entry },
                VerifiedTarget::CustomRust {
                    source: actual_source,
                    entry: actual_entry,
                },
            ) => source == actual_source && entry == actual_entry,
            _ => false,
        }
    }
}

fn matches_path(template: &str, path: &[String], kind: PathKind) -> bool {
    PathTemplate::parse(template, kind).is_ok_and(|template| template.matches(path))
}

fn validate_key(key: &str, field: &str) -> Result<(), PolicyInputError> {
    if is_key(key) {
        Ok(())
    } else {
        Err(PolicyInputError::new(field, "policy.invalid_key"))
    }
}

fn validate_template(value: &str, kind: PathKind, field: &str) -> Result<(), PolicyInputError> {
    PathTemplate::parse(value, kind)
        .map(|_| ())
        .map_err(|_| PolicyInputError::new(field, "policy.invalid_path_template"))
}

/// A semantic error with a fixed code and a JSON Pointer. Input values stay out of diagnostics.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolicyInputError {
    field: String,
    code: &'static str,
}

impl PolicyInputError {
    pub(crate) fn new(field: &str, code: &'static str) -> Self {
        Self {
            field: field.into(),
            code,
        }
    }

    pub(crate) fn at_grant(self, collection: &str, index: usize) -> Self {
        Self {
            field: format!("{collection}/{index}{}", self.field),
            ..self
        }
    }

    pub fn field(&self) -> &str {
        &self.field
    }

    pub fn code(&self) -> &'static str {
        self.code
    }
}

impl fmt::Display for PolicyInputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} at {}", self.code, self.field)
    }
}

impl std::error::Error for PolicyInputError {}

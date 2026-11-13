//! An operator-owned launch profile. The portable project supplies requests.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, Metadata, OpenOptions};
use std::io::Read;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use builder_project::{Binding, Capability, ManifestRevision, Transport, ValidatedManifest};
use mcp_policy::{
    ApprovedGrant, CallerGrant, CallerIdentity, GrantTarget, HttpOrigin, LimitOverrides,
    OperatorPolicy,
};
use mcp_runtime::{BoxFuture, Principal};
use mcp_starter_adapters::{LocalBindings, SecretError, SecretRequest, SecretResolver};
use serde::de::{self, MapAccess, Visitor, value::MapAccessDeserializer};
use serde::{Deserialize, Deserializer};
use uuid::Uuid;
use zeroize::Zeroizing;

pub(crate) const STDIO_OPERATOR: &str = "standalone-stdio";
const CONFIG_VERSION: u32 = 1;
const MAX_CONFIG_BYTES: usize = 4 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RuntimeConfig {
    config_version: u32,
    kind: Transport,
    #[serde(default, deserialize_with = "present")]
    manifest_revision: Option<ManifestRevision>,
    #[serde(default)]
    approved_grant_ids: Vec<String>,
    #[serde(default)]
    approved_prompt_ids: Vec<String>,
    #[serde(default, deserialize_with = "unique_map")]
    target_bindings: BTreeMap<String, TargetBinding>,
    #[serde(default, deserialize_with = "unique_map")]
    secret_sources: BTreeMap<String, SecretSource>,
    #[serde(default)]
    limits: LimitOverrides,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
enum TargetBinding {
    FileRead { path: PathBuf },
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
enum SecretSource {
    Env { name: String },
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            config_version: CONFIG_VERSION,
            kind: Transport::Stdio,
            manifest_revision: None,
            approved_grant_ids: Vec::new(),
            approved_prompt_ids: Vec::new(),
            target_bindings: BTreeMap::new(),
            secret_sources: BTreeMap::new(),
            limits: LimitOverrides::default(),
        }
    }
}

pub(crate) struct RuntimeInputs {
    pub(crate) policy: OperatorPolicy,
    pub(crate) local_bindings: LocalBindings,
}

impl RuntimeConfig {
    pub(crate) fn read(path: &Path) -> Result<Self, &'static str> {
        let bytes = read_file(
            path,
            MAX_CONFIG_BYTES,
            "runtime_config.unavailable",
            "runtime_config.too_large",
        )?;
        Self::parse(&bytes)
    }

    fn parse(bytes: &[u8]) -> Result<Self, &'static str> {
        let NamedObject(config) = serde_json::from_slice::<NamedObject<Self>>(bytes)
            .map_err(|_| "runtime_config.invalid")?;
        if config.config_version != CONFIG_VERSION {
            return Err("runtime_config.unsupported_version");
        }
        Ok(config)
    }

    pub(crate) fn into_inputs(
        self,
        manifest: &ValidatedManifest,
    ) -> Result<RuntimeInputs, &'static str> {
        let document = manifest.document();
        if self.kind != document.transport {
            return Err("runtime_config.transport_mismatch");
        }
        match self.manifest_revision {
            Some(revision) if revision != manifest.revision() => {
                return Err("runtime_config.manifest_revision_mismatch");
            }
            None if !(self.approved_grant_ids.is_empty()
                && self.approved_prompt_ids.is_empty()) =>
            {
                return Err("runtime_config.manifest_revision_required");
            }
            _ => {}
        }

        let capabilities: BTreeMap<_, _> = document
            .capabilities
            .iter()
            .map(|item| (item.id(), item))
            .collect();
        let bindings: BTreeMap<_, _> = document
            .bindings
            .iter()
            .map(|item| (item.id(), item))
            .collect();
        let requested: BTreeMap<_, _> = document
            .policy
            .grants
            .iter()
            .map(|item| (item.id.as_str(), item))
            .collect();
        let mut approved_prompt_ids = BTreeSet::new();
        for id in self.approved_prompt_ids {
            if !matches!(capabilities.get(id.as_str()), Some(Capability::Prompt(_))) {
                return Err("runtime_config.unknown_prompt");
            }
            if !approved_prompt_ids.insert(id) {
                return Err("runtime_config.duplicate_prompt");
            }
        }
        let mut capability_ids = approved_prompt_ids.clone();
        let mut grant_ids = BTreeSet::new();
        let mut approved_grants = Vec::new();
        let mut required_roots = BTreeSet::new();
        let mut secret_scopes = BTreeMap::new();
        for id in self.approved_grant_ids {
            let grant = requested
                .get(id.as_str())
                .ok_or("runtime_config.unknown_grant")?;
            if !grant_ids.insert(id) {
                return Err("runtime_config.duplicate_grant");
            }
            capability_ids.insert(grant.capability_id.clone());
            approved_grants.push(ApprovedGrant::new((*grant).clone()));
            match &grant.target {
                GrantTarget::FileRead { root_ref, .. } => {
                    required_roots.insert(root_ref.as_str());
                }
                GrantTarget::Rest { origin, .. } => {
                    let Some(Capability::Tool(tool)) =
                        capabilities.get(grant.capability_id.as_str())
                    else {
                        return Err("runtime_config.unsupported_grant");
                    };
                    let Some(Binding::Rest(binding)) = bindings.get(tool.binding_id.as_str())
                    else {
                        return Err("runtime_config.unsupported_grant");
                    };
                    secret_scopes
                        .entry(grant.capability_id.clone())
                        .or_insert_with(|| SecretScope {
                            binding_id: binding.id.clone(),
                            origin: origin.clone(),
                            references: binding
                                .headers_from_secrets
                                .iter()
                                .flatten()
                                .map(|(_, reference)| reference.clone())
                                .collect(),
                        });
                }
                _ => return Err("runtime_config.unsupported_grant"),
            }
        }

        let file_references: BTreeSet<_> = document
            .bindings
            .iter()
            .filter_map(|binding| match binding {
                Binding::FileRead(file) => Some(file.root_ref.as_str()),
                Binding::Rest(_) => None,
            })
            .collect();
        let mut file_roots = BTreeMap::new();
        for (reference, TargetBinding::FileRead { path }) in self.target_bindings {
            if !file_references.contains(reference.as_str()) {
                return Err("runtime_config.unknown_target");
            }
            if !path.is_absolute() {
                return Err("runtime_config.invalid_target_path");
            }
            // Pin only roots used by an approved grant.
            if required_roots.contains(reference.as_str()) {
                file_roots.insert(reference, path);
            }
        }
        if file_roots.len() != required_roots.len() {
            return Err("runtime_config.target_binding_required");
        }

        let declared_secrets: BTreeSet<_> = document
            .secret_refs
            .iter()
            .map(|secret| secret.id.as_str())
            .collect();
        for (reference, SecretSource::Env { name }) in &self.secret_sources {
            if !declared_secrets.contains(reference.as_str()) {
                return Err("runtime_config.unknown_secret");
            }
            if !is_environment_name(name) {
                return Err("runtime_config.invalid_environment_name");
            }
        }
        let secrets = (!self.secret_sources.is_empty()).then(|| {
            Arc::new(EnvironmentSecrets {
                project_id: document.project_id,
                scopes: secret_scopes,
                sources: self.secret_sources,
            }) as Arc<dyn SecretResolver>
        });
        Ok(RuntimeInputs {
            policy: OperatorPolicy {
                approved_grants,
                approved_prompt_ids,
                caller_grants: vec![CallerGrant {
                    identity: CallerIdentity::Local {
                        operator: STDIO_OPERATOR.into(),
                    },
                    capability_ids,
                    grant_ids,
                    required_scopes: BTreeSet::new(),
                    limits: LimitOverrides::default(),
                }],
                limits: self.limits,
            },
            local_bindings: LocalBindings {
                file_roots,
                secrets,
                ..LocalBindings::default()
            },
        })
    }
}

struct SecretScope {
    binding_id: String,
    origin: HttpOrigin,
    references: BTreeSet<String>,
}

struct EnvironmentSecrets {
    project_id: Uuid,
    scopes: BTreeMap<String, SecretScope>,
    sources: BTreeMap<String, SecretSource>,
}

impl SecretResolver for EnvironmentSecrets {
    fn resolve<'a>(
        &'a self,
        request: SecretRequest<'a>,
    ) -> BoxFuture<'a, Result<Zeroizing<String>, SecretError>> {
        Box::pin(async move {
            if request.project_id != self.project_id
                || !matches!(request.principal, Principal::Local { operator } if *operator == STDIO_OPERATOR)
                || !self.scopes.get(request.capability_id).is_some_and(|scope| {
                    scope.binding_id == request.binding_id
                        && &scope.origin == request.origin
                        && scope.references.contains(request.reference)
                })
            {
                return Err(SecretError::Unavailable);
            }
            let SecretSource::Env { name } = self
                .sources
                .get(request.reference)
                .ok_or(SecretError::Missing)?;
            std::env::var(name)
                .map(Zeroizing::new)
                .map_err(|error| match error {
                    std::env::VarError::NotPresent => SecretError::Missing,
                    std::env::VarError::NotUnicode(_) => SecretError::Unavailable,
                })
        })
    }
}

fn is_environment_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    matches!(bytes.next(), Some(b'A'..=b'Z' | b'a'..=b'z' | b'_'))
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

pub(crate) fn read_file(
    path: &Path,
    max_bytes: usize,
    unavailable: &'static str,
    too_large: &'static str,
) -> Result<Vec<u8>, &'static str> {
    if !fs::symlink_metadata(path).is_ok_and(is_regular_file) {
        return Err(unavailable);
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;

        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path).map_err(|_| unavailable)?;
    if !file.metadata().is_ok_and(is_regular_file) {
        return Err(unavailable);
    }
    let mut bytes = Vec::new();
    file.take(max_bytes as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| unavailable)?;
    if bytes.len() > max_bytes {
        return Err(too_large);
    }
    Ok(bytes)
}

fn is_regular_file(metadata: Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return false;
        }
    }
    metadata.is_file()
}

fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

// Require named records even where Serde derives accept positional sequences.
struct NamedObject<T>(T);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for NamedObject<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Object<T>(PhantomData<T>);

        impl<'de, T: Deserialize<'de>> Visitor<'de> for Object<T> {
            type Value = NamedObject<T>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a named object")
            }

            fn visit_map<M: MapAccess<'de>>(self, map: M) -> Result<Self::Value, M::Error> {
                T::deserialize(MapAccessDeserializer::new(map)).map(NamedObject)
            }
        }

        deserializer.deserialize_map(Object(PhantomData))
    }
}

fn unique_map<'de, D: Deserializer<'de>, V: Deserialize<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, V>, D::Error> {
    struct UniqueMap<V>(PhantomData<V>);

    impl<'de, V: Deserialize<'de>> Visitor<'de> for UniqueMap<V> {
        type Value = BTreeMap<String, V>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("an object with unique references")
        }

        fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
            let mut entries = BTreeMap::new();
            while let Some((key, NamedObject(value))) = map.next_entry()? {
                if entries.insert(key, value).is_some() {
                    return Err(de::Error::custom("duplicate reference"));
                }
            }
            Ok(entries)
        }
    }

    deserializer.deserialize_map(UniqueMap(PhantomData))
}

#[cfg(test)]
mod tests {
    use builder_project::{ProjectDocument, validate_document};

    use super::*;

    #[test]
    fn example_is_a_deny_by_default_operator_profile() {
        let manifest = validate_document(ProjectDocument::new("Standalone test".into())).unwrap();
        let config =
            RuntimeConfig::parse(include_bytes!("../runtime-config.example.json")).unwrap();
        let inputs = config.into_inputs(&manifest).unwrap();
        assert!(inputs.policy.approved_grants.is_empty());
        assert!(inputs.policy.approved_prompt_ids.is_empty());
        assert!(inputs.policy.caller_grants[0].capability_ids.is_empty());
        assert!(inputs.local_bindings.file_roots.is_empty());
        assert!(inputs.local_bindings.secrets.is_none());
    }

    #[test]
    fn ambiguous_or_unsupported_config_shapes_are_rejected() {
        for json in [
            r#"[1, "stdio"]"#,
            r#"{"configVersion":1,"kind":"stdio","manifestRevision":null}"#,
            r#"{"configVersion":1,"kind":"stdio","limits":{"timeoutMs":0}}"#,
            r#"{"configVersion":1,"kind":"stdio","approvedGrantIds":null}"#,
            r#"{"configVersion":1,"kind":"stdio","targetBindings":{"a":["fileRead","/tmp"]}}"#,
            r#"{"configVersion":1,"kind":"stdio","secretSources":{"a":{"kind":"env","name":"A"},"a":{"kind":"env","name":"B"}}}"#,
            r#"{"configVersion":1,"kind":"stdio","secretSources":{"a":{"kind":"env","name":"A","value":"unexpected"}}}"#,
        ] {
            assert_eq!(
                RuntimeConfig::parse(json.as_bytes()).err(),
                Some("runtime_config.invalid")
            );
        }
        assert_eq!(
            RuntimeConfig::parse(br#"{"configVersion":2,"kind":"stdio"}"#).err(),
            Some("runtime_config.unsupported_version")
        );
    }

    #[test]
    fn approvals_require_the_exact_manifest_bytes() {
        let manifest = validate_document(ProjectDocument::new("Standalone test".into())).unwrap();
        let mut config = RuntimeConfig::parse(
            br#"{"configVersion":1,"kind":"stdio","approvedPromptIds":["prompt"]}"#,
        )
        .unwrap();
        assert_eq!(
            config.into_inputs(&manifest).err(),
            Some("runtime_config.manifest_revision_required")
        );
        config = RuntimeConfig::parse(
            br#"{
            "configVersion":1,
            "kind":"stdio",
            "approvedGrantIds":["unknown"],
            "targetBindings":{"unknown":{"kind":"fileRead","path":"relative"}},
            "secretSources":{"unknown":{"kind":"env","name":"invalid name"}}
        }"#,
        )
        .unwrap();
        config.manifest_revision = Some(ManifestRevision::of(b"different manifest"));
        assert_eq!(
            config.into_inputs(&manifest).err(),
            Some("runtime_config.manifest_revision_mismatch")
        );
    }

    #[test]
    fn reads_are_bounded_and_require_regular_files() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("profile.json");
        fs::write(&path, b"12345678").unwrap();
        assert_eq!(
            read_file(&path, 8, "unavailable", "too large").unwrap(),
            b"12345678"
        );
        assert_eq!(
            read_file(&path, 7, "unavailable", "too large"),
            Err("too large")
        );
        assert_eq!(
            read_file(temp.path(), 8, "unavailable", "too large"),
            Err("unavailable")
        );
        #[cfg(unix)]
        {
            let link = temp.path().join("link.json");
            std::os::unix::fs::symlink(&path, &link).unwrap();
            assert_eq!(
                read_file(&link, 8, "unavailable", "too large"),
                Err("unavailable")
            );
        }
    }
}

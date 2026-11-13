//! Handler assembly for the exported server.

use std::sync::Arc;

use builder_project::{ValidatedManifest, runtime_bindings, runtime_definition};
use mcp_runtime::Runtime;
use mcp_starter_adapters::{SetupError, StarterAdapter};

use crate::config::RuntimeInputs;

pub(crate) fn build(
    manifest: &ValidatedManifest,
    inputs: RuntimeInputs,
) -> Result<Arc<Runtime>, &'static str> {
    let adapter = StarterAdapter::new(
        manifest.document().project_id,
        runtime_bindings(manifest),
        inputs.local_bindings,
    )
    .map_err(|error| match error {
        SetupError::InvalidBinding => "adapter.invalid_binding",
        SetupError::InvalidTrust => "adapter.invalid_trust",
        SetupError::FileRootUnavailable => "adapter.file_root_unavailable",
        SetupError::CredentialRequiresHttps => "credential.https_required",
    })?;
    Runtime::new(
        runtime_definition(manifest),
        inputs.policy,
        Arc::new(adapter),
    )
    .map(Arc::new)
    .map_err(|_| "standalone.invalid_runtime")
}

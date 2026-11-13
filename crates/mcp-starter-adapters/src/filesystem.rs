mod native;

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use mcp_policy::{DenyReason, Effect, VerifiedTarget};
use mcp_runtime::bindings::FileReadBinding;
use mcp_runtime::syntax::{
    SegmentTemplate, expand_path, is_key, is_portable_path, is_portable_segment, render_path,
    segments,
};
use mcp_runtime::{
    AdapterContext, AdapterError, AdapterOutput, BoxFuture, PreparedCall, PreparedTarget,
};
use tokio::sync::Semaphore;

use crate::{CancelPending, SetupError, denied, failed, invalid_arguments, output_limit};

pub(super) struct Binding {
    root_ref: String,
    path: Vec<SegmentTemplate>,
}

impl Binding {
    pub(super) fn new(binding: FileReadBinding) -> Result<Self, SetupError> {
        let path = segments(&binding.relative_path_template)
            .filter(|parts| is_portable_path(&expand_path(parts)))
            .ok_or(SetupError::InvalidBinding)?;
        if !is_key(&binding.root_ref) {
            return Err(SetupError::InvalidBinding);
        }
        Ok(Self {
            root_ref: binding.root_ref,
            path,
        })
    }
}

pub(super) struct Files {
    roots: BTreeMap<String, Arc<native::Root>>,
    workers: Arc<Semaphore>,
}

impl Files {
    pub(super) fn new(
        roots: BTreeMap<String, PathBuf>,
        identities: Option<&BTreeMap<String, [u8; 24]>>,
    ) -> Result<Self, SetupError> {
        if identities.is_some_and(|ids| ids.len() != roots.len()) {
            return Err(SetupError::InvalidBinding);
        }
        let roots = roots
            .into_iter()
            .map(|(reference, path)| {
                if !is_key(&reference) {
                    return Err(SetupError::InvalidBinding);
                }
                let root =
                    native::Root::open(&path).map_err(|_| SetupError::FileRootUnavailable)?;
                if let Some(identities) = identities {
                    let current = root
                        .identity()
                        .map_err(|_| SetupError::FileRootUnavailable)?;
                    if identities.get(&reference) != Some(&current) {
                        return Err(SetupError::FileRootUnavailable);
                    }
                }
                Ok((reference, Arc::new(root)))
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            roots,
            workers: Arc::new(Semaphore::new(8)),
        })
    }

    pub(super) fn prepare<'a>(
        &'a self,
        binding: &'a Binding,
        variables: &BTreeMap<String, String>,
    ) -> Result<Box<dyn PreparedCall + 'a>, AdapterError> {
        let path = render_path(
            &binding.path,
            |name| {
                variables
                    .get(name)
                    .filter(|value| is_portable_segment(value))
                    .map(|value| Cow::Borrowed(value.as_str()))
            },
            4096,
        )
        .filter(|path| is_portable_path(path))
        .ok_or_else(invalid_arguments)?;
        Ok(Box::new(FileCall {
            files: self,
            root_ref: &binding.root_ref,
            path,
        }))
    }
}

pub(super) fn review_root(path: &Path) -> Result<[u8; 24], SetupError> {
    native::Root::open(path)
        .and_then(|root| root.identity())
        .map_err(|_| SetupError::FileRootUnavailable)
}

struct FileCall<'a> {
    files: &'a Files,
    root_ref: &'a str,
    path: String,
}

impl PreparedCall for FileCall<'_> {
    fn target(&self) -> PreparedTarget<'_> {
        PreparedTarget::Verified(VerifiedTarget::FileRead {
            root_ref: self.root_ref,
            relative_path: &self.path,
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
            let root = self
                .files
                .roots
                .get(self.root_ref)
                .cloned()
                .ok_or_else(|| failed(Effect::None))?;
            let worker = Arc::clone(&self.files.workers)
                .acquire_owned()
                .await
                .map_err(|_| failed(Effect::None))?;
            let deadline = context.deadline();
            let cancellation = context.cancellation().clone();
            let mut budget = context.limits().output_budget();
            let path = self.path;
            let read = tokio::task::spawn_blocking(move || {
                let _worker = worker;
                let check_active = || {
                    if cancellation.is_cancelled() || Instant::now() >= deadline {
                        return Err(failed(Effect::None));
                    }
                    Ok(())
                };
                check_active()?;
                let mut file = root.open_file(&path).map_err(map_file_error)?;
                let mut bytes = Vec::new();
                let mut buffer = [0; 8192];
                loop {
                    check_active()?;
                    let len = buffer.len().min(budget.remaining() as usize + 1);
                    let count = file.read(&mut buffer[..len]).map_err(map_file_error)?;
                    if count == 0 {
                        break;
                    }
                    budget
                        .consume(count)
                        .map_err(|_| output_limit(Effect::None))?;
                    bytes.extend_from_slice(&buffer[..count]);
                }
                String::from_utf8(bytes).map_err(|_| failed(Effect::None))
            });
            let _abort = CancelPending::new(&read);
            let text = read.await.map_err(|_| failed(Effect::None))??;
            Ok(AdapterOutput::Resource { text })
        })
    }
}

fn map_file_error(error: std::io::Error) -> AdapterError {
    if error.kind() == std::io::ErrorKind::PermissionDenied {
        denied(DenyReason::TargetDenied, Effect::None)
    } else {
        failed(Effect::None)
    }
}

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use builder_app::{Diagnostic, DiagnosticCode, ProjectService, Reply, Severity};
use uuid::Uuid;

use crate::{export, paths, preview};

pub(crate) const HANDLE_LIFETIME: Duration = Duration::from_secs(5 * 60);
const MAX_HANDLES: usize = 64;

pub(crate) type DesktopState<'a> = tauri::State<'a, Arc<Desktop>>;
pub(crate) type Outcome<T> = Result<T, Fault>;

pub(crate) struct Desktop {
    pub projects: ProjectService,
    pub paths: Mutex<Handles<paths::Selection>>,
    pub picking: tokio::sync::Mutex<()>,
    pub exports: Mutex<Handles<export::PendingExport>>,
    pub preview: Mutex<preview::Playground>,
}

impl Desktop {
    pub(crate) fn new(projects: ProjectService) -> Arc<Self> {
        Arc::new(Self {
            projects,
            paths: Mutex::default(),
            picking: tokio::sync::Mutex::new(()),
            exports: Mutex::default(),
            preview: Mutex::default(),
        })
    }

    pub(crate) fn expire_handles(state: &Arc<Self>) {
        let state = Arc::downgrade(state);
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(15)).await;
                let Some(state) = state.upgrade() else {
                    return;
                };
                state.paths.lock().expect("path table").prune();
                state.exports.lock().expect("export table").prune();
                state.preview.lock().expect("playground table").prune();
            }
        });
    }

    pub(crate) fn clear(&self) {
        self.paths.lock().expect("path table").clear();
        self.exports.lock().expect("export table").clear();
        self.preview.lock().expect("playground table").clear();
    }
}

struct Entry<T> {
    expires: Instant,
    value: T,
}

pub(crate) struct Handles<T>(HashMap<Uuid, Entry<T>>);

impl<T> Default for Handles<T> {
    fn default() -> Self {
        Self(HashMap::new())
    }
}

impl<T> Handles<T> {
    pub(crate) fn insert(&mut self, value: T) -> Outcome<Uuid> {
        self.insert_until(value, Instant::now() + HANDLE_LIFETIME)
    }

    pub(crate) fn insert_until(&mut self, value: T, expires: Instant) -> Outcome<Uuid> {
        self.prune();
        if self.0.len() >= MAX_HANDLES {
            return Err(Fault::invalid(
                "Too many pending selections or reviews.",
                "Wait for old selections to expire and try again.",
            ));
        }
        let id = Uuid::new_v4();
        self.0.insert(id, Entry { expires, value });
        Ok(id)
    }

    pub(crate) fn take(&mut self, id: Uuid) -> Option<T> {
        self.take_if(id, |_| true).map(|(value, _)| value)
    }

    pub(crate) fn take_if(
        &mut self,
        id: Uuid,
        matches: impl FnOnce(&T) -> bool,
    ) -> Option<(T, Instant)> {
        self.prune();
        if !matches(&self.0.get(&id)?.value) {
            return None;
        }
        self.0.remove(&id).map(|entry| (entry.value, entry.expires))
    }

    pub(crate) fn prune(&mut self) {
        let now = Instant::now();
        self.0.retain(|_, entry| entry.expires > now);
    }

    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }
}

#[derive(Debug)]
pub(crate) struct Fault {
    error: Box<Diagnostic>,
    diagnostics: Vec<Diagnostic>,
}

impl Fault {
    pub(crate) fn new(
        code: DiagnosticCode,
        message: &str,
        action: &str,
        field: Option<&str>,
    ) -> Self {
        Self {
            error: Box::new(Diagnostic {
                code,
                message: message.into(),
                severity: Severity::Error,
                correlation_id: Uuid::new_v4(),
                action: Some(action.into()),
                field: field.map(String::from),
                capability_id: None,
                details: None,
            }),
            diagnostics: Vec::new(),
        }
    }

    pub(crate) fn invalid(message: &str, action: &str) -> Self {
        Self::new(DiagnosticCode::InvalidInput, message, action, None)
    }

    pub(crate) fn internal() -> Self {
        Self::new(
            DiagnosticCode::InternalError,
            "The desktop operation could not finish.",
            "Retry the operation.",
            None,
        )
    }

    pub(crate) fn into_reply<T>(self) -> Reply<T> {
        Reply::Failure {
            error: *self.error,
            diagnostics: self.diagnostics,
        }
    }

    pub(crate) fn into_diagnostic(self) -> Diagnostic {
        *self.error
    }
}

pub(crate) fn unpack<T>(reply: Reply<T>) -> Outcome<(T, Vec<Diagnostic>)> {
    match reply {
        Reply::Success { data, diagnostics } => Ok((data, diagnostics)),
        Reply::Failure { error, diagnostics } => Err(Fault {
            error: Box::new(error),
            diagnostics,
        }),
    }
}

pub(crate) fn success<T>(data: T) -> Reply<T> {
    Reply::Success {
        data,
        diagnostics: Vec::new(),
    }
}

pub(crate) fn reply<T>(outcome: Outcome<T>) -> Reply<T> {
    match outcome {
        Ok(data) => success(data),
        Err(error) => error.into_reply(),
    }
}

pub(crate) async fn on_disk<T: Send + 'static>(
    operation: impl FnOnce() -> Outcome<T> + Send + 'static,
) -> Outcome<T> {
    tauri::async_runtime::spawn_blocking(operation)
        .await
        .map_err(|_| Fault::internal())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expired_handles_release_capacity_and_shutdown_invalidates_remaining_handles() {
        let mut handles = Handles::default();
        let past = Instant::now() - Duration::from_secs(1);
        let expired = handles.insert_until("expired review", past).unwrap();
        let active = handles.insert("active review").unwrap();
        for _ in 1..MAX_HANDLES {
            handles.insert("another pending review").unwrap();
        }
        assert!(handles.take(expired).is_none());
        assert_eq!(
            handles
                .insert("above the limit")
                .unwrap_err()
                .into_diagnostic()
                .code,
            DiagnosticCode::InvalidInput
        );
        handles.clear();
        assert!(handles.take(active).is_none());
        assert!(handles.insert("new review").is_ok());
    }
}

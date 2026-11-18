use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

use builder_app::Diagnostic;
use builder_app::playground::{
    ApprovedProjectPreview, CancellationToken, Discovery, Invocation, ManagedSession,
    PreparedProjectPreview, PreviewError, PreviewMode, PreviewOptions, PreviewRun,
};
use serde::Serialize;
use tokio::sync::RwLock;
use uuid::Uuid;

use super::{LaunchSubject, disconnected, preview_fault};
use crate::ipc::{Fault, Handles, Outcome};

const MAX_OPERATIONS: usize = 8;
type Session = Arc<RwLock<Option<ManagedSession>>>;

#[derive(Default)]
pub(crate) struct Playground {
    reviews: Handles<PendingLaunch>,
    approvals: Handles<ApprovedLaunch>,
    slot: Slot,
    operations: VecDeque<(Uuid, Operation)>,
}

#[derive(Default)]
enum Slot {
    #[default]
    Idle,
    Starting(Uuid),
    Active(Connection),
    Stopping(Uuid),
}

struct Connection {
    id: Uuid,
    settings: LaunchSettings,
    discovery: Arc<Discovery>,
    session: Session,
    run: Option<Uuid>,
}

pub(super) struct LaunchSettings {
    pub subject: LaunchSubject,
    pub mode: PreviewMode,
    pub options: PreviewOptions,
}

pub(super) struct PendingLaunch {
    pub settings: LaunchSettings,
    pub prepared: PreparedProjectPreview,
}

pub(super) struct ApprovedLaunch {
    pub settings: LaunchSettings,
    pub approved: ApprovedProjectPreview,
}

pub(super) struct StartTask {
    pub operation: OperationHandle,
    pub launch: ApprovedLaunch,
}

pub(super) enum RunAction {
    Call(Invocation),
    Replay {
        subject: LaunchSubject,
        scenario_id: String,
        allow_live: bool,
    },
}

pub(super) struct OperationHandle {
    pub id: Uuid,
    pub cancellation: CancellationToken,
}

enum Operation {
    Pending(CancellationToken),
    Terminal(Arc<Event>),
}

#[derive(Serialize)]
struct Event {
    cursor: u32,
    #[serde(flatten)]
    data: EventData,
}

#[derive(Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
enum EventData {
    Started {
        connection_id: Uuid,
        discovery: Arc<Discovery>,
    },
    Completed {
        run: PreviewRun,
    },
    Failed {
        error: Diagnostic,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PollResult {
    events: Vec<Arc<Event>>,
    next_cursor: u32,
    terminal: bool,
}

impl Playground {
    pub(super) fn pending(&mut self, pending: PendingLaunch, expires: Instant) -> Outcome<Uuid> {
        self.reviews.insert_until(pending, expires)
    }

    pub(super) fn take_review(&mut self, id: Uuid) -> Outcome<(PendingLaunch, Instant)> {
        self.reviews
            .take_if(id, |_| true)
            .ok_or_else(approval_required)
    }

    pub(super) fn approved(&mut self, approval: ApprovedLaunch, expires: Instant) -> Outcome<Uuid> {
        self.approvals.insert_until(approval, expires)
    }

    pub(super) fn start(&mut self, approval: Uuid, subject: LaunchSubject) -> Outcome<StartTask> {
        let launch = self
            .approvals
            .take(approval)
            .filter(|launch| launch.settings.subject == subject)
            .ok_or_else(approval_required)?;
        if !matches!(self.slot, Slot::Idle) {
            return Err(Fault::invalid(
                "A preview is already starting, active, or stopping.",
                "Stop the existing preview before approving a new one.",
            ));
        }
        let operation = self.new_operation();
        self.slot = Slot::Starting(operation.id);
        Ok(StartTask { operation, launch })
    }

    pub(super) fn connect(
        &mut self,
        id: Uuid,
        settings: LaunchSettings,
        session: ManagedSession,
        discovery: Discovery,
    ) -> Option<ManagedSession> {
        if !matches!(self.slot, Slot::Starting(known) if known == id)
            || !matches!(self.operation(id), Some(Operation::Pending(token)) if !token.is_cancelled())
        {
            return Some(session);
        }
        let connection_id = Uuid::new_v4();
        let discovery = Arc::new(discovery);
        self.slot = Slot::Active(Connection {
            id: connection_id,
            settings,
            discovery: Arc::clone(&discovery),
            session: Arc::new(RwLock::new(Some(session))),
            run: None,
        });
        self.finish(
            id,
            EventData::Started {
                connection_id,
                discovery,
            },
        );
        None
    }

    pub(super) fn fail_start(&mut self, id: Uuid, error: PreviewError) {
        if matches!(self.slot, Slot::Starting(known) if known == id) {
            self.slot = Slot::Idle;
            self.finish(id, failure(error));
        }
    }

    fn connection(&self, id: Uuid) -> Outcome<&Connection> {
        match &self.slot {
            Slot::Active(connection) if connection.id == id => Ok(connection),
            _ => Err(disconnected()),
        }
    }

    pub(super) fn discover(&self, id: Uuid) -> Outcome<Arc<Discovery>> {
        Ok(Arc::clone(&self.connection(id)?.discovery))
    }

    pub(super) fn session(&self, id: Uuid) -> Outcome<Session> {
        Ok(Arc::clone(&self.connection(id)?.session))
    }

    pub(super) fn begin_run(
        &mut self,
        connection_id: Uuid,
        action: &RunAction,
    ) -> Outcome<(OperationHandle, Session)> {
        let connection = self.connection(connection_id)?;
        if connection.run.is_some() {
            return Err(Fault::invalid(
                "The current playground call is still running.",
                "Wait for it to finish or cancel it.",
            ));
        }
        check_action(&connection.settings, action)?;
        let session = Arc::clone(&connection.session);
        let operation = self.new_operation();
        if let Slot::Active(connection) = &mut self.slot {
            connection.run = Some(operation.id);
        }
        Ok((operation, session))
    }

    pub(super) fn complete_run(&mut self, id: Uuid, result: Result<PreviewRun, PreviewError>) {
        if let Slot::Active(connection) = &mut self.slot
            && connection.run == Some(id)
        {
            connection.run = None;
            self.finish(
                id,
                match result {
                    Ok(run) => EventData::Completed { run },
                    Err(error) => failure(error),
                },
            );
        }
    }

    pub(super) fn begin_stop(&mut self, id: Uuid) -> Outcome<Session> {
        let session = self.session(id)?;
        self.cancel_operations();
        self.slot = Slot::Stopping(id);
        Ok(session)
    }

    pub(super) fn finish_stop(&mut self, id: Uuid) {
        if matches!(self.slot, Slot::Stopping(known) if known == id) {
            self.slot = Slot::Idle;
        }
    }

    pub(super) fn poll(&self, id: Uuid, cursor: u32) -> Outcome<PollResult> {
        match self.operation(id).ok_or_else(operation_expired)? {
            Operation::Pending(_) if cursor == 0 => Ok(PollResult {
                events: Vec::new(),
                next_cursor: 0,
                terminal: false,
            }),
            Operation::Terminal(event) if cursor <= event.cursor => Ok(PollResult {
                events: (cursor < event.cursor)
                    .then(|| Arc::clone(event))
                    .into_iter()
                    .collect(),
                next_cursor: event.cursor,
                terminal: true,
            }),
            _ => Err(Fault::invalid(
                "The playground operation cursor is invalid.",
                "Use the cursor returned by the previous poll.",
            )),
        }
    }

    pub(super) fn cancel(&self, id: Uuid) -> Outcome<bool> {
        match self.operation(id).ok_or_else(operation_expired)? {
            Operation::Pending(token) if !token.is_cancelled() => {
                token.cancel();
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    pub(crate) fn prune(&mut self) {
        self.reviews.prune();
        self.approvals.prune();
    }

    pub(crate) fn clear(&mut self) {
        self.reviews.clear();
        self.approvals.clear();
        self.cancel_operations();
        self.slot = Slot::Idle;
    }

    fn new_operation(&mut self) -> OperationHandle {
        // Only one pending operation is admitted at a time, so eviction removes terminal data.
        while self.operations.len() >= MAX_OPERATIONS {
            self.operations.pop_front();
        }
        let id = Uuid::new_v4();
        let cancellation = CancellationToken::new();
        self.operations
            .push_back((id, Operation::Pending(cancellation.clone())));
        OperationHandle { id, cancellation }
    }

    fn operation(&self, id: Uuid) -> Option<&Operation> {
        self.operations
            .iter()
            .find_map(|(known, operation)| (*known == id).then_some(operation))
    }

    fn finish(&mut self, id: Uuid, data: EventData) {
        if let Some((_, operation)) = self.operations.iter_mut().find(|(known, _)| *known == id) {
            *operation = Operation::Terminal(Arc::new(Event { cursor: 1, data }));
        }
    }

    fn cancel_operations(&mut self) {
        for (_, operation) in self.operations.drain(..) {
            if let Operation::Pending(token) = operation {
                token.cancel();
            }
        }
    }
}

fn check_action(settings: &LaunchSettings, action: &RunAction) -> Outcome<()> {
    match (&settings.mode, action) {
        (PreviewMode::Live { scenario_id: None }, RunAction::Call(_)) => Ok(()),
        (
            PreviewMode::Fixture { scenario_id },
            RunAction::Replay {
                subject,
                scenario_id: requested,
                ..
            },
        ) if subject == &settings.subject && scenario_id == requested => Ok(()),
        (
            PreviewMode::Live {
                scenario_id: Some(scenario_id),
            },
            RunAction::Replay {
                subject,
                scenario_id: requested,
                allow_live: true,
            },
        ) if subject == &settings.subject && scenario_id == requested => Ok(()),
        _ => Err(approval_required()),
    }
}

fn failure(error: PreviewError) -> EventData {
    EventData::Failed {
        error: preview_fault(error).into_diagnostic(),
    }
}

fn approval_required() -> Fault {
    preview_fault(PreviewError::ApprovalRequired)
}

fn operation_expired() -> Fault {
    Fault::invalid(
        "The playground operation is unknown or its result expired.",
        "Start a new call on the reviewed connection.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use builder_app::ManifestRevision;

    #[test]
    fn pending_cancel_and_terminal_poll_are_idempotent_and_bounded() {
        let mut playground = Playground::default();
        let pending = playground.new_operation();
        assert!(!playground.poll(pending.id, 0).unwrap().terminal);
        assert!(playground.cancel(pending.id).unwrap());
        assert!(!playground.cancel(pending.id).unwrap());
        assert!(pending.cancellation.is_cancelled());
        playground.finish(pending.id, failure(PreviewError::Cancelled));
        let first = playground.poll(pending.id, 0).unwrap();
        let frame = serde_json::to_value(first).unwrap();
        assert_eq!(frame["events"][0]["kind"], "failed");
        assert_eq!(frame["events"][0]["error"]["code"], "run.cancelled");
        assert_eq!(frame["nextCursor"], 1);
        assert_eq!(frame["terminal"], true);
        assert!(playground.poll(pending.id, 1).unwrap().events.is_empty());
        for _ in 0..MAX_OPERATIONS {
            let operation = playground.new_operation();
            playground.finish(operation.id, failure(PreviewError::Cancelled));
        }
        assert!(playground.poll(pending.id, 0).is_err());
    }

    #[test]
    fn scoped_playback_checks_revision_scenario_and_live_consent() {
        let registration_id = Uuid::new_v4();
        let subject = LaunchSubject::Launch {
            registration_id,
            expected_revision: ManifestRevision::of(b"reviewed"),
        };
        let mut settings = LaunchSettings {
            subject,
            mode: PreviewMode::Fixture {
                scenario_id: "pilot".into(),
            },
            options: PreviewOptions::default(),
        };
        let request = |subject, scenario: &str, allow_live| RunAction::Replay {
            subject,
            scenario_id: scenario.into(),
            allow_live,
        };
        assert!(check_action(&settings, &request(subject, "pilot", false)).is_ok());
        assert!(check_action(&settings, &request(subject, "another", false)).is_err());
        let stale = LaunchSubject::Launch {
            registration_id,
            expected_revision: ManifestRevision::of(b"stale"),
        };
        assert!(check_action(&settings, &request(stale, "pilot", false)).is_err());
        settings.mode = PreviewMode::Live {
            scenario_id: Some("pilot".into()),
        };
        assert!(check_action(&settings, &request(subject, "pilot", false)).is_err());
        assert!(check_action(&settings, &request(subject, "pilot", true)).is_ok());
    }
}

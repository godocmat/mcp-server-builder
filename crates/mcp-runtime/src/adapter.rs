use std::collections::BTreeMap;
use std::num::NonZeroU64;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use mcp_policy::{
    Action, DenyReason, Effect, ExecutionCeiling, ExecutionLimits, LimitOverrides, Operation,
    OperationContext, Policy, RestRoute, VerifiedTarget,
};
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::{BoxFuture, CancellationToken, ErrorKind, Principal, Progress, ProgressSink};

/// Trusted adapter code prepares and retains the same target that it will use. Preparation
/// does not resolve credentials or perform the requested read, mutation or command.
pub trait Adapter: Send + Sync {
    fn prepare<'a>(
        &'a self,
        invocation: Invocation<'a>,
        context: &'a PreparationContext,
    ) -> BoxFuture<'a, Result<Box<dyn PreparedCall + 'a>, AdapterError>>;
}

pub trait PreparedCall: Send {
    fn target(&self) -> PreparedTarget<'_>;

    /// Use the prepared target only after admission. Any changed target requires
    /// `authorize_target` before effects, with the previous deadline and limits retained.
    /// Return promptly after a denial so the runtime can report the typed decision.
    fn execute<'a>(
        self: Box<Self>,
        context: &'a AdapterContext<'_>,
    ) -> BoxFuture<'a, Result<AdapterOutput, AdapterError>>
    where
        Self: 'a;
}

/// A REST route allows DNS resolution after route admission. The adapter must
/// authorize the concrete peer before using the connection.
pub enum PreparedTarget<'a> {
    Verified(VerifiedTarget<'a>),
    RestRoute(&'a RestRoute),
}

pub struct Invocation<'a> {
    pub capability_id: &'a str,
    pub binding_id: &'a str,
    pub input: InvocationInput<'a>,
}

pub enum InvocationInput<'a> {
    Tool(&'a Map<String, Value>),
    Resource {
        uri: &'a str,
        variables: &'a BTreeMap<String, String>,
    },
}

#[derive(Debug)]
pub enum AdapterOutput {
    Tool {
        text: Option<String>,
        structured_content: Option<Map<String, Value>>,
        effect: Effect,
    },
    Resource {
        text: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdapterError {
    pub kind: AdapterErrorKind,
    pub effect: Effect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdapterErrorKind {
    Failed,
    FixtureMissing,
    InvalidArguments,
    OutputLimit,
    CredentialMissing,
    CredentialUnavailable,
    CredentialInvalid,
    Denied(DenyReason),
}

impl AdapterErrorKind {
    pub(crate) fn runtime_kind(self) -> ErrorKind {
        match self {
            Self::Failed => ErrorKind::AdapterFailed,
            Self::FixtureMissing => ErrorKind::FixtureMissing,
            Self::InvalidArguments => ErrorKind::InvalidArguments,
            Self::OutputLimit => ErrorKind::OutputLimit,
            Self::CredentialMissing => ErrorKind::CredentialMissing,
            Self::CredentialUnavailable => ErrorKind::CredentialUnavailable,
            Self::CredentialInvalid => ErrorKind::CredentialInvalid,
            Self::Denied(_) => ErrorKind::Denied,
        }
    }
}

pub struct PreparationContext {
    pub(crate) ceiling: ExecutionCeiling,
    pub(crate) cancellation: CancellationToken,
    pub(crate) correlation_id: Uuid,
}

impl PreparationContext {
    pub fn deadline(&self) -> Instant {
        self.ceiling.deadline
    }

    pub fn limits(&self) -> ExecutionLimits {
        self.ceiling.limits
    }

    pub fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }

    pub fn correlation_id(&self) -> Uuid {
        self.correlation_id
    }
}

pub struct AdapterContext<'a> {
    pub(crate) preparation: &'a PreparationContext,
    pub(crate) policy: &'a Policy,
    pub(crate) principal: &'a Principal<'a>,
    pub(crate) capability_id: &'a str,
    pub(crate) operation: Operation,
    pub(crate) timeout_ms: Option<NonZeroU64>,
    pub(crate) requested_limits: LimitOverrides,
    pub(crate) admitted_at: Instant,
    pub(crate) state: Mutex<TargetState>,
    pub(crate) progress: Option<&'a dyn ProgressSink>,
    pub(crate) progress_state: tokio::sync::Mutex<ProgressState>,
    pub(crate) deadline_changed: tokio::sync::Notify,
}

pub(crate) struct TargetState {
    pub(crate) ceiling: ExecutionCeiling,
    pub(crate) denial: Option<DenyReason>,
    pub(crate) pending_rest: Option<(RestRoute, bool)>,
}

#[derive(Default)]
pub(crate) struct ProgressState {
    sent: u8,
    last: Option<(Instant, f64)>,
}

impl AdapterContext<'_> {
    pub fn principal(&self) -> &Principal<'_> {
        self.principal
    }

    pub fn deadline(&self) -> Instant {
        self.current_ceiling().deadline
    }

    pub fn limits(&self) -> ExecutionLimits {
        self.current_ceiling().limits
    }

    pub fn cancellation(&self) -> &CancellationToken {
        self.preparation.cancellation()
    }

    pub fn correlation_id(&self) -> Uuid {
        self.preparation.correlation_id()
    }

    pub fn authorize_target(
        &self,
        target: &VerifiedTarget<'_>,
    ) -> Result<ExecutionCeiling, DenyReason> {
        self.recheck(
            Action::Adapter {
                operation: self.operation,
                target,
            },
            None,
        )
    }

    /// Check a proposed redirect before DNS. Authorize its actual peer separately.
    pub fn preflight_redirect(&self, route: &RestRoute) -> Result<(), DenyReason> {
        self.recheck(
            Action::PreflightRest {
                operation: self.operation,
                route,
                redirected: true,
            },
            Some((route.clone(), true)),
        )
        .map(|_| ())
    }

    fn recheck(
        &self,
        action: Action<'_>,
        pending_rest: Option<(RestRoute, bool)>,
    ) -> Result<ExecutionCeiling, DenyReason> {
        let mut state = self.state.lock().expect("execution target lock");
        if self.cancellation().is_cancelled() {
            return Err(DenyReason::InvalidContext);
        }
        if let Some(reason) = state.denial {
            return Err(reason);
        }
        if let (Some((route, redirected)), Action::Adapter { target, .. }) =
            (&state.pending_rest, &action)
            && !matches!(
                target,
                VerifiedTarget::Rest { origin, method, path, redirected: actual_redirect, .. }
                    if *origin == &route.origin && *method == route.method
                        && *path == route.path && actual_redirect == redirected
            )
        {
            state.denial = Some(DenyReason::InvalidTarget);
            return Err(DenyReason::InvalidTarget);
        }
        let decision = self.policy.evaluate(&OperationContext {
            principal: self.principal,
            capability_id: self.capability_id,
            action,
            capability_timeout_ms: self.timeout_ms,
            requested_limits: self.requested_limits,
            ceiling: Some(state.ceiling),
            admitted_at: self.admitted_at,
            now: Instant::now(),
        });
        match decision {
            Ok(permit) => {
                let next = permit.ceiling();
                if next.deadline < state.ceiling.deadline {
                    self.deadline_changed.notify_one();
                }
                state.ceiling = next;
                state.pending_rest = pending_rest;
            }
            Err(reason) => {
                state.denial = Some(reason);
                return Err(reason);
            }
        }
        Ok(state.ceiling)
    }

    pub async fn report_progress(&self, mut event: Progress) {
        let Some(sink) = self.progress else { return };
        if self.cancellation().is_cancelled()
            || !event.progress.is_finite()
            || event.progress < 0.0
            || event
                .total
                .is_some_and(|total| !total.is_finite() || total <= 0.0 || total < event.progress)
        {
            return;
        }
        let mut state = self.progress_state.lock().await;
        let now = Instant::now();
        if self.cancellation().is_cancelled()
            || now >= self.deadline()
            || self.recheck_denial().is_some()
            || state.sent >= 32
            || state.last.is_some_and(|(time, previous)| {
                event.progress <= previous || now.duration_since(time) < Duration::from_millis(50)
            })
        {
            return;
        }
        event.message = None;
        state.last = Some((now, event.progress));
        state.sent += 1;
        sink.send(event).await;
    }

    pub(crate) fn current_ceiling(&self) -> ExecutionCeiling {
        self.state.lock().expect("execution target lock").ceiling
    }

    pub(crate) fn recheck_denial(&self) -> Option<DenyReason> {
        self.state.lock().expect("execution target lock").denial
    }

    pub(crate) fn peer_pending(&self) -> bool {
        self.state
            .lock()
            .expect("execution target lock")
            .pending_rest
            .is_some()
    }
}

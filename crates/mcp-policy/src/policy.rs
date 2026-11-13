use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::num::NonZeroU64;
use std::time::Instant;

use serde::{Serialize, Serializer};

use crate::grants::{Grant, GrantTarget, Operation, PolicyInputError, RequestedPolicy};
use crate::limits::{ExecutionCeiling, ExecutionLimits, LimitOverrides};
use crate::paths::is_key;
use crate::targets::{RestRoute, VerifiedTarget};

/// Identity and claims established by local launch or transport authentication.
pub enum Principal<'a> {
    Anonymous,
    Local {
        operator: &'a str,
    },
    Remote {
        issuer: &'a str,
        subject: &'a str,
        client_id: Option<&'a str>,
        scopes: &'a [&'a str],
        expires_at: Instant,
    },
}

/// An operator-configured selector. A client-specific rule takes precedence over a subject rule.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum CallerIdentity {
    Local {
        operator: String,
    },
    Remote {
        issuer: String,
        subject: String,
        client_id: Option<String>,
    },
}

#[derive(Clone, Debug)]
pub struct CallerGrant {
    pub identity: CallerIdentity,
    pub capability_ids: BTreeSet<String>,
    pub grant_ids: BTreeSet<String>,
    pub required_scopes: BTreeSet<String>,
    pub limits: LimitOverrides,
}

/// A grant captured during operator review, optionally narrowed by operator policy.
/// Keep this snapshot tied to the reviewed launch, deployment and target bindings.
#[derive(Clone, Debug)]
pub struct ApprovedGrant {
    pub grant: Grant,
    pub redirects: Vec<RestRoute>,
}

impl ApprovedGrant {
    pub fn new(grant: Grant) -> Self {
        Self {
            grant,
            redirects: Vec::new(),
        }
    }

    fn permits_redirect(&self, target: &VerifiedTarget<'_>) -> bool {
        match target {
            VerifiedTarget::Rest {
                origin,
                method,
                path,
                redirected: true,
                ..
            } => self.redirects.iter().any(|route| {
                &route.origin == *origin && route.method == *method && route.path == *path
            }),
            _ => true,
        }
    }
}

/// Trusted inputs from reviewed local launch or operator-owned deployment configuration.
#[derive(Clone, Debug, Default)]
pub struct OperatorPolicy {
    pub approved_grants: Vec<ApprovedGrant>,
    pub approved_prompt_ids: BTreeSet<String>,
    pub caller_grants: Vec<CallerGrant>,
    pub limits: LimitOverrides,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityAction {
    Adapter(Operation),
    RenderPrompt,
}

pub enum Action<'a> {
    Adapter {
        operation: Operation,
        target: &'a VerifiedTarget<'a>,
    },
    RenderPrompt,
}

pub struct OperationContext<'a> {
    pub principal: &'a Principal<'a>,
    pub capability_id: &'a str,
    pub action: Action<'a>,
    pub capability_timeout_ms: Option<NonZeroU64>,
    pub requested_limits: LimitOverrides,
    /// The previous permit for this call on a recheck, or an earlier runtime deadline.
    pub ceiling: Option<ExecutionCeiling>,
    pub admitted_at: Instant,
    pub now: Instant,
}

/// A validated pair of immutable project and operator snapshots.
pub struct Policy {
    grants: BTreeMap<String, (Grant, usize)>,
    operator: OperatorPolicy,
}

impl Policy {
    pub fn new(
        requested: &RequestedPolicy,
        operator: &OperatorPolicy,
    ) -> Result<Self, PolicyInputError> {
        Self::from_snapshots(requested, operator.clone())
    }

    pub fn from_snapshots(
        requested: &RequestedPolicy,
        operator: OperatorPolicy,
    ) -> Result<Self, PolicyInputError> {
        requested.validate()?;
        let requested_by_id: BTreeMap<_, _> = requested
            .grants
            .iter()
            .map(|grant| (grant.id.as_str(), grant))
            .collect();
        let mut grants = BTreeMap::new();
        for (index, approved) in operator.approved_grants.iter().enumerate() {
            let grant = validate_approval(&requested_by_id, approved)
                .map_err(|error| error.at_grant("/approvedGrants", index))?;
            if grants
                .insert(approved.grant.id.clone(), (grant.clone(), index))
                .is_some()
            {
                return Err(PolicyInputError::new("/id", "policy.duplicate_approval")
                    .at_grant("/approvedGrants", index));
            }
        }
        validate_callers(&operator)?;
        if !operator.approved_prompt_ids.iter().all(|id| is_key(id)) {
            return Err(PolicyInputError::new(
                "/approvedPromptIds",
                "policy.invalid_key",
            ));
        }
        Ok(Self { grants, operator })
    }

    /// Filter discovery using caller identity, scopes, capability and approved operation.
    /// Invocation always needs a fresh `evaluate` with verified target facts.
    pub fn is_visible(
        &self,
        principal: &Principal<'_>,
        capability_id: &str,
        action: CapabilityAction,
        now: Instant,
    ) -> bool {
        let Ok(caller) = self.caller(principal, now) else {
            return false;
        };
        caller.capability_ids.contains(capability_id)
            && match action {
                CapabilityAction::RenderPrompt => {
                    self.operator.approved_prompt_ids.contains(capability_id)
                }
                CapabilityAction::Adapter(operation) => self
                    .grants_for(capability_id, operation)
                    .any(|(requested, _)| caller.grant_ids.contains(&requested.id)),
            }
    }

    pub fn evaluate(&self, context: &OperationContext<'_>) -> Result<Permit<'_>, DenyReason> {
        if context.admitted_at > context.now || !is_key(context.capability_id) {
            return Err(DenyReason::InvalidContext);
        }
        let caller = self.caller(context.principal, context.now)?;
        self.check_capability(caller, context.capability_id)?;
        let limits = self.caller_limits(
            caller,
            context.capability_timeout_ms,
            context.requested_limits,
        );
        let (grant_id, limits) = match context.action {
            Action::RenderPrompt
                if self
                    .operator
                    .approved_prompt_ids
                    .contains(context.capability_id) =>
            {
                (None, limits)
            }
            Action::RenderPrompt => return Err(DenyReason::GrantRequired),
            Action::Adapter { operation, target } => {
                self.evaluate_adapter(context.capability_id, operation, target, caller, limits)?
            }
        };
        let mut ceiling =
            call_ceiling(context.principal, limits, context.admitted_at, context.now)?;
        if let Some(previous) = context.ceiling {
            ceiling.limits = ceiling.limits.intersect(previous.limits);
            ceiling.deadline = ceiling.deadline.min(previous.deadline);
            if context.now >= ceiling.deadline {
                return Err(DenyReason::DeadlineExceeded);
            }
        }
        Ok(Permit { grant_id, ceiling })
    }

    /// Bound inert preparation before a concrete target can be checked.
    /// The returned ceiling conveys no permission to perform an adapter effect.
    pub fn preparation_ceiling(
        &self,
        principal: &Principal<'_>,
        capability_id: &str,
        capability_timeout_ms: Option<NonZeroU64>,
        requested_limits: LimitOverrides,
        admitted_at: Instant,
        now: Instant,
    ) -> Result<ExecutionCeiling, DenyReason> {
        let caller = self.caller(principal, now)?;
        self.check_capability(caller, capability_id)?;
        call_ceiling(
            principal,
            self.caller_limits(caller, capability_timeout_ms, requested_limits),
            admitted_at,
            now,
        )
    }

    fn check_capability(
        &self,
        caller: &CallerGrant,
        capability_id: &str,
    ) -> Result<(), DenyReason> {
        if !is_key(capability_id) {
            return Err(DenyReason::InvalidContext);
        }
        if !caller.capability_ids.contains(capability_id) {
            return Err(DenyReason::CapabilityDenied);
        }
        Ok(())
    }

    fn caller_limits(
        &self,
        caller: &CallerGrant,
        timeout_ms: Option<NonZeroU64>,
        limits: LimitOverrides,
    ) -> ExecutionLimits {
        ExecutionLimits::for_capability(timeout_ms)
            .tighten(self.operator.limits)
            .tighten(caller.limits)
            .tighten(limits)
    }

    fn evaluate_adapter(
        &self,
        capability_id: &str,
        operation: Operation,
        target: &VerifiedTarget<'_>,
        caller: &CallerGrant,
        mut limits: ExecutionLimits,
    ) -> Result<(Option<&str>, ExecutionLimits), DenyReason> {
        if operation != Operation::from(target) {
            return Err(DenyReason::OperationMismatch);
        }
        let facts = target.checked().ok_or(DenyReason::InvalidTarget)?;
        let mut grants = self.grants_for(capability_id, operation).peekable();
        if grants.peek().is_none() {
            return Err(DenyReason::GrantRequired);
        }
        let mut grants = grants
            .filter(|(requested, _)| caller.grant_ids.contains(&requested.id))
            .peekable();
        if grants.peek().is_none() {
            return Err(DenyReason::CallerDenied);
        }
        let mut grants = grants
            .filter(|(requested, approved)| {
                requested.target.matches(&facts) && approved.grant.target.matches(&facts)
            })
            .peekable();
        if grants.peek().is_none() {
            return Err(DenyReason::TargetDenied);
        }
        let mut grant_id = None;
        for (requested, approved) in
            grants.filter(|(_, approved)| approved.permits_redirect(target))
        {
            grant_id.get_or_insert(requested.id.as_str());
            limits = limits
                .tighten(requested.limits.unwrap_or_default())
                .tighten(approved.grant.limits.unwrap_or_default());
        }
        if grant_id.is_none() {
            return Err(DenyReason::RedirectDenied);
        }
        Ok((grant_id, limits))
    }

    fn grants_for(
        &self,
        capability_id: &str,
        operation: Operation,
    ) -> impl Iterator<Item = (&Grant, &ApprovedGrant)> {
        self.grants.values().filter_map(move |(requested, index)| {
            (requested.capability_id == capability_id && requested.operation == operation)
                .then(|| (requested, &self.operator.approved_grants[*index]))
        })
    }

    fn caller(&self, principal: &Principal<'_>, now: Instant) -> Result<&CallerGrant, DenyReason> {
        let rules = &self.operator.caller_grants;
        match principal {
            Principal::Anonymous => Err(DenyReason::PrincipalRequired),
            Principal::Local { operator } => rules.iter().find(|rule| {
                matches!(&rule.identity, CallerIdentity::Local { operator: id } if id == operator)
            }).ok_or(DenyReason::CallerDenied),
            Principal::Remote { issuer, subject, client_id, scopes, expires_at } => {
                if now >= *expires_at {
                    return Err(DenyReason::AuthorizationExpired);
                }
                let rule = rules.iter().filter_map(|rule| match &rule.identity {
                    CallerIdentity::Remote { issuer: id_issuer, subject: id_subject, client_id: id_client }
                        if id_issuer == issuer && id_subject == subject
                            && id_client.as_deref().is_none_or(|id| Some(id) == *client_id) => {
                        Some((id_client.is_some(), rule))
                    }
                    _ => None,
                }).max_by_key(|(specific, _)| *specific).map(|(_, rule)| rule).ok_or(DenyReason::CallerDenied)?;
                if !rule.required_scopes.iter().all(|required| scopes.contains(&required.as_str())) {
                    return Err(DenyReason::InsufficientScope);
                }
                Ok(rule)
            }
        }
    }
}

fn call_ceiling(
    principal: &Principal<'_>,
    limits: ExecutionLimits,
    admitted_at: Instant,
    now: Instant,
) -> Result<ExecutionCeiling, DenyReason> {
    if admitted_at > now {
        return Err(DenyReason::InvalidContext);
    }
    let mut deadline = admitted_at
        .checked_add(limits.timeout())
        .ok_or(DenyReason::InvalidContext)?;
    if let Principal::Remote { expires_at, .. } = principal {
        deadline = deadline.min(*expires_at);
    }
    if now >= deadline {
        return Err(DenyReason::DeadlineExceeded);
    }
    Ok(ExecutionCeiling { limits, deadline })
}

fn validate_approval<'a>(
    requested_by_id: &BTreeMap<&str, &'a Grant>,
    approved: &ApprovedGrant,
) -> Result<&'a Grant, PolicyInputError> {
    approved.grant.validate()?;
    let requested = requested_by_id
        .get(approved.grant.id.as_str())
        .copied()
        .ok_or_else(|| PolicyInputError::new("/id", "policy.unknown_approved_grant"))?;
    if requested.capability_id != approved.grant.capability_id
        || requested.operation != approved.grant.operation
        || std::mem::discriminant(&requested.target)
            != std::mem::discriminant(&approved.grant.target)
    {
        return Err(PolicyInputError::new("/id", "policy.approval_mismatch"));
    }
    if !approved.redirects.is_empty()
        && (!matches!(approved.grant.target, GrantTarget::Rest { .. })
            || approved.redirects.iter().any(|route| !route.is_valid()))
    {
        return Err(PolicyInputError::new(
            "/redirects",
            "policy.invalid_redirect",
        ));
    }
    Ok(requested)
}

fn validate_callers(operator: &OperatorPolicy) -> Result<(), PolicyInputError> {
    let mut identities = BTreeSet::new();
    for (index, rule) in operator.caller_grants.iter().enumerate() {
        let invalid =
            |field, code| PolicyInputError::new(field, code).at_grant("/callerGrants", index);
        let is_valid_identity = match &rule.identity {
            CallerIdentity::Local { operator } => {
                valid_identity_value(operator) && rule.required_scopes.is_empty()
            }
            CallerIdentity::Remote {
                issuer,
                subject,
                client_id,
            } => {
                valid_identity_value(issuer)
                    && valid_identity_value(subject)
                    && client_id.as_deref().is_none_or(valid_identity_value)
            }
        };
        if !is_valid_identity || !identities.insert(&rule.identity) {
            return Err(invalid("/identity", "policy.invalid_caller_identity"));
        }
        if !rule.capability_ids.iter().all(|id| is_key(id)) {
            return Err(invalid("/capabilityIds", "policy.invalid_key"));
        }
        if !rule.grant_ids.iter().all(|id| is_key(id)) {
            return Err(invalid("/grantIds", "policy.invalid_key"));
        }
        if rule.required_scopes.iter().any(|scope| {
            scope.is_empty()
                || !scope
                    .bytes()
                    .all(|byte| matches!(byte, b'!' | b'#'..=b'[' | b']'..=b'~'))
        }) {
            return Err(invalid("/requiredScopes", "policy.invalid_scope"));
        }
    }
    Ok(())
}

fn valid_identity_value(value: &str) -> bool {
    !value.trim().is_empty() && !value.chars().any(char::is_control)
}

/// Authorization for one concrete action. Use the deadline and budgets during execution.
#[must_use]
#[derive(Debug)]
pub struct Permit<'a> {
    grant_id: Option<&'a str>,
    ceiling: ExecutionCeiling,
}

impl<'a> Permit<'a> {
    pub const REASON: &'static str = "policy.allowed";

    pub fn grant_id(&self) -> Option<&'a str> {
        self.grant_id
    }

    pub fn limits(&self) -> ExecutionLimits {
        self.ceiling.limits
    }

    pub fn deadline(&self) -> Instant {
        self.ceiling.deadline
    }

    pub fn ceiling(&self) -> ExecutionCeiling {
        self.ceiling
    }
}

/// Stable denial reasons suitable for diagnostics without including any request data.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DenyReason {
    InvalidContext,
    PrincipalRequired,
    AuthorizationExpired,
    InsufficientScope,
    CallerDenied,
    CapabilityDenied,
    OperationMismatch,
    InvalidTarget,
    GrantRequired,
    TargetDenied,
    RedirectDenied,
    DeadlineExceeded,
}

impl DenyReason {
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidContext => "policy.invalid_context",
            Self::PrincipalRequired => "policy.principal_required",
            Self::AuthorizationExpired => "auth.expired",
            Self::InsufficientScope => "auth.insufficient_scope",
            Self::CallerDenied => "policy.caller_denied",
            Self::CapabilityDenied => "policy.capability_denied",
            Self::OperationMismatch => "policy.operation_mismatch",
            Self::InvalidTarget => "policy.invalid_target",
            Self::GrantRequired => "policy.grant_required",
            Self::TargetDenied => "policy.target_denied",
            Self::RedirectDenied => "policy.redirect_denied",
            Self::DeadlineExceeded => "execution.deadline_exceeded",
        }
    }
}

impl Serialize for DenyReason {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.code())
    }
}

impl fmt::Display for DenyReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for DenyReason {}

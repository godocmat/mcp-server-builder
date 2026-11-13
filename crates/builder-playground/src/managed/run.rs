use std::collections::BTreeSet;
use std::time::Instant;

use builder_project::{Capability, Invocation, ValidatedManifest};
use mcp_policy::{Effect, ExecutionLimits, Operation};
use mcp_runtime::bindings::StarterBinding;
use serde::Serialize;
use serde_json::Value;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use super::{ManagedSession, PreviewError, PreviewMode};
use crate::client::{ClientError, ProtocolRevision, RequestOptions, redact_result};
use crate::scenario::{
    AssertionResult, RunOutcome, ScenarioEvaluation, evaluate_with_redactor, outcome_for_error,
};
use crate::trace::TraceSummary;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RunTarget {
    ManagedFixture,
    ManagedLive,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunDiagnostic {
    pub code: &'static str,
    pub message: &'static str,
    pub correlation_id: Uuid,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunResultV1 {
    pub version: u32,
    pub run_id: Uuid,
    pub correlation_id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scenario_id: Option<String>,
    pub target: RunTarget,
    pub transport: &'static str,
    pub protocol_revision: ProtocolRevision,
    pub started_at: String,
    pub duration_ms: u64,
    pub outcome: RunOutcome,
    pub effect: Effect,
    pub assertions: Vec<AssertionResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<RunDiagnostic>,
    pub trace_summary: TraceSummary,
}

/// The result record carries only metadata. The sanitized display payload is transient.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewRun {
    pub result: RunResultV1,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
}

impl ManagedSession {
    /// Replay only the scenario selected in this connection's launch review.
    pub async fn replay(&self, options: RequestOptions) -> Result<PreviewRun, PreviewError> {
        let scenario = self
            .scenario
            .as_ref()
            .ok_or(PreviewError::ScenarioRequired)?;
        Ok(self.run(&scenario.call, options, true).await)
    }

    /// Make an ad hoc call on a reviewed live connection.
    pub async fn invoke(
        &self,
        call: &Invocation,
        options: RequestOptions,
    ) -> Result<PreviewRun, PreviewError> {
        if self.mode.is_fixture() || self.scenario.is_some() {
            return Err(PreviewError::ScenarioRequired);
        }
        Ok(self.run(call, options, false).await)
    }

    async fn run(
        &self,
        call: &Invocation,
        mut options: RequestOptions,
        replay: bool,
    ) -> PreviewRun {
        let started_at = OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .expect("current UTC time fits RFC 3339");
        let started = Instant::now();
        let correlation_id = options.correlation_id;
        options.timeout = options.timeout.min(ExecutionLimits::MAX_TIMEOUT);
        // Callers cannot override the managed session's credential and path redaction.
        options.redactor = None;
        let response = self.client.invoke(call, options).await;
        let wire_error = response.as_ref().err();
        let code = wire_error.map(ClientError::code);
        let (outcome, assertions, diagnostic) =
            if let Some(scenario) = self.scenario.as_ref().filter(|_| replay) {
                let evaluated = evaluate_with_redactor(
                    scenario,
                    response.as_ref().map_err(ClientError::code),
                    &self.redactor,
                );
                let ScenarioEvaluation {
                    outcome,
                    assertions,
                    diagnostic_code,
                } = evaluated;
                let diagnostic = diagnostic_code.or(code.filter(|_| outcome != RunOutcome::Passed));
                (outcome, assertions, diagnostic)
            } else {
                (
                    code.map_or(RunOutcome::Passed, outcome_for_error),
                    Vec::new(),
                    code,
                )
            };
        let effect = if matches!(call, Invocation::Tool { name, .. } if self.write_tools.contains(name))
            && !matches!(code, Some("input.invalid" | "mcp.unknown_tool"))
        {
            // The starter adapter and MCP result cannot confirm an upstream commit.
            Effect::Unknown
        } else {
            Effect::None
        };
        let diagnostic = diagnostic.map(|code| RunDiagnostic {
            code,
            message: wire_error
                .filter(|error| error.code() == code)
                .map_or_else(|| scenario_message(code), ClientError::message),
            correlation_id,
        });
        let payload = response.as_ref().ok().and_then(|value| {
            redact_result(
                value,
                &self.redactor,
                ExecutionLimits::MAX_OUTPUT_BYTES as usize,
            )
            .ok()
        });
        let result = RunResultV1 {
            version: 1,
            run_id: Uuid::new_v4(),
            correlation_id,
            scenario_id: self.scenario.as_ref().filter(|_| replay).and_then(|item| {
                let id = self.redactor.redact_text(&item.id, 256);
                (!id.truncated && id.text == item.id).then(|| item.id.clone())
            }),
            target: match self.mode {
                PreviewMode::Fixture { .. } => RunTarget::ManagedFixture,
                PreviewMode::Live { .. } => RunTarget::ManagedLive,
            },
            transport: "stdio",
            protocol_revision: self.client.protocol_revision(),
            started_at,
            duration_ms: started.elapsed().as_millis() as u64,
            outcome,
            effect,
            assertions,
            diagnostic,
            trace_summary: self.client.trace_summary(),
        };
        PreviewRun { result, payload }
    }
}

pub(super) fn write_tools(
    manifest: &ValidatedManifest,
    grant_ids: &BTreeSet<String>,
) -> BTreeSet<String> {
    let document = manifest.document();
    let writes: BTreeSet<_> = document
        .policy
        .grants
        .iter()
        .filter(|grant| grant.operation == Operation::HttpWrite && grant_ids.contains(&grant.id))
        .map(|grant| &grant.capability_id)
        .collect();
    document
        .capabilities
        .iter()
        .filter_map(|capability| match capability {
            Capability::Tool(tool) if writes.contains(&tool.id) => Some(tool.name.clone()),
            _ => None,
        })
        .collect()
}

pub(super) fn is_mutating_call(manifest: &ValidatedManifest, call: &Invocation) -> bool {
    let Invocation::Tool { name, .. } = call else {
        return false;
    };
    let document = manifest.document();
    let Some(tool) = document
        .capabilities
        .iter()
        .find_map(|capability| match capability {
            Capability::Tool(tool) if &tool.name == name => Some(tool),
            _ => None,
        })
    else {
        return false;
    };
    matches!(
        builder_project::runtime_bindings(manifest).get(&tool.binding_id),
        Some(StarterBinding::Rest(binding)) if !binding.method.is_read()
    )
}

fn scenario_message(code: &str) -> &'static str {
    match code {
        "scenario.assertion_failed" => "The result differs from the saved scenario assertions.",
        "scenario.invalid" => "Check the saved scenario assertions.",
        "runtime.output_limit" => "The result exceeds the allowed output size.",
        _ => "The scenario did not pass.",
    }
}

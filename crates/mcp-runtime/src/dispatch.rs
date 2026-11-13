use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Mutex;
use std::time::Instant;

use mcp_policy::{Action, Effect, ExecutionCeiling, ExecutionLimits, OperationContext};
use serde::Serialize;
use serde_json::{Map, Value};

use crate::adapter::TargetState;
use crate::execution::{json_depth_is_bounded, serialized_len};
use crate::syntax::ResourceAddress;
use crate::{
    AdapterBinding, AdapterContext, AdapterOutput, CancellationToken, ErrorKind, ExecutionContext,
    Invocation, InvocationInput, PreparationContext, PreparedTarget, PromptMessage, PromptResult,
    ResourceContent, ResourceResult, Runtime, RuntimeError, TextContent, ToolResult, prompt,
};

impl Runtime {
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: Map<String, Value>,
        context: &ExecutionContext<'_>,
    ) -> Result<ToolResult, RuntimeError> {
        let now = Instant::now();
        let tool = self
            .tools
            .get(name)
            .filter(|tool| {
                let def = &tool.definition;
                self.adapter_visible(context.principal, &def.id, &def.binding, now)
            })
            .ok_or_else(|| context.error(ErrorKind::UnknownTool, Effect::None))?;
        check_cancelled(context, Effect::None)?;
        let arguments = Value::Object(arguments);
        if !json_depth_is_bounded(&arguments, 64)
            || !fits(&arguments, ExecutionLimits::MAX_INPUT_BYTES)
            || !tool.input_schema.is_valid(&arguments)
        {
            return Err(context.error(ErrorKind::InvalidArguments, Effect::None));
        }
        let arguments = arguments.as_object().expect("tool input is an object");
        let definition = &tool.definition;
        let invocation = Invocation {
            capability_id: &definition.id,
            binding_id: &definition.binding.id,
            input: InvocationInput::Tool(arguments),
        };
        let (output, ceiling) = self
            .invoke(invocation, &definition.binding, context)
            .await?;
        let AdapterOutput::Tool {
            text,
            structured_content,
            effect,
        } = output
        else {
            return Err(context.error(ErrorKind::InvalidOutput, Effect::Unknown));
        };
        check_finished(context, ceiling, effect)?;
        let max_output = ceiling.limits.max_output_bytes();
        let structured = structured_content.map(Value::Object);
        if text
            .as_ref()
            .is_some_and(|text| text.len() as u64 > max_output)
            || structured
                .as_ref()
                .is_some_and(|value| !json_depth_is_bounded(value, 64) || !fits(value, max_output))
        {
            return Err(context.error(ErrorKind::OutputLimit, effect));
        }
        if let Some(schema) = &tool.output_schema
            && !structured
                .as_ref()
                .is_some_and(|output| schema.is_valid(output))
        {
            return Err(context.error(ErrorKind::InvalidOutput, effect));
        }
        let mut content = Vec::new();
        if let Some(text) = text {
            content.push(TextContent { text });
        }
        if let Some(output) = &structured {
            content.push(TextContent {
                text: output.to_string(),
            });
        }
        if content.is_empty() {
            return Err(context.error(ErrorKind::InvalidOutput, effect));
        }
        let result = ToolResult {
            content,
            structured_content: structured.and_then(|value| match value {
                Value::Object(object) => Some(object),
                _ => None,
            }),
        };
        finish(result, context, ceiling, effect)
    }

    pub async fn read_resource(
        &self,
        uri: &str,
        context: &ExecutionContext<'_>,
    ) -> Result<ResourceResult, RuntimeError> {
        let unknown = || context.error(ErrorKind::UnknownResource, Effect::None);
        let address = ResourceAddress::parse(uri, false).ok_or_else(unknown)?;
        let now = Instant::now();
        let resource = self.resources.values().find(|item| {
            self.adapter_visible(
                context.principal,
                &item.definition.id,
                &item.definition.binding,
                now,
            ) && item.address.overlaps(&address)
        });
        let (capability_id, binding, mime_type, variables) = if let Some(resource) = resource {
            check_cancelled(context, Effect::None)?;
            let def = &resource.definition;
            (
                def.id.as_str(),
                &def.binding,
                def.info.mime_type.as_str(),
                BTreeMap::new(),
            )
        } else {
            let template = self
                .resource_templates
                .values()
                .find(|item| {
                    self.adapter_visible(
                        context.principal,
                        &item.definition.id,
                        &item.definition.binding,
                        now,
                    ) && item.address.overlaps(&address)
                })
                .ok_or_else(unknown)?;
            check_cancelled(context, Effect::None)?;
            let invalid = || context.error(ErrorKind::InvalidArguments, Effect::None);
            let variables = template.address.capture(&address).ok_or_else(invalid)?;
            if !template.variables.iter().all(|(key, schema)| {
                variables
                    .get(key)
                    .is_some_and(|value| schema.is_valid(&Value::String(value.clone())))
            }) {
                return Err(invalid());
            }
            let def = &template.definition;
            (
                def.id.as_str(),
                &def.binding,
                def.info.mime_type.as_str(),
                variables,
            )
        };
        let invocation = Invocation {
            capability_id,
            binding_id: &binding.id,
            input: InvocationInput::Resource {
                uri,
                variables: &variables,
            },
        };
        let (output, ceiling) = self.invoke(invocation, binding, context).await?;
        let text = match output {
            AdapterOutput::Resource { text } => text,
            AdapterOutput::Tool { effect, .. } => {
                return Err(context.error(ErrorKind::InvalidOutput, effect));
            }
        };
        finish(
            ResourceResult {
                contents: vec![ResourceContent {
                    uri: uri.into(),
                    mime_type: mime_type.into(),
                    text,
                }],
            },
            context,
            ceiling,
            Effect::None,
        )
    }

    pub async fn get_prompt(
        &self,
        name: &str,
        arguments: BTreeMap<String, String>,
        context: &ExecutionContext<'_>,
    ) -> Result<PromptResult, RuntimeError> {
        let now = Instant::now();
        let prompt = self
            .prompts
            .get(name)
            .filter(|item| self.prompt_visible(context.principal, &item.definition.id, now))
            .ok_or_else(|| context.error(ErrorKind::UnknownPrompt, Effect::None))?;
        check_cancelled(context, Effect::None)?;
        if !fits(&arguments, ExecutionLimits::MAX_INPUT_BYTES)
            || arguments
                .keys()
                .any(|name| !prompt.arguments.contains_key(name))
            || prompt
                .arguments
                .iter()
                .any(|(name, required)| *required && !arguments.contains_key(name))
        {
            return Err(context.error(ErrorKind::InvalidArguments, Effect::None));
        }
        let ceiling = self
            .policy
            .evaluate(&OperationContext {
                principal: context.principal,
                capability_id: &prompt.definition.id,
                action: Action::RenderPrompt,
                capability_timeout_ms: None,
                requested_limits: context.limits,
                ceiling: None,
                admitted_at: context.admitted_at,
                now: Instant::now(),
            })
            .map_err(|reason| context.denial(reason))?
            .ceiling();
        let mut budget = ceiling.limits.output_budget();
        let mut messages = Vec::new();
        for template in &prompt.definition.messages {
            check_finished(context, ceiling, Effect::None)?;
            let text = prompt::render(&template.text_template, &arguments, &mut budget)
                .ok_or_else(|| context.error(ErrorKind::OutputLimit, Effect::None))?;
            messages.push(PromptMessage {
                role: template.role,
                content: TextContent { text },
            });
        }
        finish(
            PromptResult {
                description: Some(prompt.definition.info.description.clone()),
                messages,
            },
            context,
            ceiling,
            Effect::None,
        )
    }

    async fn invoke(
        &self,
        invocation: Invocation<'_>,
        binding: &AdapterBinding,
        context: &ExecutionContext<'_>,
    ) -> Result<(AdapterOutput, ExecutionCeiling), RuntimeError> {
        let ceiling = self
            .policy
            .preparation_ceiling(
                context.principal,
                invocation.capability_id,
                binding.timeout_ms,
                context.limits,
                context.admitted_at,
                Instant::now(),
            )
            .map_err(|reason| context.denial(reason))?;
        check_finished(context, ceiling, Effect::None)?;
        let cancellation = context.cancellation.child_token();
        let _guard = cancellation.clone().drop_guard();
        let preparation = PreparationContext {
            ceiling,
            cancellation,
            correlation_id: context.correlation_id,
        };
        let capability_id = invocation.capability_id;
        let prepared = wait_for_preparation(
            async { self.adapter.prepare(invocation, &preparation).await },
            &preparation.cancellation,
            ceiling.deadline,
        )
        .await
        .map_err(|kind| context.error(kind, Effect::None))?
        .map_err(|error| context.error(error.kind.into(), Effect::None))?;
        check_finished(context, ceiling, Effect::None)?;
        let target = prepared.target();
        let ceiling = self
            .policy
            .evaluate(&OperationContext {
                principal: context.principal,
                capability_id,
                action: match &target {
                    PreparedTarget::Verified(target) => Action::Adapter {
                        operation: binding.operation,
                        target,
                    },
                    PreparedTarget::RestRoute(route) => Action::PreflightRest {
                        operation: binding.operation,
                        route,
                        redirected: false,
                    },
                },
                capability_timeout_ms: binding.timeout_ms,
                requested_limits: context.limits,
                ceiling: Some(ceiling),
                admitted_at: context.admitted_at,
                now: Instant::now(),
            })
            .map_err(|reason| context.denial(reason))?
            .ceiling();
        check_finished(context, ceiling, Effect::None)?;
        let execution = AdapterContext {
            preparation: &preparation,
            policy: &self.policy,
            principal: context.principal,
            capability_id,
            operation: binding.operation,
            timeout_ms: binding.timeout_ms,
            requested_limits: context.limits,
            admitted_at: context.admitted_at,
            state: Mutex::new(TargetState {
                ceiling,
                denial: None,
                peer_pending: matches!(target, PreparedTarget::RestRoute(_)),
            }),
            progress: context.progress,
            progress_state: Default::default(),
            deadline_changed: Default::default(),
        };
        let handler = async { prepared.execute(&execution).await };
        tokio::pin!(handler);
        let output = loop {
            let deadline = execution.deadline();
            tokio::select! {
                biased;
                () = execution.cancellation().cancelled() => {
                    execution.cancellation().cancel();
                    return Err(context.error(ErrorKind::Cancelled, Effect::Unknown));
                }
                () = execution.deadline_changed.notified() => continue,
                () = tokio::time::sleep_until(deadline.into()) => {
                    execution.cancellation().cancel();
                    return Err(context.error(ErrorKind::TimedOut, Effect::Unknown));
                }
                output = &mut handler => break output,
            }
        };
        let effect = match &output {
            Ok(AdapterOutput::Tool { effect, .. }) => *effect,
            Ok(AdapterOutput::Resource { .. }) => Effect::None,
            Err(error) => error.effect,
        };
        let ceiling = execution.current_ceiling();
        check_finished(context, ceiling, effect)?;
        if let Some(reason) = execution.recheck_denial() {
            return Err(RuntimeError {
                effect,
                ..context.denial(reason)
            });
        }
        if output.is_ok() && execution.peer_pending() {
            return Err(RuntimeError {
                effect,
                ..context.denial(mcp_policy::DenyReason::InvalidTarget)
            });
        }
        output
            .map(|output| (output, ceiling))
            .map_err(|error| context.error(error.kind.into(), effect))
    }
}

async fn wait_for_preparation<T>(
    future: impl Future<Output = T>,
    cancellation: &CancellationToken,
    deadline: Instant,
) -> Result<T, ErrorKind> {
    tokio::select! {
        biased;
        () = cancellation.cancelled() => Err(ErrorKind::Cancelled),
        () = tokio::time::sleep_until(deadline.into()) => {
            cancellation.cancel();
            Err(ErrorKind::TimedOut)
        }
        result = future => Ok(result),
    }
}

fn fits(value: &impl Serialize, bytes: u64) -> bool {
    serialized_len(value, usize::try_from(bytes).unwrap_or(usize::MAX)).is_some()
}

fn check_finished(
    context: &ExecutionContext<'_>,
    ceiling: ExecutionCeiling,
    effect: Effect,
) -> Result<(), RuntimeError> {
    check_cancelled(context, effect)?;
    if Instant::now() >= ceiling.deadline {
        return Err(context.error(ErrorKind::TimedOut, effect));
    }
    Ok(())
}

fn check_cancelled(context: &ExecutionContext<'_>, effect: Effect) -> Result<(), RuntimeError> {
    if context.cancellation.is_cancelled() {
        return Err(context.error(ErrorKind::Cancelled, effect));
    }
    Ok(())
}

fn finish<T: Serialize>(
    value: T,
    context: &ExecutionContext<'_>,
    ceiling: ExecutionCeiling,
    effect: Effect,
) -> Result<T, RuntimeError> {
    check_finished(context, ceiling, effect)?;
    if !fits(&value, ceiling.limits.max_output_bytes()) {
        return Err(context.error(ErrorKind::OutputLimit, effect));
    }
    check_finished(context, ceiling, effect)?;
    Ok(value)
}

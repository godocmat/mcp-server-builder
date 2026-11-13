use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Instant;

use jsonschema::Validator;
use mcp_policy::{CapabilityAction, Effect, OperatorPolicy, Policy};
use serde::Serialize;
use uuid::Uuid;

use crate::pagination::{ListKind, Pagination};
use crate::syntax::{ResourceAddress, is_key, is_protocol_name, is_variable};
use crate::{
    Adapter, AdapterBinding, Capabilities, Page, Principal, PromptDefinition, PromptInfo,
    ResourceDefinition, ResourceInfo, ResourceTemplateDefinition, ResourceTemplateInfo,
    RuntimeError, ServerDefinition, ServerInfo, ToolDefinition, ToolInfo, compile_schema,
    compile_tool_schema, prompt_variables,
};

const MAX_CAPABILITIES: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DefinitionErrorKind {
    Policy,
    InvalidCapability,
    DuplicateCapability,
    AmbiguousResource,
    InvalidSchema,
}

#[derive(Debug, PartialEq, Eq)]
pub struct DefinitionError {
    pub kind: DefinitionErrorKind,
    pub field: String,
}

impl std::fmt::Display for DefinitionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "Invalid runtime definition ({:?}) at {}",
            self.kind, self.field
        )
    }
}

impl std::error::Error for DefinitionError {}

pub(crate) struct Tool {
    pub(crate) definition: ToolDefinition,
    pub(crate) input_schema: Validator,
    pub(crate) output_schema: Option<Validator>,
}

pub(crate) struct Resource {
    pub(crate) definition: ResourceDefinition,
    pub(crate) address: ResourceAddress,
}

pub(crate) struct ResourceTemplate {
    pub(crate) definition: ResourceTemplateDefinition,
    pub(crate) address: ResourceAddress,
    pub(crate) variables: BTreeMap<String, Validator>,
}

pub(crate) struct Prompt {
    pub(crate) definition: PromptDefinition,
    pub(crate) arguments: BTreeMap<String, bool>,
}

pub struct Runtime {
    info: ServerInfo,
    pub(crate) policy: Policy,
    pub(crate) adapter: Arc<dyn Adapter>,
    pub(crate) tools: BTreeMap<String, Tool>,
    pub(crate) resources: BTreeMap<String, Resource>,
    pub(crate) resource_templates: BTreeMap<String, ResourceTemplate>,
    pub(crate) prompts: BTreeMap<String, Prompt>,
    pagination: Pagination,
}

impl Runtime {
    pub fn new(
        definition: ServerDefinition,
        operator: OperatorPolicy,
        adapter: Arc<dyn Adapter>,
    ) -> Result<Self, DefinitionError> {
        use DefinitionErrorKind as Kind;
        let error = |kind, field: &str| DefinitionError {
            kind,
            field: field.into(),
        };
        let count = definition
            .tools
            .len()
            .saturating_add(definition.resources.len())
            .saturating_add(definition.resource_templates.len())
            .saturating_add(definition.prompts.len());
        if count > MAX_CAPABILITIES {
            return Err(error(Kind::InvalidCapability, "capabilities"));
        }
        let mut ids = BTreeSet::new();
        let mut tools = BTreeMap::new();
        for tool in definition.tools {
            check_binding(&tool.binding, "tools/binding")?;
            register_id(&mut ids, &tool.id, "tools/id")?;
            if !is_protocol_name(&tool.info.name) {
                return Err(error(Kind::InvalidCapability, "tools/name"));
            }
            let input_schema = compile_tool_schema(&tool.info.input_schema)
                .map_err(|_| error(Kind::InvalidSchema, "tools/inputSchema"))?;
            let output_schema = tool
                .info
                .output_schema
                .as_ref()
                .map(compile_tool_schema)
                .transpose()
                .map_err(|_| error(Kind::InvalidSchema, "tools/outputSchema"))?;
            if tools
                .insert(
                    tool.info.name.clone(),
                    Tool {
                        definition: tool,
                        input_schema,
                        output_schema,
                    },
                )
                .is_some()
            {
                return Err(error(Kind::DuplicateCapability, "tools/name"));
            }
        }
        let mut resources = BTreeMap::new();
        let mut resource_templates = BTreeMap::new();
        let mut addresses: Vec<ResourceAddress> = Vec::new();
        for resource in definition.resources {
            check_binding(&resource.binding, "resources/binding")?;
            register_id(&mut ids, &resource.id, "resources/id")?;
            let address =
                register_address(&mut addresses, &resource.info.uri, false, "resources/uri")?;
            resources.insert(
                resource.info.uri.clone(),
                Resource {
                    definition: resource,
                    address,
                },
            );
        }
        for template in definition.resource_templates {
            check_binding(&template.binding, "resourceTemplates/binding")?;
            register_id(&mut ids, &template.id, "resourceTemplates/id")?;
            let address = register_address(
                &mut addresses,
                &template.info.uri_template,
                true,
                "resourceTemplates/uriTemplate",
            )?;
            if address
                .variables()
                .into_iter()
                .ne(template.variables.keys().map(String::as_str))
            {
                return Err(error(
                    Kind::InvalidCapability,
                    "resourceTemplates/variables",
                ));
            }
            let variables = template
                .variables
                .iter()
                .map(|(name, schema)| {
                    compile_schema(schema)
                        .map(|validator| (name.clone(), validator))
                        .map_err(|_| error(Kind::InvalidSchema, "resourceTemplates/variables"))
                })
                .collect::<Result<_, _>>()?;
            resource_templates.insert(
                template.info.uri_template.clone(),
                ResourceTemplate {
                    definition: template,
                    address,
                    variables,
                },
            );
        }
        let mut prompts = BTreeMap::new();
        for prompt in definition.prompts {
            register_id(&mut ids, &prompt.id, "prompts/id")?;
            if !is_protocol_name(&prompt.info.name) {
                return Err(error(Kind::InvalidCapability, "prompts/name"));
            }
            let arguments = prompt_arguments(&prompt)?;
            if prompts
                .insert(
                    prompt.info.name.clone(),
                    Prompt {
                        definition: prompt,
                        arguments,
                    },
                )
                .is_some()
            {
                return Err(error(Kind::DuplicateCapability, "prompts/name"));
            }
        }
        let policy =
            Policy::from_snapshots(&definition.requested_policy, operator).map_err(|failure| {
                DefinitionError {
                    kind: Kind::Policy,
                    field: failure.field().into(),
                }
            })?;
        Ok(Self {
            info: definition.info,
            policy,
            adapter,
            tools,
            resources,
            resource_templates,
            prompts,
            pagination: Pagination::default(),
        })
    }

    pub fn info(&self) -> &ServerInfo {
        &self.info
    }

    pub fn capabilities(&self, principal: &Principal<'_>) -> Capabilities {
        let now = Instant::now();
        Capabilities {
            tools: self.tools.values().any(|item| {
                self.adapter_visible(
                    principal,
                    &item.definition.id,
                    &item.definition.binding,
                    now,
                )
            }),
            resources: self.resources.values().any(|item| {
                self.adapter_visible(
                    principal,
                    &item.definition.id,
                    &item.definition.binding,
                    now,
                )
            }) || self.resource_templates.values().any(|item| {
                self.adapter_visible(
                    principal,
                    &item.definition.id,
                    &item.definition.binding,
                    now,
                )
            }),
            prompts: self
                .prompts
                .values()
                .any(|prompt| self.prompt_visible(principal, &prompt.definition.id, now)),
        }
    }

    pub fn list_tools(
        &self,
        principal: &Principal<'_>,
        cursor: Option<&str>,
    ) -> Result<Page<'_, ToolInfo>, RuntimeError> {
        let now = Instant::now();
        self.list(
            ListKind::Tools,
            principal,
            cursor,
            self.tools.values().map(|item| {
                let definition = &item.definition;
                self.adapter_visible(principal, &definition.id, &definition.binding, now)
                    .then_some(&definition.info)
            }),
        )
    }

    pub fn list_resources(
        &self,
        principal: &Principal<'_>,
        cursor: Option<&str>,
    ) -> Result<Page<'_, ResourceInfo>, RuntimeError> {
        let now = Instant::now();
        self.list(
            ListKind::Resources,
            principal,
            cursor,
            self.resources.values().map(|item| {
                let definition = &item.definition;
                self.adapter_visible(principal, &definition.id, &definition.binding, now)
                    .then_some(&definition.info)
            }),
        )
    }

    pub fn list_resource_templates(
        &self,
        principal: &Principal<'_>,
        cursor: Option<&str>,
    ) -> Result<Page<'_, ResourceTemplateInfo>, RuntimeError> {
        let now = Instant::now();
        self.list(
            ListKind::ResourceTemplates,
            principal,
            cursor,
            self.resource_templates.values().map(|item| {
                let definition = &item.definition;
                self.adapter_visible(principal, &definition.id, &definition.binding, now)
                    .then_some(&definition.info)
            }),
        )
    }

    pub fn list_prompts(
        &self,
        principal: &Principal<'_>,
        cursor: Option<&str>,
    ) -> Result<Page<'_, PromptInfo>, RuntimeError> {
        let now = Instant::now();
        self.list(
            ListKind::Prompts,
            principal,
            cursor,
            self.prompts.values().map(|item| {
                self.prompt_visible(principal, &item.definition.id, now)
                    .then_some(&item.definition.info)
            }),
        )
    }

    fn list<'a, T: Serialize + 'a>(
        &self,
        kind: ListKind,
        principal: &Principal<'_>,
        cursor: Option<&str>,
        items: impl Iterator<Item = Option<&'a T>>,
    ) -> Result<Page<'a, T>, RuntimeError> {
        let visible = items
            .enumerate()
            .filter_map(|(index, info)| Some((index, info?)))
            .collect();
        self.pagination
            .page(kind, principal, cursor, visible)
            .map_err(|kind| RuntimeError {
                kind,
                correlation_id: Uuid::new_v4(),
                effect: Effect::None,
                denial: None,
            })
    }

    pub(crate) fn adapter_visible(
        &self,
        principal: &Principal<'_>,
        id: &str,
        binding: &AdapterBinding,
        now: Instant,
    ) -> bool {
        self.policy.is_visible(
            principal,
            id,
            CapabilityAction::Adapter(binding.operation),
            now,
        )
    }

    pub(crate) fn prompt_visible(&self, principal: &Principal<'_>, id: &str, now: Instant) -> bool {
        self.policy
            .is_visible(principal, id, CapabilityAction::RenderPrompt, now)
    }
}

fn register_id(ids: &mut BTreeSet<String>, id: &str, field: &str) -> Result<(), DefinitionError> {
    let kind = if !is_key(id) {
        DefinitionErrorKind::InvalidCapability
    } else if !ids.insert(id.to_owned()) {
        DefinitionErrorKind::DuplicateCapability
    } else {
        return Ok(());
    };
    Err(DefinitionError {
        kind,
        field: field.into(),
    })
}

fn check_binding(binding: &AdapterBinding, field: &str) -> Result<(), DefinitionError> {
    if !is_key(&binding.id) {
        return Err(DefinitionError {
            kind: DefinitionErrorKind::InvalidCapability,
            field: field.into(),
        });
    }
    Ok(())
}

fn register_address(
    addresses: &mut Vec<ResourceAddress>,
    value: &str,
    template: bool,
    field: &str,
) -> Result<ResourceAddress, DefinitionError> {
    let address = ResourceAddress::parse(value, template).ok_or_else(|| DefinitionError {
        kind: DefinitionErrorKind::InvalidCapability,
        field: field.into(),
    })?;
    if addresses.iter().any(|other| address.overlaps(other)) {
        return Err(DefinitionError {
            kind: DefinitionErrorKind::AmbiguousResource,
            field: field.into(),
        });
    }
    addresses.push(address.clone());
    Ok(address)
}

fn prompt_arguments(prompt: &PromptDefinition) -> Result<BTreeMap<String, bool>, DefinitionError> {
    let mut arguments = BTreeMap::new();
    if prompt.info.arguments.len() > 512
        || !prompt.info.arguments.iter().all(|argument| {
            is_variable(&argument.name)
                && arguments
                    .insert(argument.name.clone(), argument.required)
                    .is_none()
        })
    {
        return Err(DefinitionError {
            kind: DefinitionErrorKind::InvalidCapability,
            field: "prompts/arguments".into(),
        });
    }
    if !(1..=512).contains(&prompt.messages.len())
        || !prompt.messages.iter().all(|message| {
            message.text_template.len() <= 16 * 1024
                && prompt_variables(&message.text_template)
                    .is_some_and(|names| names.into_iter().all(|name| arguments.contains_key(name)))
        })
    {
        return Err(DefinitionError {
            kind: DefinitionErrorKind::InvalidCapability,
            field: "prompts/messages".into(),
        });
    }
    Ok(arguments)
}

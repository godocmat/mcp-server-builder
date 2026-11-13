use std::collections::BTreeMap;
use std::num::NonZeroU64;

use mcp_policy::Operation;
use mcp_runtime::bindings::{FileReadBinding, RestBinding, StarterBinding, credential_header};
use mcp_runtime::{
    AdapterBinding, PromptArgument, PromptDefinition, PromptInfo, PromptTemplateMessage,
    ResourceDefinition, ResourceInfo, ResourceTemplateDefinition, ResourceTemplateInfo, Role,
    ServerDefinition, ServerInfo, ToolDefinition, ToolInfo,
};

use crate::{Binding, Capability, PromptRole, ValidatedManifest, syntax};

/// Project binding configuration for the live starter adapter. This resolves no local inputs.
pub fn runtime_bindings(manifest: &ValidatedManifest) -> BTreeMap<String, StarterBinding> {
    manifest
        .document()
        .bindings
        .iter()
        .map(|binding| {
            let config = match binding {
                Binding::Rest(binding) => StarterBinding::Rest(RestBinding {
                    origin: syntax::http_origin(&binding.origin).expect("validated HTTP origin"),
                    method: syntax::http_method(&binding.method).expect("validated HTTP method"),
                    path_template: binding.path_template.clone(),
                    path_params: binding.path_params.clone(),
                    body_pointer: binding.body_pointer.clone(),
                    headers_from_secrets: binding
                        .headers_from_secrets
                        .iter()
                        .flatten()
                        .map(|(name, reference)| {
                            (
                                credential_header(name).expect("validated header"),
                                reference.clone(),
                            )
                        })
                        .collect(),
                }),
                Binding::FileRead(binding) => StarterBinding::FileRead(FileReadBinding {
                    root_ref: binding.root_ref.clone(),
                    relative_path_template: binding.relative_path_template.clone(),
                }),
            };
            (binding.id().into(), config)
        })
        .collect()
}

/// Project an already validated manifest into the transport-neutral runtime catalog.
pub fn runtime_definition(manifest: &ValidatedManifest) -> ServerDefinition {
    let document = manifest.document();
    let server = &document.server;
    let mut definition = ServerDefinition {
        info: ServerInfo {
            name: server.name.clone(),
            version: server.version.clone(),
            description: server.description.clone(),
            instructions: server.instructions.clone(),
        },
        requested_policy: document.policy.clone(),
        tools: Vec::new(),
        resources: Vec::new(),
        resource_templates: Vec::new(),
        prompts: Vec::new(),
    };
    let bindings: BTreeMap<_, _> = document
        .bindings
        .iter()
        .map(|binding| (binding.id(), binding))
        .collect();
    let runtime_binding = |id: &str, timeout_ms: Option<u64>| {
        let operation = match bindings[id] {
            Binding::Rest(binding) => {
                let method = syntax::http_method(&binding.method).expect("validated HTTP method");
                Operation::from_http_method(method)
            }
            Binding::FileRead(_) => Operation::FileRead,
        };
        AdapterBinding {
            id: id.into(),
            operation,
            timeout_ms: timeout_ms.and_then(NonZeroU64::new),
        }
    };
    for capability in &document.capabilities {
        match capability {
            Capability::Tool(tool) => definition.tools.push(ToolDefinition {
                id: tool.id.clone(),
                info: ToolInfo {
                    name: tool.name.clone(),
                    description: tool.description.clone(),
                    input_schema: tool.input_schema.clone(),
                    output_schema: tool.output_schema.clone(),
                },
                binding: runtime_binding(&tool.binding_id, tool.timeout_ms),
            }),
            Capability::Resource(resource) => definition.resources.push(ResourceDefinition {
                id: resource.id.clone(),
                info: ResourceInfo {
                    name: resource.name.clone(),
                    description: resource.description.clone(),
                    uri: resource.uri.clone(),
                    mime_type: resource.mime_type.clone(),
                },
                binding: runtime_binding(&resource.binding_id, resource.timeout_ms),
            }),
            Capability::ResourceTemplate(template) => {
                definition
                    .resource_templates
                    .push(ResourceTemplateDefinition {
                        id: template.id.clone(),
                        info: ResourceTemplateInfo {
                            name: template.name.clone(),
                            description: template.description.clone(),
                            uri_template: template.uri_template.clone(),
                            mime_type: template.mime_type.clone(),
                        },
                        binding: runtime_binding(&template.binding_id, template.timeout_ms),
                        variables: template.variables.clone(),
                    })
            }
            Capability::Prompt(prompt) => definition.prompts.push(PromptDefinition {
                id: prompt.id.clone(),
                info: PromptInfo {
                    name: prompt.name.clone(),
                    description: prompt.description.clone(),
                    arguments: prompt
                        .arguments
                        .iter()
                        .map(|argument| PromptArgument {
                            name: argument.name.clone(),
                            description: argument.description.clone(),
                            required: argument.required,
                        })
                        .collect(),
                },
                messages: prompt
                    .messages
                    .iter()
                    .map(|message| PromptTemplateMessage {
                        role: match message.role {
                            PromptRole::User => Role::User,
                            PromptRole::Assistant => Role::Assistant,
                        },
                        text_template: message.text_template.clone(),
                    })
                    .collect(),
            }),
        }
    }
    definition
}

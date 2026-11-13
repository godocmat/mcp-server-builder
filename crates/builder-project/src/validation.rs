mod bindings;
mod scenarios;

use std::collections::{BTreeMap, BTreeSet};

use mcp_policy::ExecutionLimits;
use mcp_runtime::{compile_schema, compile_tool_schema};
use serde_json::Value;

use crate::syntax::{
    ResourceAddress, field, file_template, is_key, is_protocol_name, is_variable, prompt_variables,
    variables,
};
use crate::{Binding, Capability, IssueCode, ProjectDocument, Prompt, ValidationIssue};

const MAX_TIMEOUT_MS: u64 = ExecutionLimits::MAX_TIMEOUT.as_secs() * 1000;
const MAX_OUTPUT_BYTES: u64 = ExecutionLimits::MAX_OUTPUT_BYTES;
const MAX_RECORDS: usize = 512;
const MAX_TEXT_BYTES: usize = 16 * 1024;
const MAX_ISSUES: usize = 128;
struct Check {
    issues: Vec<ValidationIssue>,
    base: String,
    capability_id: Option<String>,
}

impl Check {
    fn new() -> Self {
        Self {
            issues: Vec::new(),
            base: String::new(),
            capability_id: None,
        }
    }

    fn at(&mut self, path: impl Into<String>, capability_id: Option<&str>) {
        self.base = path.into();
        self.capability_id = capability_id.filter(|id| is_key(id)).map(str::to_owned);
    }

    fn require(&mut self, condition: bool, code: IssueCode, suffix: &str, message: &'static str) {
        if condition || self.issues.len() > MAX_ISSUES {
            return;
        }
        if self.issues.len() == MAX_ISSUES {
            self.issues.push(ValidationIssue::new(
                IssueCode::Limit,
                "",
                "Additional validation errors were omitted.",
            ));
            return;
        }
        self.issues.push(ValidationIssue {
            code,
            field: format!("{}{suffix}", self.base),
            capability_id: self.capability_id.clone(),
            message,
        });
    }

    fn key(&mut self, value: &str, suffix: &str) {
        self.require(
            is_key(value),
            IssueCode::Field,
            suffix,
            "Use a lowercase ASCII identifier with up to 64 characters.",
        );
    }

    fn unique_id<'a>(&mut self, value: &'a str, ids: &mut BTreeSet<&'a str>) {
        self.key(value, "/id");
        self.require(
            ids.insert(value),
            IssueCode::DuplicateId,
            "/id",
            "This ID is already used in the collection.",
        );
    }

    fn text(&mut self, value: &str, maximum: usize, nonempty: bool, suffix: &str) {
        self.require(
            value.len() <= maximum
                && (!nonempty || !value.trim().is_empty())
                && !value.contains('\0'),
            IssueCode::Field,
            suffix,
            "Use text within the field's size and content bounds.",
        );
    }

    fn timeout(&mut self, value: Option<u64>, suffix: &str) {
        self.require(
            value.is_none_or(|value| (1..=MAX_TIMEOUT_MS).contains(&value)),
            IssueCode::Limit,
            suffix,
            "Use a timeout from 1 to 30000 milliseconds.",
        );
    }

    fn schema(&mut self, value: &Value, object_root: bool, suffix: &str) {
        if object_root && value.get("type").and_then(Value::as_str) != Some("object") {
            self.require(
                false,
                IssueCode::Schema,
                suffix,
                "Declare an object-root schema for the dual-revision tool profile.",
            );
            return;
        }
        let (result, message) = if object_root {
            (
                compile_tool_schema(value),
                "Use a valid JSON Schema 2020-12 tool schema with in-document references and no MCP parameter headers.",
            )
        } else {
            (
                compile_schema(value),
                "Use a valid JSON Schema 2020-12 schema with in-document references.",
            )
        };
        self.require(result.is_ok(), IssueCode::Schema, suffix, message);
    }
}

pub(crate) fn validate(document: &ProjectDocument) -> Vec<ValidationIssue> {
    let mut check = Check::new();
    for (length, field) in [
        (document.capabilities.len(), "/capabilities"),
        (document.bindings.len(), "/bindings"),
        (document.secret_refs.len(), "/secretRefs"),
        (document.policy.grants.len(), "/policy/grants"),
        (document.scenarios.len(), "/scenarios"),
    ] {
        check.require(
            length <= MAX_RECORDS,
            IssueCode::Limit,
            field,
            "This collection can contain at most 512 records.",
        );
    }
    if !check.issues.is_empty() {
        return check.issues;
    }

    check.text(&document.name, 256, true, "/name");
    check.text(&document.server.name, 128, true, "/server/name");
    check.require(
        document.server.version.len() <= 128
            && semver::Version::parse(&document.server.version).is_ok(),
        IssueCode::Field,
        "/server/version",
        "Use a SemVer server version such as 0.1.0.",
    );
    for (text, field) in [
        (&document.server.description, "/server/description"),
        (&document.server.instructions, "/server/instructions"),
    ] {
        if let Some(text) = text {
            check.text(text, MAX_TEXT_BYTES, false, field);
        }
    }

    let mut secrets = BTreeSet::new();
    for (index, reference) in document.secret_refs.iter().enumerate() {
        check.at(format!("/secretRefs/{index}"), None);
        check.unique_id(&reference.id, &mut secrets);
        check.text(&reference.purpose, 1024, true, "/purpose");
    }
    let binding_index = check.bindings(&document.bindings, &secrets);
    let capabilities = check.capabilities(&document.capabilities, &binding_index);
    check.grants(&document.policy, &capabilities.by_id, &binding_index);
    check.scenarios(&document.scenarios, &capabilities, &binding_index);
    check.issues
}

#[derive(Default)]
struct Capabilities<'a> {
    by_id: BTreeMap<&'a str, &'a Capability>,
    tools: BTreeMap<&'a str, &'a Capability>,
    prompts: BTreeMap<&'a str, &'a Capability>,
    resources: Vec<(ResourceAddress, &'a Capability)>,
}

fn binding_id(capability: &Capability) -> Option<&str> {
    match capability {
        Capability::Tool(tool) => Some(&tool.binding_id),
        Capability::Resource(resource) => Some(&resource.binding_id),
        Capability::ResourceTemplate(template) => Some(&template.binding_id),
        Capability::Prompt(_) => None,
    }
}

impl Check {
    fn capabilities<'a>(
        &mut self,
        capabilities: &'a [Capability],
        bindings: &BTreeMap<&str, &Binding>,
    ) -> Capabilities<'a> {
        let mut registry = Capabilities::default();
        let mut ids = BTreeSet::new();
        for (index, capability) in capabilities.iter().enumerate() {
            let id = capability.id();
            self.at(format!("/capabilities/{index}"), Some(id));
            self.unique_id(id, &mut ids);
            registry.by_id.entry(id).or_insert(capability);

            let binding = binding_id(capability).and_then(|id| bindings.get(id).copied());
            match capability {
                Capability::Tool(tool) => {
                    self.protocol_name(tool.name.as_str(), &mut registry.tools, capability);
                    self.text(&tool.description, MAX_TEXT_BYTES, false, "/description");
                    self.schema(&tool.input_schema, true, "/inputSchema");
                    if let Some(schema) = &tool.output_schema {
                        self.schema(schema, true, "/outputSchema");
                    }
                    self.timeout(tool.timeout_ms, "/timeoutMs");
                    self.require(
                        matches!(binding, Some(Binding::Rest(_))),
                        IssueCode::Reference,
                        "/bindingId",
                        "Select a REST binding for this tool.",
                    );
                }
                Capability::Resource(resource) => {
                    self.text(&resource.name, 256, true, "/name");
                    self.text(&resource.description, MAX_TEXT_BYTES, false, "/description");
                    self.mime_type(&resource.mime_type);
                    self.timeout(resource.timeout_ms, "/timeoutMs");
                    let address = ResourceAddress::parse(&resource.uri, false);
                    self.resource_address(address, "/uri", capability, &mut registry);
                    self.file_variables(binding, &BTreeSet::new());
                }
                Capability::ResourceTemplate(template) => {
                    self.text(&template.name, 256, true, "/name");
                    self.text(&template.description, MAX_TEXT_BYTES, false, "/description");
                    self.mime_type(&template.mime_type);
                    self.timeout(template.timeout_ms, "/timeoutMs");
                    let declared: BTreeSet<_> =
                        template.variables.keys().map(String::as_str).collect();
                    let address = ResourceAddress::parse(&template.uri_template, true);
                    self.require(
                        address
                            .as_ref()
                            .is_none_or(|address| address.variables() == declared),
                        IssueCode::Reference,
                        "/variables",
                        "Declare a schema for each URI variable.",
                    );
                    for (index, (name, schema)) in template.variables.iter().enumerate() {
                        if index == MAX_RECORDS {
                            self.require(
                                false,
                                IssueCode::Limit,
                                "/variables",
                                "Too many template variables.",
                            );
                            break;
                        }
                        let suffix = field("/variables", name);
                        self.require(
                            is_variable(name),
                            IssueCode::Field,
                            "/variables",
                            "Use simple named template variables.",
                        );
                        self.schema(schema, false, &suffix);
                    }
                    self.resource_address(address, "/uriTemplate", capability, &mut registry);
                    self.file_variables(binding, &declared);
                }
                Capability::Prompt(prompt) => {
                    self.protocol_name(prompt.name.as_str(), &mut registry.prompts, capability);
                    self.text(&prompt.description, MAX_TEXT_BYTES, false, "/description");
                    self.prompt(prompt);
                }
            }
        }
        registry
    }

    fn protocol_name<'a>(
        &mut self,
        name: &'a str,
        names: &mut BTreeMap<&'a str, &'a Capability>,
        capability: &'a Capability,
    ) {
        self.require(
            is_protocol_name(name),
            IssueCode::Field,
            "/name",
            "Use 1 to 128 ASCII letters, digits, dots, underscores or hyphens.",
        );
        self.require(
            !names.contains_key(name),
            IssueCode::DuplicateId,
            "/name",
            "This name is already used by a capability of the same kind.",
        );
        names.entry(name).or_insert(capability);
    }

    fn mime_type(&mut self, value: &str) {
        self.require(
            value.len() <= 256 && !value.contains('*') && value.parse::<mime::Mime>().is_ok(),
            IssueCode::Field,
            "/mimeType",
            "Use a concrete media type.",
        );
    }

    fn resource_address<'a>(
        &mut self,
        address: Option<ResourceAddress>,
        suffix: &str,
        capability: &'a Capability,
        registry: &mut Capabilities<'a>,
    ) {
        self.require(
            address.is_some(),
            IssueCode::Field,
            suffix,
            "Use an absolute resource URI with simple, distinct path variables for a template.",
        );
        if let Some(address) = address {
            self.require(
                !registry
                    .resources
                    .iter()
                    .any(|(other, _)| address.overlaps(other)),
                IssueCode::AmbiguousResource,
                suffix,
                "This address overlaps a registered resource or template.",
            );
            registry.resources.push((address, capability));
        }
    }

    fn file_variables(&mut self, binding: Option<&Binding>, declared: &BTreeSet<&str>) {
        let Some(Binding::FileRead(binding)) = binding else {
            self.require(
                false,
                IssueCode::Reference,
                "/bindingId",
                "Select a file-read binding.",
            );
            return;
        };
        if let Some(parts) = file_template(&binding.relative_path_template) {
            self.require(
                variables(&parts).is_subset(declared),
                IssueCode::Reference,
                "/bindingId",
                "Bind file path variables to declared resource template variables.",
            );
        }
    }

    fn prompt(&mut self, prompt: &Prompt) {
        self.require(
            prompt.arguments.len() <= MAX_RECORDS,
            IssueCode::Limit,
            "/arguments",
            "Too many prompt arguments.",
        );
        self.require(
            (1..=MAX_RECORDS).contains(&prompt.messages.len()),
            IssueCode::Limit,
            "/messages",
            "Declare at least one prompt message and no more than 512.",
        );
        let mut names = BTreeSet::new();
        for (index, argument) in prompt.arguments.iter().take(MAX_RECORDS).enumerate() {
            let suffix = format!("/arguments/{index}");
            self.require(
                is_variable(&argument.name),
                IssueCode::Field,
                &field(&suffix, "name"),
                "Use a simple prompt argument name.",
            );
            self.require(
                names.insert(argument.name.as_str()),
                IssueCode::DuplicateId,
                &field(&suffix, "name"),
                "This argument name is already declared.",
            );
            if let Some(description) = &argument.description {
                self.text(
                    description,
                    MAX_TEXT_BYTES,
                    false,
                    &field(&suffix, "description"),
                );
            }
        }
        for (index, message) in prompt.messages.iter().take(MAX_RECORDS).enumerate() {
            let suffix = format!("/messages/{index}/textTemplate");
            self.text(&message.text_template, MAX_TEXT_BYTES, false, &suffix);
            self.require(
                prompt_variables(&message.text_template).is_some_and(|used| used.is_subset(&names)),
                IssueCode::Reference,
                &suffix,
                "Use declared prompt arguments in {name} placeholders.",
            );
        }
    }
}

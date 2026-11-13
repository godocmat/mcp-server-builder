use std::collections::{BTreeMap, BTreeSet};

use super::{Capabilities, Check, MAX_OUTPUT_BYTES, MAX_RECORDS, binding_id};
use crate::syntax::{
    ResourceAddress, SegmentTemplate, file_template, http_method, is_key, is_pointer,
    is_portable_path, path_is_within, portable_path_key, rest_template, variables,
};
use crate::{
    Assertion, Binding, Capability, Fixture, Invocation, IssueCode, ScenarioMode, ScenarioV1,
};

impl Check {
    pub(super) fn scenarios(
        &mut self,
        scenarios: &[ScenarioV1],
        capabilities: &Capabilities<'_>,
        bindings: &BTreeMap<&str, &Binding>,
    ) {
        let mut ids = BTreeSet::new();
        for (index, scenario) in scenarios.iter().enumerate() {
            let base = format!("/scenarios/{index}");
            let (capability, target_field) = match &scenario.call {
                Invocation::Tool { name, .. } => {
                    (capabilities.tools.get(name.as_str()).copied(), "name")
                }
                Invocation::Prompt { name, .. } => {
                    (capabilities.prompts.get(name.as_str()).copied(), "name")
                }
                Invocation::Resource { uri } => (
                    ResourceAddress::parse(uri, false).and_then(|address| {
                        capabilities
                            .resources
                            .iter()
                            .find_map(|(candidate, capability)| {
                                candidate.overlaps(&address).then_some(*capability)
                            })
                    }),
                    "uri",
                ),
            };
            self.at(&base, capability.map(Capability::id));
            self.unique_id(&scenario.id, &mut ids);
            self.require(
                scenario.version == 1,
                IssueCode::UnsupportedSetting,
                "/version",
                "Use scenario version 1.",
            );
            self.text(&scenario.name, 256, true, "/name");
            self.require(
                capability.is_some(),
                IssueCode::Reference,
                &format!("/call/{target_field}"),
                "Select a registered capability for the invocation.",
            );

            // Scenarios may intentionally send schema-invalid arguments or assert a denial.
            let fixtures = scenario.fixtures.as_deref().unwrap_or_default();
            self.require(
                scenario.mode == ScenarioMode::Fixture || fixtures.is_empty(),
                IssueCode::Field,
                "/fixtures",
                "Use synthetic fixtures in fixture-mode scenarios.",
            );
            self.require(
                fixtures.len() <= MAX_RECORDS,
                IssueCode::Limit,
                "/fixtures",
                "Too many fixtures.",
            );
            let mut fixture_keys = BTreeSet::new();
            for (fixture_index, fixture) in fixtures.iter().take(MAX_RECORDS).enumerate() {
                self.at(
                    format!("{base}/fixtures/{fixture_index}"),
                    capability.map(Capability::id),
                );
                let key = self.fixture(fixture, bindings, capability.and_then(binding_id));
                self.require(
                    fixture_keys.insert(key),
                    IssueCode::DuplicateId,
                    "",
                    "This synthetic fixture target is already defined.",
                );
            }

            self.at(&base, capability.map(Capability::id));
            self.require(
                scenario.assertions.len() <= MAX_RECORDS,
                IssueCode::Limit,
                "/assertions",
                "Too many scenario assertions.",
            );
            for (assertion_index, assertion) in
                scenario.assertions.iter().take(MAX_RECORDS).enumerate()
            {
                self.at(
                    format!("{base}/assertions/{assertion_index}"),
                    capability.map(Capability::id),
                );
                match assertion {
                    Assertion::Equals { path, .. } => self.assertion_path(path),
                    Assertion::Schema { path, schema } => {
                        self.assertion_path(path);
                        self.schema(schema, false, "/schema");
                    }
                    Assertion::ErrorCode { code } => {
                        self.require(
                            code.len() <= 128 && code.contains('.') && code.split('.').all(is_key),
                            IssueCode::Field,
                            "/code",
                            "Use a dot-delimited diagnostic code.",
                        );
                    }
                }
            }
            if let Some(paths) = &scenario.sensitive_paths {
                self.at(&base, capability.map(Capability::id));
                self.require(
                    paths.len() <= MAX_RECORDS,
                    IssueCode::Limit,
                    "/sensitivePaths",
                    "Too many sensitive paths.",
                );
                for (path_index, path) in paths.iter().take(MAX_RECORDS).enumerate() {
                    self.require(
                        is_pointer(path),
                        IssueCode::Field,
                        &format!("/sensitivePaths/{path_index}"),
                        "Use a JSON Pointer for a sensitive path.",
                    );
                }
            }
        }
    }

    fn assertion_path(&mut self, path: &str) {
        self.require(
            is_pointer(path),
            IssueCode::Field,
            "/path",
            "Use a JSON Pointer over the normalized result.",
        );
    }

    fn fixture<'a>(
        &mut self,
        fixture: &'a Fixture,
        bindings: &BTreeMap<&str, &Binding>,
        expected_binding: Option<&str>,
    ) -> (&'a str, &'a str, String) {
        match fixture {
            Fixture::Rest {
                binding_id,
                method,
                path,
                status,
                ..
            } => {
                let binding = bindings.get(binding_id.as_str());
                self.require(
                    matches!(binding, Some(Binding::Rest(_)))
                        && expected_binding == Some(binding_id),
                    IssueCode::Reference,
                    "/bindingId",
                    "Select the invocation's REST binding.",
                );
                let parts = rest_template(path).filter(|parts| variables(parts).is_empty());
                self.require(
                    parts.is_some(),
                    IssueCode::Field,
                    "/path",
                    "Use a concrete HTTP path.",
                );
                self.require(
                    http_method(method).is_some(),
                    IssueCode::Field,
                    "/method",
                    "Choose a supported uppercase HTTP method.",
                );
                self.require(
                    (100..=599).contains(status),
                    IssueCode::Field,
                    "/status",
                    "Use an HTTP status from 100 to 599.",
                );
                if let Some(Binding::Rest(binding)) = binding {
                    self.require(
                        method == &binding.method,
                        IssueCode::Reference,
                        "/method",
                        "Use the REST binding's method.",
                    );
                    if let Some((parts, base)) =
                        parts.as_ref().zip(rest_template(&binding.path_template))
                    {
                        self.require(
                            path_is_within(parts, &base),
                            IssueCode::Reference,
                            "/path",
                            "Use a path within the REST binding's template.",
                        );
                    }
                }
                let path = parts.map_or_else(
                    || path.clone(),
                    |parts| {
                        parts
                            .into_iter()
                            .filter_map(|part| match part {
                                SegmentTemplate::Literal(value) => Some(value),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                            .join("/")
                    },
                );
                (binding_id, method, path)
            }
            Fixture::FileRead {
                binding_id,
                relative_path,
                text,
            } => {
                let binding = bindings.get(binding_id.as_str());
                self.require(
                    matches!(binding, Some(Binding::FileRead(_)))
                        && expected_binding == Some(binding_id),
                    IssueCode::Reference,
                    "/bindingId",
                    "Select the invocation's file-read binding.",
                );
                self.require(
                    is_portable_path(relative_path),
                    IssueCode::Field,
                    "/relativePath",
                    "Use a portable relative file path.",
                );
                self.text(text, MAX_OUTPUT_BYTES as usize, false, "/text");
                if let Some(Binding::FileRead(binding)) = binding {
                    let base = file_template(&binding.relative_path_template);
                    if let Some((parts, base)) = file_template(relative_path).zip(base) {
                        self.require(
                            variables(&parts).is_empty() && path_is_within(&parts, &base),
                            IssueCode::Reference,
                            "/relativePath",
                            "Use a concrete path within the file-read binding.",
                        );
                    }
                }
                (binding_id, "", portable_path_key(relative_path))
            }
        }
    }
}

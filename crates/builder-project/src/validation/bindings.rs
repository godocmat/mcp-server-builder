use std::collections::{BTreeMap, BTreeSet};

use http::header::HeaderName;
use ipnet::IpNet;

use super::{Check, MAX_OUTPUT_BYTES, MAX_RECORDS, binding_id};
use crate::syntax::{
    file_template, http_operation, http_origin, is_pointer, path_is_within, rest_template,
    variables,
};
use crate::{
    Binding, Capability, GrantOperation, GrantTarget, IssueCode, RequestedGrant, RestBinding,
};

impl Check {
    pub(super) fn bindings<'a>(
        &mut self,
        bindings: &'a [Binding],
        secrets: &BTreeSet<&str>,
    ) -> BTreeMap<&'a str, &'a Binding> {
        let mut ids = BTreeSet::new();
        let mut index = BTreeMap::new();
        for (position, binding) in bindings.iter().enumerate() {
            self.at(format!("/bindings/{position}"), None);
            self.unique_id(binding.id(), &mut ids);
            index.entry(binding.id()).or_insert(binding);
            match binding {
                Binding::Rest(binding) => self.rest_binding(binding, secrets),
                Binding::FileRead(binding) => {
                    self.key(&binding.root_ref, "/rootRef");
                    self.require(file_template(&binding.relative_path_template).is_some(),
                        IssueCode::Field, "/relativePathTemplate",
                        "Use a portable relative path with slash-separated segments and simple variables.");
                }
            }
        }
        index
    }

    fn rest_binding(&mut self, binding: &RestBinding, secrets: &BTreeSet<&str>) {
        self.require(
            http_origin(&binding.origin).is_some(),
            IssueCode::Field,
            "/origin",
            "Use an HTTP or HTTPS origin with a host and optional port.",
        );
        self.require(
            http_operation(&binding.method).is_some(),
            IssueCode::UnsupportedSetting,
            "/method",
            "Choose a supported uppercase HTTP method.",
        );
        let path = rest_template(&binding.path_template);
        self.require(
            path.is_some(),
            IssueCode::Field,
            "/pathTemplate",
            "Use an absolute HTTP path with simple segment variables.",
        );
        if let Some(path) = path {
            self.require(
                variables(&path)
                    .into_iter()
                    .eq(binding.path_params.keys().map(String::as_str)),
                IssueCode::Reference,
                "/pathParams",
                "Supply an input pointer for each path variable.",
            );
        }
        self.require(
            binding
                .path_params
                .values()
                .all(|pointer| is_pointer(pointer)),
            IssueCode::Field,
            "/pathParams",
            "Use JSON Pointers for HTTP path parameters.",
        );
        self.require(
            binding.body_pointer.as_deref().is_none_or(is_pointer),
            IssueCode::Field,
            "/bodyPointer",
            "Use a JSON Pointer for the HTTP body.",
        );

        if let Some(headers) = &binding.headers_from_secrets {
            self.require(
                headers.len() <= MAX_RECORDS,
                IssueCode::Limit,
                "/headersFromSecrets",
                "Too many credential header references.",
            );
            let mut names = BTreeSet::new();
            for (name, reference) in headers.iter().take(MAX_RECORDS) {
                let valid_name = name.len() <= 128
                    && name.parse::<HeaderName>().is_ok_and(|name| {
                        !matches!(
                            name.as_str(),
                            "host"
                                | "content-length"
                                | "transfer-encoding"
                                | "connection"
                                | "keep-alive"
                                | "upgrade"
                                | "te"
                                | "trailer"
                                | "proxy-authorization"
                                | "proxy-connection"
                                | "expect"
                        )
                    });
                self.require(
                    valid_name && names.insert(name.to_ascii_lowercase()),
                    IssueCode::Field,
                    "/headersFromSecrets",
                    "Use distinct, supported HTTP header names.",
                );
                self.require(
                    secrets.contains(reference.as_str()),
                    IssueCode::Reference,
                    "/headersFromSecrets",
                    "Select a declared secret reference for each header.",
                );
            }
        }
    }

    pub(super) fn grants(
        &mut self,
        grants: &[RequestedGrant],
        capabilities: &BTreeMap<&str, &Capability>,
        bindings: &BTreeMap<&str, &Binding>,
    ) {
        let mut ids = BTreeSet::new();
        for (index, grant) in grants.iter().enumerate() {
            let capability = capabilities.get(grant.capability_id.as_str()).copied();
            self.at(
                format!("/policy/grants/{index}"),
                capability.map(Capability::id),
            );
            self.unique_id(&grant.id, &mut ids);
            let binding = capability
                .and_then(binding_id)
                .and_then(|id| bindings.get(id).copied());
            self.require(
                binding.is_some(),
                IssueCode::Reference,
                "/capabilityId",
                "Select a capability with a binding for this requested grant.",
            );

            match &grant.target {
                GrantTarget::Rest {
                    origin,
                    method,
                    path_template,
                    allowed_peer_ranges,
                } => {
                    self.rest_grant(grant.operation, binding, origin, method, path_template);
                    if let Some(ranges) = allowed_peer_ranges {
                        self.require(
                            ranges.len() <= MAX_RECORDS
                                && ranges.iter().all(|range| {
                                    range.len() <= 64
                                        && range.parse::<IpNet>().is_ok_and(|range| {
                                            range == range.trunc()
                                                && range.addr().to_canonical() == range.addr()
                                        })
                                }),
                            IssueCode::Field,
                            "/target/allowedPeerRanges",
                            "Use canonical IPv4 or IPv6 CIDR ranges.",
                        );
                    }
                }
                GrantTarget::FileRead {
                    root_ref,
                    relative_path_template,
                } => {
                    self.file_grant(
                        grant.operation,
                        binding,
                        root_ref,
                        relative_path_template.as_deref(),
                    );
                }
            }
            if let Some(limits) = &grant.limits {
                self.timeout(limits.timeout_ms, "/limits/timeoutMs");
                self.require(
                    limits
                        .max_output_bytes
                        .is_none_or(|bytes| (1..=MAX_OUTPUT_BYTES).contains(&bytes)),
                    IssueCode::Limit,
                    "/limits/maxOutputBytes",
                    "Use an output limit from 1 to 1048576 bytes.",
                );
            }
        }
    }

    fn rest_grant(
        &mut self,
        operation: GrantOperation,
        binding: Option<&Binding>,
        origin: &str,
        method: &str,
        path: &str,
    ) {
        let origin = http_origin(origin);
        let parts = rest_template(path);
        self.require(
            origin.is_some(),
            IssueCode::Field,
            "/target/origin",
            "Use an HTTP or HTTPS origin with a host and optional port.",
        );
        self.require(
            http_operation(method) == Some(operation),
            IssueCode::Reference,
            "/operation",
            "Select the HTTP operation corresponding to the requested method.",
        );
        self.require(
            parts.is_some(),
            IssueCode::Field,
            "/target/pathTemplate",
            "Use an absolute HTTP path with simple segment variables.",
        );
        if let Some(Binding::Rest(binding)) = binding {
            self.require(
                origin.is_some()
                    && origin == http_origin(&binding.origin)
                    && method == binding.method,
                IssueCode::Reference,
                "/target",
                "Use the REST binding's origin and method.",
            );
            if let Some((parts, base)) = parts.zip(rest_template(&binding.path_template)) {
                self.require(
                    path_is_within(&parts, &base),
                    IssueCode::Reference,
                    "/target/pathTemplate",
                    "Use the binding's path template or a narrower path.",
                );
            }
        } else {
            self.require(
                false,
                IssueCode::Reference,
                "/target",
                "Select a REST target for a REST binding.",
            );
        }
    }

    fn file_grant(
        &mut self,
        operation: GrantOperation,
        binding: Option<&Binding>,
        root_ref: &str,
        path: Option<&str>,
    ) {
        self.require(
            operation == GrantOperation::FileRead,
            IssueCode::Reference,
            "/operation",
            "Use file.read for a file-read target.",
        );
        self.key(root_ref, "/target/rootRef");
        let parts = path.and_then(file_template);
        self.require(
            path.is_none() || parts.is_some(),
            IssueCode::Field,
            "/target/relativePathTemplate",
            "Use a portable relative path with simple segment variables.",
        );
        if let Some(Binding::FileRead(binding)) = binding {
            self.require(
                root_ref == binding.root_ref,
                IssueCode::Reference,
                "/target/rootRef",
                "Use the file-read binding's root reference.",
            );
            if let Some((parts, base)) = parts.zip(file_template(&binding.relative_path_template)) {
                self.require(
                    path_is_within(&parts, &base),
                    IssueCode::Reference,
                    "/target/relativePathTemplate",
                    "Use the binding's path template or a narrower path.",
                );
            }
        } else {
            self.require(
                false,
                IssueCode::Reference,
                "/target",
                "Select a file-read target for a file-read binding.",
            );
        }
    }
}

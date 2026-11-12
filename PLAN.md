# MCP Server Builder development plan

12 November 2026

MCP Server Builder will let developers create a server visually, exercise it through a real MCP client, and export a maintainable Rust project that builds and runs independently. This roadmap starts with design and gives each implementation phase an observable exit criterion.

Use the [task breakdown](TASKS.md) to dispatch work in fresh Codex chats and worktrees.

## Release scope

| Milestone | Scope | Acceptance workflow |
| --- | --- | --- |
| **Local MVP** | Windows desktop pilot and a shared Rust CLI. Create, open, validate, and export a versioned project. Configure a REST tool, a read-only file resource and resource template, and a prompt. Run the project over stdio, inspect a trace, and replay a saved fixture test. | On a clean Windows machine, create the sample project in the GUI, run its tests from the CLI, then compile and invoke its exported server from a separate directory using a generic MCP client. Verify that invalid input and a denied permission produce actionable errors. |
| **Commercial v1** | Complete GUI and CLI project workflows, including rename, duplicate, import, migration, and client configuration export. Manage server metadata, transports, dependencies, and environment variables. Add PostgreSQL, SQLite, Git, custom Rust, and policy-bound external command templates. Support stdio and authenticated Streamable HTTP, full playground and audit workflows, Windows/macOS/Linux packages on verified targets, and Docker and self-hosted Linux deployment. | A new user can install the builder, configure and test a server, export and build it, deploy it to clean Linux infrastructure, and connect with the supported client configurations. CI and the release candidate supply evidence for the workflow. |

Proposed v1 scope leaves legacy HTTP+SSE, optional extensions such as Tasks and MCP Apps, cloud-specific adapters, hosted builder infrastructure, and centralized team administration for later evaluation. Payment processing requires separate authorization and credentials. PostgreSQL as a **tool integration** is in v1. PostgreSQL as **storage for a hosted builder** is optional.

## Protocol and client baseline

- Use **MCP 2026-07-28** as the primary target. Implement stateless requests, per-request metadata, `server/discover`, and Streamable HTTP header validation. Use the cancellation behavior defined for each transport. Treat **2025-11-25** as the proposed compatibility floor, with `initialize`, capability negotiation, and legacy HTTP session behavior where exercised by a supported client. Record supported capabilities by revision and transport. [Current MCP specification](https://modelcontextprotocol.io/specification/2026-07-28), [Versioning and compatibility](https://modelcontextprotocol.io/specification/2026-07-28/basic/versioning), [Streamable HTTP binding](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/streamable-http), [stdio binding](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/stdio)
- Evaluate **`rmcp` 3.9.0** as the initial Rust SDK candidate. Pin the tested version after verifying the supported methods, schema handling, cancellation, and both protocol revisions over real stdio and loopback HTTP. Keep SDK-specific types at the transport boundary. [Official Rust SDK release](https://github.com/modelcontextprotocol/rust-sdk/releases/tag/rmcp-v3.9.0)
- Use the HTTP server as an OAuth resource server backed by an external authorization server. Select a self-hostable reference provider in discovery. Verify protected resource metadata, the authorization challenge, token validation, and scope enforcement. The playground’s client flow must also prove discovery, authorization, and token refresh against the reference provider. [MCP authorization specification](https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization)

| Client | Documented starting point | Interoperability work |
| --- | --- | --- |
| Claude Code | Local stdio and remote HTTP with OAuth. Its MCP client runtime affects revision support. [Claude Code MCP docs](https://code.claude.com/docs/en/mcp) | Verify the generated configuration for each transport and the revision selected by the tested runtime. |
| Codex | Local stdio and Streamable HTTP with OAuth and multiple registration methods. [Official Codex MCP docs](https://learn.chatgpt.com/docs/extend/mcp) | Verify generated configuration, login, discovery, and one invocation on each transport. Record the Codex build, negotiated revision, and tested registration flow. |
| OpenCode | Local stdio, remote Streamable HTTP and OAuth. Its docs list a legacy default and explicit modern revision settings. [OpenCode MCP docs](https://opencode.ai/docs/mcp-servers) | Verify configuration export for the tested release, including revision selection and OAuth. |
| Generic MCP | The official Inspector provides independent testing for local and remote connections. [MCP Inspector docs](https://modelcontextprotocol.io/docs/2026-07-28/tools/inspector) | Use Inspector and an independent protocol test client to verify discovery and calls. Publish command, URL, and authentication requirements for other clients. |

For each supported combination, the release matrix records the client version, OS, transport, negotiated revision, authentication mode, and capabilities exercised. Test tool invocation and resource and prompt operations where the client exposes them. List unsupported optional protocol features and any combinations that could only be documented because a client feature or account was unavailable.

## Market discovery

| Product | Documented workflow |
| --- | --- |
| MCP Inspector | Connects to servers, inspects capabilities and traffic, and exercises tools, resources, and prompts. Use it as the baseline for the playground. [MCP Inspector docs](https://modelcontextprotocol.io/docs/2026-07-28/tools/inspector) |
| FastMCP | Provides a Python framework for authoring MCP servers and clients, with schema generation, transport, and authentication support. [FastMCP docs](https://gofastmcp.com/docs/getting-started/welcome) |
| Speakeasy | Generates standalone MCP servers from OpenAPI specifications and supports operation scoping and deployment options. [Standalone MCP generation docs](https://www.speakeasy.com/docs/standalone-mcp/overview) |
| Claude Code MCP development plugin | Scaffolds a remote HTTP or local stdio server from a described use case. [Claude Code MCP docs](https://code.claude.com/docs/en/mcp#find-and-build-mcp-servers) |

**Positioning hypothesis.** Developers and internal platform teams will value one workflow for configuring non-OpenAPI integrations, reviewing permissions, replaying tests, and shipping independently editable Rust. In discovery, time representative users from a clean install to the first successful tool call. Compare project portability, policy visibility, test replay, and deployment effort across the documented alternatives. Record observed limitations separately from claims that still need evidence.

**Pricing hypotheses.** Free Developer covers local authoring, testing, and source export. Professional adds advanced automation or template packs. Team and Enterprise may pay for centralized policy, audit retention, and support. Validate willingness to pay with target users and protect access to existing projects, exported servers, and security controls during entitlement outages.

## Architecture to validate

Use a Cargo workspace with shared Rust libraries. The Clap CLI and Tauri 2 desktop shell call the same application services. Angular uses strict TypeScript and Signals over typed, narrowly scoped Tauri IPC. Generated servers use Tokio, Serde, and tracing, with Axum for HTTP. Pin the Rust, Node, Angular, and Tauri versions after checking supported OS and WebView targets.

| Component | Owns | Lead |
| --- | --- | --- |
| Project core and storage | Project schema, validation, migrations, import/export, and project operations. A versioned JSON manifest is portable project data. SQLite via SQLx holds the local project index, run history, and audit metadata. Use atomic manifest saves, conflict detection, transactional migrations, and recovery tests. | Rust and architecture |
| Runtime and policy | Capability registry, adapter dispatch, schema validation, authorization decisions, execution limits, cancellation, credential resolution, and audit events. Apply the same policy rules to preview and generated execution. | Rust, MCP, and security |
| Generator | Versioned templates and deterministic emission. Bundle a versioned, redistributable runtime crate with generated source, subject to dependency license review. Include locked dependencies, real handlers and tests, and a regeneration workflow that preserves user edits. | Rust and architecture |
| Playground | MCP client, supervised local processes, connection profiles, scenario replay, fixtures, sanitized wire messages and timing, and report export. Use a separate connection record for client imports whose handler source is unavailable. | Rust and QA |
| Desktop and CLI | Guided project editor, policy and trust review, testing and deployment workflows, and client configuration export. CLI contracts cover human-readable output, versioned JSON output, diagnostics, and exit codes for CI. | Angular, Rust, and QA |
| Release and licensing | Platform installers, containers, deployment packaging, and a builder-side entitlement boundary. Record licensing terms for the builder and redistribution of generated code and bundled dependencies. | Platform and product |

Data entities for the design include project, capability, integration binding, policy, secret reference, connection profile, test scenario, run result, audit event, and generation record. Keep schema, generator, and protocol versioning independent. Store secret references in projects and resolve values through the OS credential store or deployment-provided secrets.

## Phase 0 design deliverables

Finalize these artifacts before scaffolding. Carry unresolved implementation questions into the earliest phase that can test them.

- **Product and QA** deliver the PRD, competitive analysis, functional and non-functional requirements, and a traceability matrix linking each requirement to its release milestone and acceptance test. Establish a reference workload and budgets for latency, resource use, startup, and shutdown.
- **Architecture and security** deliver the system architecture, component diagrams, data model, threat model, security architecture, and ADRs for protocol support, project storage, runtime distribution, trust boundaries, and authorization.
- **Desktop and CLI** deliver wireframes, CLI command specifications, and API contracts covering IPC, project schemas, structured errors, and report formats. Wireframes cover import, capability editing, permission review, playground, generation, and recovery states.
- **MCP, QA, and platform** deliver the compatibility matrix, testing strategy, deployment architecture, and release plan. Capture client versions, OS targets, reference authorization provider, and which checks run automatically.
- **Product and engineering** finalize the prioritized backlog and risk register. Track implementation tasks in `TASKS.md`, progress and evidence in `PROGRESS.md`, and decisions in `DECISIONS.md` and individual ADRs.

## Implementation order and exit criteria

| Phase | Deliverable | Exit criterion |
| --- | --- | --- |
| **0. Discovery** | Complete the design package. Review SDK and client releases, OAuth provider options, supported import formats, packaging constraints, and competitive workflows. | Every v1 requirement has an owner and acceptance test. Compatibility and packaging assumptions have scheduled checks. |
| **1. Foundation** | Establish the Cargo workspace, minimal Tauri and Angular shell, CLI entry point, versioned configuration, shared errors and logging, toolchain locks, and GitHub Actions. | A clean checkout builds and runs lint, formatting, unit, and platform smoke checks in the Windows, macOS, and Linux CI matrix. |
| **2. Core MCP engine** | Implement project validation, capability registration, discovery and invocation, input and applicable output schema validation, structured results and errors, applicable notifications, timeouts, and cancellation. Introduce deny-by-default adapter policies before connecting integrations. | A real client lists and exercises tools, resources, resource templates, and prompts over stdio and loopback HTTP. The protocol suite covers both target revisions, unsupported versions, and any legacy session behavior enabled for compatibility. |
| **3. Server generator** | Add the REST and read-only filesystem templates with explicit host and path grants. Emit standalone Cargo source, lockfile, configuration, documentation, real tests, and deployment inputs. Record template and runtime versions in exports. | Repeated generation from the same inputs yields identical files. A clean checkout outside the builder builds, tests, and serves real MCP responses. Regeneration preserves user edits. |
| **4. Testing and debugging** | Add local launch approval and process supervision, JSON inputs, expected-output assertions, timing and error inspection, bounded redacted wire traces, fixture replay, and reusable scenarios. Add the initial desktop and CLI workflow for the local MVP. | Demonstrate the local MVP acceptance workflow. Saved scenarios reproduce pass and fail results, malformed-input tests run against a controlled fixture, and cancellation or shutdown cleans up the launched server when required. |
| **5. Security and integrations** | Extend the MVP controls with remote OAuth, request and rate limits, OS credential storage, reusable permission approvals, and audit retention. Add remote playground connections and deliver SQLite, PostgreSQL, Git, custom Rust, and approved executable templates once their policies are testable. | Real OAuth and adapter integration tests show allowed operations succeed and denied operations leave protected state intact. Logs, reports, and exports pass secret redaction checks. |
| **6. Desktop GUI** | Complete project management, guided capability editors, schema and policy diagnostics, connection and OAuth flows, playground, and report and project export. Cover loading, empty, error, and recovery states. | A GUI test or documented native smoke run covers import, edit, test, export, and recovery on each supported desktop target. Keyboard navigation and accessibility checks pass on the core flows. |
| **7. CLI and automation** | Expose project operations, validation, generation, execution, scenario replay, report and client config export, and policy review through Clap. | CI runs the full headless workflow. JSON output and exit codes distinguish invalid configuration, policy denial, test failure, and process failure. GUI and CLI agree on project validation and policy decisions. |
| **8. Deployment and packaging** | Produce native installers, a generated Dockerfile and Compose configuration, self-hosted service instructions, health checks, graceful shutdown, structured logs, and backup/restore procedures. | A clean Linux host runs the generated deployment with its reference OAuth provider. An operator can verify readiness, upgrade, roll back, and restore state from a backup. Desktop install and launch pass on every advertised target. |
| **9. Hardening** | Run the release matrix, performance and accessibility checks, dependency and container scans, failure injection, and recovery tests. | The release candidate meets the release gates below and has no unresolved release-blocking defects. |
| **10. Production release** | Produce versioned installers and images, SBOMs, checksums, release notes, documentation, and operational runbooks. Sign the artifacts for which credentials are available and record their signing status. | Verify downloads, checksums and signatures where applicable, installation, and upgrade from published artifacts. Record the release evidence. |

The MVP depends on the end-to-end path through phases 1–4. Desktop and CLI work can expand in parallel once project and policy contracts stabilize. Authenticated HTTP and the additional adapters must pass phase 5 before inclusion in a distributable beta.

## Security acceptance criteria

- **Import, preview, and launch.** Validate imported manifests, archives, and template parameters before writing or acting on them. Show the resolved executable, working directory, environment references, and requested permissions at the trust decision. Passive import must leave imported code inert. Treat custom Rust handlers and their build dependencies as trusted code that requires explicit approval before build or execution. Document the enforced boundary per platform and reserve sandbox claims for tested OS-level isolation.
- **Adapter permissions.** Enforce approved roots and file modes, including symlink and platform path handling. Check network destinations at connection time and reapply policy on redirects. Run database templates against configured parameterized statements and least-privilege roles. Default Git to read-only operations. External commands require an approved executable, validated arguments, a restricted environment and working directory, a timeout, and tested process-tree cleanup. Use shell-free execution.
- **Local stdio and remote HTTP.** Use local OS identity and explicit launch trust for stdio. For remote HTTP, require authenticated HTTPS, validate issuer, resource or audience, expiry, and scope, and enforce permission by capability and caller. Verify Origin handling, request and rate limits, and graceful shutdown through the documented ingress configuration.
- **Secrets and evidence.** Keep secrets in the OS credential store locally and deployment-managed secret injection remotely. If the local credential store is unavailable, offer a session-only or environment-provided secret. Filter or redact known secrets and sensitive fields before display, persistence, and export. Limit trace retention and include principal, operation, decision, outcome, and correlation ID in audit events.

## Release evidence

- **Correctness.** Run Rust and Angular unit tests, CLI and desktop workflows, independent MCP conformance tests over real transports, generated-project compile and run tests, security regressions, and platform smoke checks. Fixtures can simulate upstream integrations while the MCP exchanges use real stdio or HTTP.
- **Quality.** Meet initial line coverage targets of **80%** for core business logic and **90%** for security-critical modules, with documented exclusions. Exercise project migration, concurrent saves, malformed inputs, canceled runs, missing credentials, network loss, and restore from backup. Compare performance against the Phase 0 budgets.
- **Supply chain.** Use pinned toolchains and dependencies, clean build environments, and repeatable build instructions. Compare clean builds for reproducibility and document platform-specific differences. Pass secret, dependency, and container scans. Resolve critical and high findings or record acceptance by the authorized owner.
- **Documentation.** Publish the README, architecture, requirements, security, development, testing, deployment, troubleshooting, compatibility, and commercial model guides, plus the changelog, contribution guide, and license or licensing decision record. Validate installation and operations from the published instructions on clean machines.

## Initial risk register

| Risk | Treatment | Owner |
| --- | --- | --- |
| SDK behavior differs across protocol revisions or client releases. | Pin a tested SDK, maintain independent wire fixtures, and rerun the compatibility matrix before updating the pin or advertising a new client version. | MCP and QA |
| OAuth discovery and registration vary by client and provider. | Pick the reference provider in discovery and exercise each supported login path early. Advertise only combinations supported by evidence. | Security and MCP |
| Process cleanup, credential stores, and native GUI automation vary by OS. | Test the smallest desktop and CLI flow on each OS during foundation. Specify native smoke checks where automation coverage is limited. | Desktop and platform |
| Imported projects or templates can grant unintended access. | Keep import passive, require explicit grants before use, and verify path, egress, database, and executable restrictions before each adapter ships. | Security and Rust |
| A generated server can retain a vulnerable runtime after export. | Embed runtime and template versions, publish security fixes and upgrade guidance, and verify that upgrades preserve project-owned code. | Rust and platform |
| Signing, client accounts, or redistribution terms can delay release. | Identify dependencies in discovery, validate artifacts with available credentials, and record any affected release target or unverified client combination. | Product and platform |

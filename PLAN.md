# MCP Server Builder development plan

12 November 2026

MCP Server Builder lets developers create a local MCP server, exercise it through a real MCP client, and export a Rust project that builds and runs independently. The Windows x64 release, CLI, and first-party export code are free and open source under Apache-2.0. They work without product accounts or entitlements. [Current release](README.md)

Use the [task breakdown](TASKS.md) to dispatch work in fresh Codex chats and worktrees. The [Phase 0 handoff](PROGRESS.md), [decisions](DECISIONS.md) and [acceptance traceability](docs/traceability.md) define the starting design and evidence gates.

## Release scope

The local milestone describes the current release. The v1 proposal and later phases below preserve the original roadmap and its acceptance criteria. Those features are outside the current release.

| Milestone | Scope | Acceptance workflow |
| --- | --- | --- |
| **Local MVP** | Windows desktop pilot and a shared Rust CLI. Create, open, validate, and export a versioned project. Configure a REST tool, a read-only file resource and resource template, and a prompt. Run the project over stdio, inspect a trace, and replay a saved fixture test. | On a clean Windows machine, create the sample project in the GUI, run its tests from the CLI, then compile and invoke its exported server from a separate directory using a generic MCP client. Verify that invalid input and a denied permission produce actionable errors. |
| **Historical v1 proposal** | Complete GUI and CLI project workflows, including rename, duplicate, import, migration, and client configuration export. Manage server metadata, transports, dependencies, and environment variables. Add PostgreSQL, SQLite, Git, custom Rust, and policy-bound external command templates. Support stdio and authenticated Streamable HTTP, full playground and audit workflows, Windows/macOS/Linux packages on verified targets, and Docker and self-hosted Linux deployment. | A new user can install the builder, configure and test a server, export and build it, deploy it to clean Linux infrastructure, and connect with the supported client configurations. CI and the release candidate supply evidence for the workflow. |

The historical v1 scope proposed a fixed catalog, a scoped client matrix, and a PostgreSQL tool integration. Later candidates included standalone GET/resumption for legacy Streamable HTTP, the separate legacy HTTP+SSE transport, change subscriptions, optional extensions such as Tasks and MCP Apps, cloud-specific deployment adapters, hosted infrastructure, and centralized team administration. [Protocol and licensing decisions](DECISIONS.md)

## Protocol and client baseline

- Use **MCP 2026-07-28** as the primary target and **2025-11-25** as the compatibility floor. Use modern per-request metadata, `server/discover` and HTTP header validation, plus legacy initialization, capability negotiation and issued Streamable HTTP sessions. The local release ships stdio. M4 verifies loopback HTTP for both revisions. The historical v1 design proposed authenticated Streamable HTTP. [Transport contract](docs/contracts.md#mcp-transport-and-execution) · [D1 compatibility evidence](docs/mcp-compatibility.md)
- Evaluate **`rmcp` 3.9.0** as the initial Rust SDK candidate. Pin the tested version after verifying the supported methods, schema handling, cancellation, and both protocol revisions over real stdio and loopback HTTP. Keep SDK-specific types at the transport boundary. [Official Rust SDK release](https://github.com/modelcontextprotocol/rust-sdk/releases/tag/rmcp-v3.9.0)
- Use the HTTP server as an OAuth resource server backed by an external authorization server. Trial Logto OSS with a pre-registered public PKCE client first and Keycloak if the required flows fail. V3/V4 verify discovery, the authorization challenge, token and scope checks, resource isolation and refresh. V10 and R4 verify registration flows in the actual clients. [OAuth and client decision](DECISIONS.md#adr-007--external-oauth-and-client-profiles)

| Client | Documented starting point | Interoperability work |
| --- | --- | --- |
| Claude Code 2.1.322 | `.mcp.json` for stdio and HTTP, and the selected OAuth registration. | Exercise the candidate runtime profiles and record actual revisions and callbacks. |
| Codex CLI 0.167.2 | `config.toml` for stdio and HTTP and CLI login. | Capture the first exchange and verify the actual revision and registration flow. |
| OpenCode V2 2.2.7 | V2 local and remote configuration. | Exercise default, auto and explicit modern profiles and verify OAuth. |
| Generic MCP · Inspector 2.15.0 | Generic command or URL, scopes and authentication instructions. | Exercise both eras and transports with Inspector and the independent wire harness. |

For each supported combination, the release matrix records the client version, OS, transport, negotiated revision, authentication mode, and capabilities exercised. Test tool invocation and resource and prompt operations where the client exposes them. Record evidence gaps for named combinations and publish verified support. [D1 client matrix](docs/mcp-compatibility.md#client-matrix)

## Market discovery

| Product | Documented workflow |
| --- | --- |
| MCP Inspector | Connects to servers, inspects capabilities and traffic, and exercises tools, resources, and prompts. Use it as the baseline for the playground. [MCP Inspector docs](https://modelcontextprotocol.io/docs/2026-07-28/tools/inspector) |
| FastMCP | Provides a Python framework for authoring MCP servers and clients, with schema generation, transport, and authentication support. [FastMCP docs](https://gofastmcp.com/docs/getting-started/welcome) |
| Speakeasy | Generates standalone MCP servers from OpenAPI specifications and supports operation scoping and deployment options. [Standalone MCP generation docs](https://www.speakeasy.com/docs/standalone-mcp/overview) |
| Claude Code MCP development plugin | Scaffolds a remote HTTP or local stdio server from a described use case. [Claude Code MCP docs](https://code.claude.com/docs/en/mcp#find-and-build-mcp-servers) |

**Positioning hypothesis.** Developers and internal platform teams will value one workflow for configuring non-OpenAPI integrations, reviewing permissions, replaying tests, and shipping independently editable Rust. In discovery, time representative users from a clean install to the first successful tool call. Compare project portability, policy visibility, test replay, and deployment effort across the documented alternatives. Record observed limitations separately from claims that still need evidence.

**Historical pricing hypotheses.** Early research considered optional automation, template packs, team policy, audit retention, and support. The current release offers the full shipped feature set free of charge. [Discovery notes](docs/commercial-model.md#packaging-and-pricing-hypotheses)

## Architecture to validate

The Cargo workspace shares application services across Clap and Tauri, and shares runtime and policy crates across managed preview and standalone Rust exports. Angular uses strict TypeScript and Signals over typed native IPC. The project manifest is portable and SQLx SQLite holds local state. F1 and F2 verify compiler, dependency and WebView candidates. The Windows pilot uses per-user NSIS. The v1 candidates add an Apple silicon DMG, Ubuntu amd64 DEB, and Ubuntu Server amd64 with native systemd and Compose profiles. [Components, data flow and deployment](docs/architecture.md) · [Shared contracts](docs/contracts.md) · [Platform decision](DECISIONS.md#adr-008--toolchain-platforms-and-packaging)

## Phase 0 design deliverables

The [Phase 0 handoff](PROGRESS.md#pre-implementation-design-checklist) maps all 18 items in the design brief to their artifacts. The coordinator reviews and integrates D5 before F1 starts. The [traceability matrix](docs/traceability.md) owns requirement and quality gates. The [prioritized dependencies](TASKS.md#prioritized-dependencies) carry unresolved evidence to the first task that can produce it.

## Implementation order and exit criteria

| Phase | Deliverable | Exit criterion |
| --- | --- | --- |
| **0. Discovery** | Consolidate the 18-item design package and choose implementation starting points. | Review D5, confirm the scope, owners and evidence gates, and integrate the handoff baseline for F1. |
| **1. Foundation** | Establish the Cargo workspace, minimal Tauri and Angular shell, CLI entry point, versioned configuration, shared errors and logging, toolchain locks, and GitHub Actions. | A clean checkout builds and runs lint, formatting, unit, and platform smoke checks in the Windows, macOS, and Linux CI matrix. |
| **2. Core MCP engine** | Implement project validation, capability registration, discovery and invocation, input and applicable output schema validation, structured results and errors, applicable notifications, timeouts, and cancellation. Introduce deny-by-default adapter policies before connecting integrations. | A real client lists and exercises tools, resources, resource templates, and prompts over stdio and loopback HTTP. The protocol suite covers both target revisions, unsupported versions, and any legacy session behavior enabled for compatibility. |
| **3. Server generator** | Add the REST and read-only filesystem templates with explicit host and path grants. Emit standalone Cargo source, lockfile, configuration, documentation, real tests, and deployment inputs. Record template and runtime versions in exports. | Repeated generation from the same inputs yields identical files. A clean checkout outside the builder builds, tests, and serves real MCP responses. Regeneration preserves user edits. |
| **4. Testing and debugging** | Add local launch approval and process supervision, JSON inputs, expected-output assertions, timing and error inspection, bounded redacted wire traces, fixture replay, and reusable scenarios. Add the initial desktop and CLI workflow for the local MVP. | Demonstrate the local MVP acceptance workflow. Saved scenarios reproduce pass and fail results, malformed-input tests run against a controlled fixture, and cancellation or shutdown cleans up the launched server when required. |
| **5. Security and integrations** | Extend the MVP controls with remote OAuth, request and rate limits, OS credential storage, reusable permission approvals, and audit retention. Add remote playground connections and deliver SQLite, PostgreSQL, Git, custom Rust, and approved executable templates once their policies are testable. | Real OAuth and adapter integration tests show allowed operations succeed and denied operations leave protected state intact. Logs, reports, and exports pass secret redaction checks. |
| **6. Desktop GUI** | Complete project management, guided capability editors, schema and policy diagnostics, connection and OAuth flows, playground, and report and project export. Cover loading, empty, error, and recovery states. | A GUI test or documented native smoke run covers import, edit, test, export, and recovery on each supported desktop target. Keyboard navigation and accessibility checks pass on the core flows. |
| **7. CLI and automation** | Expose project operations, validation, generation, execution, scenario replay, report and client config export, and policy review through Clap. | CI runs the full headless workflow. JSON output and exit codes distinguish invalid configuration, policy denial, test failure, and process failure. GUI and CLI agree on project validation and policy decisions. |
| **8. Deployment and packaging** | Produce native installers, a generated Dockerfile and Compose configuration, self-hosted service instructions, health checks, graceful shutdown, structured logs, and backup/restore procedures. | A clean Linux host runs the generated deployment with its reference OAuth provider. An operator can verify readiness, upgrade, roll back, and restore state from a backup. Desktop install and launch pass on every advertised target. |
| **9. Hardening** | Run the release matrix, performance and accessibility checks, dependency and container scans, failure injection, and recovery tests. | The release candidate meets the release gates below and has no unresolved release-blocking defects. |
| **10. Production release** | Produce versioned installers and images, SBOMs, checksums, release notes, documentation, and operational runbooks. Apply and verify the selected signing profile where credentials are available. | Verify integrity, installation and upgrade from final artifacts. Repeat integrity and install checks from published artifacts after authorized release. Record signing and support status by target. |

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

## Risk register

Risks are open pending their first verification. High risks can block a milestone or invalidate a security or support claim. Critical risks affect access to protected resources or effects. Owners update the relevant gate record as evidence arrives. [Threat model and verification cases](docs/security.md#trust-boundaries-and-threats)

| ID and priority | Risk and response | Accountable owner and verification |
| --- | --- | --- |
| RISK-01 · High | SDK or client drift breaks a revision, routing metadata, cancellation or session lifecycle. Use the pinned candidate and independent wire tests before selecting the SDK and advertising clients. | MCP · M3/M4, V10/R4 |
| RISK-02 · Critical | Wrong-resource, wrong-issuer or ID tokens, or foreign legacy sessions, reach a protected adapter. Verify two resources, caller binding, Origin and proxy trust with real tokens and zero dispatch on denial. | Resource-server security · V3, R1/R3 |
| RISK-03 · High | Provider registration or refresh cannot satisfy a required client. Test public pre-registration, then conditional CIMD or restricted DCR, with Keycloak fallback. Resolve blocked combinations before making release claims. | OAuth client lead · V4, V10/R4 |
| RISK-04 · High | Import, build or launch acts with unreviewed privileges, or a target escapes adapter scope. Keep import inert, bind approvals to identity and content, and check the concrete target. | Runtime policy · M2/M5/M7, V1/V5–V9/R3 |
| RISK-05 · High | Credentials leak through a binding, native store fallback, stderr, traces or exports. Verify scoped native lookups and inject canaries across allowed and failed paths. | Credentials lead · V2, R1/R3 |
| RISK-06 · High | Save, migration or regeneration interrupts or loses user work. Compare revisions, stage writes, retain known-good inputs and exercise recoverable failure. | Project core and generator · M1/M6/V1, R3 |
| RISK-07 · High | Native dependencies, WebView, file semantics, TLS or supervision differ by target. Verify locked toolchains, real GUI/install, CA trust and tested process and file boundaries on named systems. | Platform · F1/F2, M5/M7/M8, V2/V8/R2/R4 |
| RISK-08 · High | A native package cannot be signed, notarized, installed or upgraded as claimed. Verify available signatures and test installation and upgrade on clean hosts. Record missing signing or test inputs and the affected targets. | Release platform · R2/R5 |
| RISK-09 · High | Export license terms or a vulnerable bundled runtime block distribution or safe upgrades. Inventory components, review terms and notices, track runtime versions and prove a user-edit-preserving update. | Product and generator · M6/V9/R2/R5 |
| RISK-10 · High | Service or provider upgrade loses state or exceeds recovery targets. Use consistent backups, compatible rollback or clean restore, fresh authorization and data-marker checks. | Deployment platform · R1/R3/R5 |
| RISK-11 · Medium | Credentialed test accounts or native/benchmark capacity arrive late. Reserve disposable accounts and machines early and record blocked rows with their gate owner. | QA · M10, V10/R4 |
| RISK-12 · Medium | Latency, coverage or accessibility evidence misses its target. Track coverage paths as features land and run the same fixture and installed-app smoke at pilot and release. | QA · F2, M8/M10, R3/R4 |

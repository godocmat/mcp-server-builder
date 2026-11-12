# MCP Server Builder task breakdown

[PLAN.md](PLAN.md) defines the product scope and release gates. This backlog breaks it into assignments with one owner, an explicit starting point, and a result the coordinator can verify. [Traceability](docs/traceability.md) records the accountable role, task and evidence for each requirement. The coordinator assigns a person or worktree owner when dispatching each task.

## Running tasks in Codex

1. **Create a shared baseline.** The repository had no commits when this breakdown was prepared. Commit the planning files before starting the first worktree chats. Start each assignment from the integration branch after its listed dependencies have merged.
2. **Start a new Codex chat in Worktree mode for each assignment.** This gives each assignment a fresh session and checkout. Built-in subagents are delegated from a parent chat. If you use them, explicitly provision each writing agent's worktree and confirm its path before edits. [Codex worktrees](https://learn.chatgpt.com/docs/environments/git-worktrees), [Codex subagents](https://learn.chatgpt.com/docs/agent-configuration/subagents)
3. **Keep one coordinator for integration.** Give each task a unique branch before handoff, since a Codex-managed worktree can start detached. The coordinator reviews and integrates finished changes, reruns checks affected by the merge, and updates `PROGRESS.md`. Coordinate changes to shared contracts, workspace manifests, lockfiles, migrations, and CI workflows with their current owner. Use separate ports and test data for concurrent integration tests. [Codex worktrees](https://learn.chatgpt.com/docs/environments/git-worktrees)
4. **Give each chat a reviewable outcome.** Include the task ID, starting commit, scope, expected deliverable, files or components it owns, and checks to run. Have it return the branch or commit, check results, and any unresolved dependencies. Split work when it crosses ownership boundaries or one deliverable must land before another can be verified. Group related small changes that share the same acceptance check.

Sizes are relative. **Small** covers one bounded artifact or adapter. **Medium** covers one coherent workflow. **Large** covers a shared contract or a workflow that needs cross-component verification. Refine large tasks after their dependencies land if they still span multiple independently verifiable outcomes.

## Discovery and foundation

Start **D1, D2, and D3** together as planning-only assignments. **D4** is also independent and can start when another worktree chat is available. **D5** integrates their results and completes Phase 0 before implementation starts.

| Task | Size | Requires | Deliverable and acceptance |
| --- | --- | --- | --- |
| **D1 · MCP and clients** | Medium | Baseline commit | Own `docs/mcp-compatibility.md`. Document the protocol and SDK candidates, transport lifecycles, OAuth support, and client matrix. Link primary sources and identify the cases that need real wire tests. |
| **D2 · Product and UX** | Medium | Baseline commit | Own `docs/requirements.md` and `docs/commercial-model.md`. Draft the PRD, functional requirements, market comparison, pricing hypotheses, and workflow wireframes. Map each major workflow to an acceptance criterion. |
| **D3 · Security design** | Medium | Baseline commit | Own `docs/security.md`. Produce the threat model, trust boundaries, policy and credential design, and reference OAuth provider shortlist. Map each boundary to a verification case and identify platform-specific limits. |
| **D4 · Platform and QA design** | Medium | Baseline commit | Own `docs/deployment.md` and `docs/testing.md`. Document OS and packaging targets, CI, deployment, the test strategy, quality budgets, and signing dependencies. Give each advertised target a verification path. |
| **D5 · Shared design and contracts** | Large | D1–D4 | Deliver the [architecture](docs/architecture.md), [contracts](docs/contracts.md), [traceability](docs/traceability.md), [ADRs](DECISIONS.md), risk register and [Phase 0 handoff](PROGRESS.md). Verify the 18-item design checklist and assign each MVP, v1 and quality requirement an owner, acceptance check and gate. |
| **F1 · Workspace foundation** | Large | Reviewed and integrated D5 | Own the initial Cargo, Angular, Tauri and Clap scaffold, local toolchain trial and locks, shared DTOs, errors and tracing. Use the D5 crate boundaries. Prove a clean local build and minimal desktop and CLI launch. Record exact toolchain and candidate SDK features for F2 and M3/M4. [F1 entry conditions](PROGRESS.md#f1-entry-conditions) |
| **F2 · Build and test CI** | Medium | F1 | Prove build, lint, format, unit and smoke checks on the named Windows, macOS and Linux runners. Verify DTO parity and dependency/engine trials. Set up coverage reports and add feature gates as code lands. Trial native GUI automation before relying on it for release. |

## Local MVP

After F1, the project model, policy engine, and CI can proceed together on the agreed interfaces. After the stdio runtime lands, the loopback HTTP checks, starter adapters, and local playground can run in parallel. The GUI and CLI then consume the same completed services.

| Task | Size | Requires | Deliverable and acceptance |
| --- | --- | --- | --- |
| **M1 · Local project model** | Medium | F1 | Implement versioned manifest validation, local catalog persistence, and create, open, and save. Verify round-trip data, safe migrations, and concurrent save behavior. |
| **M2 · Local execution policy** | Medium | F1 | Implement the shared policy evaluator, grants, execution limits, and redaction primitives. Prove deny-by-default behavior with allowed and denied cases. |
| **M3 · MCP runtime and stdio** | Large | M1, M2 | Implement discovery, tool calls, resources, resource templates, prompts, schema validation, progress, structured errors and cancellation for both revisions. Verify with an independent client over real stdio. Supply SDK pin and feature evidence with M4. |
| **M4 · Loopback HTTP conformance** | Medium | M3 | Verify both revisions over real loopback sockets. Exercise headers, JSON/SSE, per-request cancellation, issued legacy session lifecycle and sessionless comparison. Close the candidate SDK pin with M3. |
| **M5 · Starter integration templates** | Medium | M3 | Add the REST and read-only filesystem adapters under the shared policy. Verify approved requests and reads, denied destinations and paths, and fixture-based failures. |
| **M6 · Standalone generator** | Large | M5 | Generate a versioned Rust project with locked dependencies, working handlers, tests, and a README. Build and run it outside the builder, compare repeated generation, and verify edit preservation. |
| **M7 · Local playground** | Medium | M3 | Own process supervision, launch trust, redacted traces, and scenario replay. Verify start and stop, a fixture pass and fail, malformed input, and timeout recovery. |
| **M8 · Desktop MVP workflow** | Medium | M6, M7 | Own the desktop create, configure, test, and export workflow. Complete it on a clean Windows setup with a real project. |
| **M9 · CLI MVP workflow** | Medium | M6, M7 | Own the CLI validate, generate, and test workflow. Verify scripted success and actionable invalid-input and policy-denial output. |
| **M10 · MVP integration** | Medium | F2, M4, M8, M9 | Demonstrate GUI → CLI → standalone server from one integrated commit. Record native install and signing status, schema 1 fixture, wire tests, quality measurements and accessibility smoke. Product uses the pilot workflow results to test D2's positioning hypotheses. |

## Commercial v1

After M10, project lifecycle, credential work, Git and entitlement continuity can start. Once credential and policy interfaces are available, the OAuth server, OAuth client and remaining adapters can proceed in separate worktrees. Merge the services before completing the full desktop and CLI workflows.

| Task | Size | Requires | Deliverable and acceptance |
| --- | --- | --- | --- |
| **V1 · Full project lifecycle** | Medium | M10 | Implement rename, duplicate, passive builder project and Claude Code config imports, metadata, transport, dependency and environment settings. Verify schema 1 → 2 migration and recovery and archive export round trips. |
| **V2 · Credentials, approvals, and audit** | Medium | M10 | Implement OS credential storage, explicit reusable grants, and audit records with retention controls. Verify missing credential store behavior and redaction in persisted and exported evidence. |
| **V3 · Authenticated HTTP server** | Large | V2 | Implement the OAuth resource server, per-caller policy, Origin and proxy checks, request and rate limits and authenticated legacy session binding. Pin the reference provider trial with V4 and verify two-resource audience separation, key rotation and scoped allow/deny calls. |
| **V4 · Remote playground client** | Medium | V2 | Implement remote profiles, OAuth discovery, login, refresh and report export. Verify issuer, exact callback, PKCE, two-resource binding and refresh or reauthorization with the selected provider. V3 and V4 can develop in parallel and jointly verify the real provider and runtime before V10. |
| **V5 · Database adapters** | Medium | V2 | Implement SQLite and PostgreSQL templates with fixed parameterized operations. Verify allowed queries and denied access using scoped databases and credentials. |
| **V6 · Git adapter** | Small | M10 | Implement read-only operations against an approved repository. Verify repository boundaries and structured errors. |
| **V7 · Custom Rust handlers** | Medium | V2 | Implement the trusted-source build and handler path. Verify passive import, explicit build and launch trust, and independent compilation. |
| **V8 · Approved executable adapter** | Large | V2 | Implement shell-free invocation under the executable and argument policy. Verify constrained inputs, timeout, and process-tree cleanup on supported platforms. |
| **V9 · Full standalone generation** | Medium | V3, V5–V8 | Extend the generator for the full template set and remote configuration. Verify standalone builds and exercised handlers for each template with the same runtime policies. |
| **V10 · Client configuration exports** | Medium | V3, V4 | Produce Claude Code, Codex CLI, OpenCode V2 and generic configurations. Validate formats and credential references. Run actual revision and registration profiles and record blocked combinations. |
| **V11 · Complete desktop workflows** | Large | V1, V4, V9, V10, V13 | Add project management, advanced capability editors, credentials and approvals, remote testing, reports and recovery. Run the desktop acceptance path. |
| **V12 · Complete CLI workflows** | Medium | V1, V4, V9, V10, V13 | Add scripting equivalents for the release workflows. Verify versioned JSON, distinct failure exit codes, and agreement with desktop validation and policy decisions. |
| **V13 · Builder entitlement continuity** | Small | M10 | Own the builder-only entitlement boundary and missing, expired and unreachable cases. Prove existing project access, editing, validation, replay, recovery, security and export across those states. Product owns distribution terms. Verify the prior export runs with builder and licensing service absent. |

## Packaging, hardening, and release

The Linux deployment and native packaging assignments have independent outputs. Run the review and compatibility passes in parallel after they land. Assign fixes to the component owner so the review worktrees produce evidence without competing edits.

| Task | Size | Requires | Deliverable and acceptance |
| --- | --- | --- | --- |
| **R1 · Linux deployment** | Medium | V9 | Deliver Dockerfile, Compose and systemd setup, NGINX and health configuration, and operations docs. Verify clean deployment, credential rotation, wire cancellation, graceful shutdown, upgrade, rollback and restore within the reference targets. |
| **R2 · Platform release artifacts** | Large | V11, V12 | Deliver native installers and the release pipeline. Verify reproducible inputs, checksums, SBOMs, approved signing profiles, and install and launch on each named OS. Record affected targets when signing or test access is unavailable. |
| **R3 · Security and recovery** | Medium | R1, R2 | Run the security regression suite, dependency and container scans, and recovery drills. Record finding dispositions, test coverage, and release blockers. |
| **R4 · Compatibility and quality** | Medium | R1, R2 | Run the full client and platform matrix, accessibility checks, and performance workloads. Record negotiated revisions, covered workflows, and results against the Phase 0 budgets. |
| **R5 · Release candidate** | Large | R3, R4 | Integrate fixes, rerun the full gate, and complete documentation, changelog and approved redistribution records. Verify installation, upgrade, deployment and entitlement continuity from final artifacts. Record target dispositions and publication evidence after an authorized release. |

## Prioritized dependencies

The top rows are the critical path to the first integrated workflow. Native access, credentials and license review can be prepared in parallel. Each task records the exact missing input and affected target when it cannot produce evidence. [D4 evidence dependencies](docs/testing.md#evidence-dependencies-and-d5-decisions) · [Risk register](PLAN.md#risk-register)

| Priority | Dependency and release impact | Accountable owner and first gate |
| --- | --- | --- |
| P0 · Foundation | Integrate reviewed D5 on the shared branch. Establish exact toolchain and lock candidates, shared DTOs and the crate dependency direction. | Coordinator → F1, then platform F2 verifies native runners |
| P0 · Pilot engine | Stable project/policy types unblock M3. Dual-revision stdio unblocks starter adapters and the playground. M4 supplies HTTP wire evidence to M10. | Project core M1, policy M2, MCP M3/M4 |
| P0 · Pilot workflow | Starter adapters and standalone output, plus supervised replay, unblock desktop and CLI integration. Reserve a clean Windows 11 VM and D4 reference benchmark capacity. | M5/M6/M7 → M8/M9; platform and QA M10 |
| P1 · Native security | Reserve a clean Apple silicon Mac, Ubuntu desktop VM and supported filesystems, WebViews and credential sessions. Provide disposable fixtures for filesystem, Git, database, custom build and executable verification. | Platform F2/M7, credentials V2, adapters V5–V9, QA R3/R4 |
| P1 · OAuth and TLS | Supply disposable provider admin access, two MCP resources, public registrations and test CA trust. Test resource binding and refresh first. Verify client callback registrations, system browser access, CIMD hosting/egress or restricted DCR where required, and clean ingress DNS/TLS and renewal. | Security V3, playground V4, interoperability V10, deployment R1 |
| P1 · External clients | Obtain selected CLI versions and any required test accounts or seats. Use isolated profiles and record blocked builds or flows. A client row needs real invocation and auth evidence for the advertised profile. | Interoperability V10, QA R4 |
| P1 · Distribution rights and signing | Inventory runtime, template and third-party terms from M6 onward. Product approves redistribution. Arrange Windows trusted publisher/timestamp and Apple Developer ID/notary access for the restricted release jobs. | Generator M6/V9, platform R2, Product and release R5 |
| P2 · Final evidence | Provide clean Ubuntu server hosts for both profiles, backups and failure controls. Rehearse rollback and recovery, rerun the native and client matrix, meet quality gates, and verify the final artifact and docs. | Deployment R1, QA/security R3/R4, release R5 |

## Example first assignment

> Complete D1 from TASKS.md in a fresh Codex chat and worktree based on the planning commit. Work on protocol and client discovery only. Save your findings in `docs/mcp-compatibility.md` with sources, proposed version pins, an initial client and transport matrix, and the wire tests needed to confirm it. Return the branch or commit and the open questions D5 needs to resolve.

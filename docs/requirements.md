# MCP Server Builder product requirements

Phase 0 draft · 12 November 2026 · [Development plan](../PLAN.md) · [Task backlog](../TASKS.md) · [Commercial model](commercial-model.md)

MCP Server Builder lets developers configure an MCP server in a desktop editor, inspect and replay real MCP calls, and hand off editable Rust that builds and runs independently. The CLI uses the same project, validation, policy, and generation services so developers can reproduce their local checks in CI.

## Target users and workflows

| User | Job to complete | Evidence of success |
| --- | --- | --- |
| AI application developer using Claude Code, Codex, OpenCode, or a generic client | Expose an API or project files to an MCP client. Create a project, add capabilities, review access, and make a first call. | An independent client discovers and invokes the configured capability. The author can explain what access the server needs before launch. |
| Automation engineer or integration maintainer | Reproduce a failure, inspect a trace, save a fixture with an assertion, and replay it in CI. | The same saved scenario passes against the expected fixture and fails with an actionable difference when the fixture changes. |
| Internal platform engineer or service owner | Review a teammate's project and grants, build the export in a clean environment, and deploy it with a supported identity setup. | A second operator follows the supplied instructions, connects with a verified client configuration, and correlates a call with an audit event. |

The Windows pilot prioritizes individual developers and automation engineers. Commercial v1 adds the service owner workflow and verified Windows, macOS, and Linux desktop targets. Teams can hand off project files, source, and redacted evidence through their existing repositories and review processes.

## Release scope

| Milestone | User-facing outcome | End-to-end release check |
| --- | --- | --- |
| Local MVP | Windows desktop pilot and shared Rust CLI. Create, open, validate, and generate from a versioned local project. Configure a REST tool, a read-only resource and resource template, and a prompt. Launch over stdio, inspect a redacted trace, and replay a fixture. | On a clean supported Windows machine, create the reference project in the GUI, replay its scenarios from the CLI, then build the export from a separate directory and exercise it with an independent MCP client. Capture invalid input and a denied permission with actionable diagnostics. M10 records the run and the loopback HTTP conformance evidence. |
| Commercial v1 | Complete project lifecycle, additional templates, credentials and audit, remote playground, authenticated Streamable HTTP, client configuration export, native packages on verified targets, and Docker and self-hosted Linux deployment. | Install the release candidate on each advertised desktop target and complete the core workflow. On clean Linux infrastructure, build and deploy the export with the reference OAuth provider and exercise it with the supported client configurations. Record successful and denied calls, then verify shutdown, upgrade, rollback, and restore. R5 records results from final artifacts. |

Later candidates include legacy HTTP+SSE, optional extensions such as Tasks and MCP Apps, cloud-specific deployment adapters, hosted builder infrastructure, and centralized team administration. PostgreSQL ships as a v1 integration template. Hosted builder storage is a separate future decision. Payments require separate business authorization and credentials.

Use MCP `2026-07-28` as the baseline target and `2025-11-25` as the proposed compatibility floor. D1 validates the SDK pin, revision behavior, and client support. The modern revision uses stateless requests, per-request metadata, and `server/discover`. The compatibility path uses initialization and capability negotiation, with legacy Streamable HTTP session behavior exercised where supported. Publish the exact methods, revisions, transports, and optional features exercised in the release matrix. [MCP 2026-07-28 specification](https://modelcontextprotocol.io/specification/2026-07-28), [MCP 2025-11-25 specification](https://modelcontextprotocol.io/specification/2025-11-25)

## Reference project

Use a catalog sample throughout the GUI, CLI, and independent client checks. `lookup_item` takes a string ID and retrieves JSON from a configured REST endpoint. A deterministic upstream fixture returns item `42` with the name `Sample item`. A resource reads a catalog guide and a resource template reads per-item notes under a temporary approved root. A prompt takes an item ID and returns a templated review request. Keep a passing scenario, an expectation mismatch, an invalid argument, and a permission denial. Simulate upstream responses while exercising the MCP exchanges over real transports.

For v1, serve the sample over HTTPS with the reference OAuth provider. QA exercises every additional integration template with its own disposable data and credentials. The deployment acceptance run uses the generated service and actual token validation.

## Functional requirements and acceptance checks

The task IDs identify proposed delivery leads and the evidence each lead supplies. D5 assigns named owners and incorporates these checks into the shared requirements matrix. Commercial v1 inherits the local MVP requirements.

### Local MVP

| ID | Requirement | Observable acceptance check | Lead and tasks |
| --- | --- | --- | --- |
| MVP-01 | Create, open, edit, save, and validate a local project with a versioned portable manifest and a local catalog. Surface errors at the affected field or capability. | Save the reference project, restart the desktop app, and reopen it with the same capabilities and policy. Open a copy in another directory and validate it from the CLI. An unsupported manifest version and an invalid field each produce a diagnostic with a next step. | Project core · M1, M8, M9 |
| MVP-02 | Provide guided editors for a REST tool, a read-only file resource, a resource template, and a prompt. Expose tool name, description, argument and optional result schemas, handler settings, timeout, and permission rules. Expose resource URIs and prompt arguments in their editors. | Configure all four capabilities from the desktop. Invalid definitions highlight the relevant field and block launch or generation. A valid project saves and its capability names, schemas, URI template, and prompt arguments match the client discovery results. | Desktop · M5, M8 |
| MVP-03 | Show the effective permissions and require explicit trust before local launch. Review the resolved executable, arguments, working directory, environment references, allowed destinations and paths, access modes, and limits. Preview and generated servers enforce the same deny-by-default rules at file access and network connection time. | Cancel the review and observe zero process launches. Approve a launch with a configured host and read root and see both operations succeed. A redirect to an unapproved fixture host and a read that resolves outside the approved root are denied. Run the allow and deny cases against preview and export and compare the decisions. | Runtime and policy · M2, M5, M7 |
| MVP-04 | Store secret references in project data. Resolve needed values from an explicitly supplied environment or session source in the pilot. Bound traces and redact secret values and designated sensitive fields before display, persistence, or export. | A missing reference identifies the setting to fix. A supplied canary value resolves for the adapter and is absent from the manifest, diagnostics, visible trace, persisted evidence, and generated files. The trace viewer marks truncated entries when a configured limit is reached. | Runtime and policy · M2, M5, M7 |
| MVP-05 | Run projects over stdio with the planned revision-specific lifecycle, discovery, tool calls, resource and template reads, prompt retrieval, structured errors, and supported cancellation and notifications. Validate arguments before dispatch and structured results against any declared output schema. | Independent wire tests exercise both planned revisions, discover all four sample capabilities, and check expected responses and advertised notifications. An invalid argument leaves the handler invocation count unchanged. A result that violates its declared schema returns a diagnostic. An unsupported revision and an interrupted call follow the corresponding revision's defined behavior. | MCP runtime · M3 |
| MVP-06 | Verify the Streamable HTTP binding on loopback as an engineering milestone for v1. | Use an independent test client for both planned revisions. Record discovery and calls, modern header and body validation, transport cancellation, and any compatibility session behavior selected by D1. | MCP runtime · M4, M10 |
| MVP-07 | Start and stop a supervised stdio server. Browse discovered capabilities, run calls with JSON input, inspect redacted request and response messages, duration, and errors, and save scenarios with fixtures and expectations. | Save a scenario in the GUI and replay it twice from the CLI with the same pass result. Change the expected name and get an assertion failure that identifies the difference. Exercise malformed input against a controlled fixture, then cancel or time out a slow case. Verify the playground can run again and stop without leaving an owned process behind. | Playground · M7, M8, M9 |
| MVP-08 | Generate editable Cargo source with real handlers, pinned inputs, a lockfile, config examples, a README, and unit and integration tests. Record schema, runtime, template, and protocol versions. Stage writes and surface conflicts before regeneration can replace edited files. | Generate twice from identical inputs and compare the emitted file contents. Copy the export outside the builder workspace, run `cargo build --locked` and `cargo test --locked`, and use an independent client to exercise the capabilities. Edit a project-owned file and verify regeneration preserves it or asks the user to resolve the conflict before writing. | Generator · M6, M8, M9 |
| MVP-09 | Recover from save conflicts, interrupted writes, and local run or generation failures. Preserve the last valid project and user edits, and identify the action that can recover the workflow. | Simulate a concurrent save and a failed write. Reopen a valid committed revision and recover the losing editor's draft as a copy. Interrupt generation and verify the last complete output remains usable and a retry succeeds. A failed local start leaves the editor available and reports the executable or setting to fix. | Project core · M1, M6, M7, M8 |
| MVP-10 | Script validation, generation, and scenario replay from the CLI against the desktop project. | Run all three operations in sequence from a fresh shell. Compare validation and permission decisions with the desktop. A validation error, policy denial, or failed test returns a nonzero status with the failing field, rule, or assertion. | CLI · M9, M10 |

### Commercial v1

| ID | Requirement | Observable acceptance check | Lead and tasks |
| --- | --- | --- | --- |
| V1-01 | Rename and duplicate projects, edit server metadata, transport, dependency, and environment settings, and import and export versioned builder projects. Preview a migration and preserve a recoverable original. | Round-trip a project to a clean profile and compare its capabilities, portable settings, policies, and scenarios. Rename and duplicate it and verify the two project records and files can be edited independently. Migrate a supported older project and inject a failed migration to verify the original reopens. | Project core · V1, V11, V12 |
| V1-02 | Import the selected client configuration formats as connection profiles. Preview parsed commands, URLs, working directories, and environment references. Require explicit review before launching or connecting. Validate project archives before writing project files. | Import supported stdio and HTTP examples without spawning or connecting. Identify unsupported entries and reject an unsafe archive before writing project files. Start a test connection after review. Reopen a builder project export for editing with its handler definitions. | Project core · V1, V11, V12 |
| V1-03 | Resolve local credentials through the OS store with a session or explicit environment fallback. Resolve deployed credentials from deployment-managed secret injection. Allow explicit, scoped, revocable reuse of approvals. Record principal, operation, policy decision, outcome, timestamp, and correlation ID in audit events with retention controls. | Simulate an unavailable credential store and complete a test via the fallback. Broaden or revoke an approved grant and verify the previous approval cannot authorize the new action. Verify allowed and denied calls produce correlated events, the retention limit removes expired evidence, and a canary secret stays out of logs, traces, reports, and exports. | Security and policy · V2, V11, R3 |
| V1-04 | Serve authenticated Streamable HTTP over HTTPS behind the documented ingress, with an external OAuth authorization server. Enforce issuer, resource or audience, expiry, scope, caller and capability policy, Origin handling, and configured request and rate limits. | Discover protected resource metadata and complete an allowed call with a token from the reference provider. Missing, invalid, expired, or wrong-resource tokens and insufficient scopes receive the applicable challenge or denial. Verify every rejected request leaves the adapter invocation count unchanged, including invalid Origin, oversized requests, and excess rate. | MCP and security · V3, R1, R3 |
| V1-05 | Connect the playground to running MCP servers via supported profiles. Support remote discovery, OAuth login and refresh, call inspection, reusable scenarios, and redacted report export. | Log in to the deployed sample, discover capabilities, and make a scoped call. After access token expiry, verify refresh restores calls. Revoke the refresh grant and expire the access token again, then verify the playground prompts for sign-in. Export a report with the scenario result, duration, revision, and correlation ID and verify its redaction. | Playground · V4, V11, V12 |
| V1-06 | Provide SQLite and PostgreSQL tools using configured parameterized statements, a scoped file or database, and least-privilege credentials where applicable. | A permitted query returns seeded data on each engine. Invalid arguments and out-of-scope operations are denied, and the protected rows and files are unchanged after the denied cases. | Integrations · V5, V9 |
| V1-07 | Provide read-only Git operations against an approved repository. | Read a seeded commit or diff from the approved fixture. Deny a write request and a request outside the approved repository. Verify repository state is unchanged. | Integrations · V6, V9 |
| V1-08 | Support custom Rust handlers and build dependencies as trusted source. Show what will build or run under the local user and collect explicit build and launch approval. Document the enforced boundary for each platform. | Import a project with a test build hook and verify the hook stays inert during import, validation, and a declined build approval. After explicit approvals, compile independently and invoke the handler through MCP. | Generator and security · V7, V9 |
| V1-09 | Run approved executables directly with validated arguments, a scoped working directory and environment, and a timeout. Supervise their process trees on each supported platform. | Run an approved fixture executable with allowed arguments. A disallowed executable or argument yields a denial before spawn. Time out a fixture with a child process and verify both exit on each advertised platform. | Runtime and policy · V8, V9, R3 |
| V1-10 | Generate standalone stdio and authenticated HTTP servers for the v1 templates, carrying their runtime policies and deployment configuration. | Build and test exports on a clean machine using documented prerequisites. Exercise each template's generated handler and its policy denials. Compare results to preview and verify the exported servers start with the builder absent. | Generator · V9, R1 |
| V1-11 | Export client configurations and generic connection instructions for Claude Code, Codex, OpenCode, and independent clients. Use credential references or supported client login flows. | Use each advertised client/version/transport combination to register or connect, discover capabilities, and invoke a tool. Exercise resources and prompts where exposed. Record OS, revision selected, auth mode, capabilities, and which documented combinations still need verification. Verify export leaves the user's existing client configuration intact. | MCP and QA · V10, R4 |
| V1-12 | Complete desktop and CLI workflows for project management, permission review, testing, generation, and report and configuration export. The CLI offers human output, versioned JSON, and stable exit codes. | Use each interface to import and validate the same project, review grants, replay a remote test, generate source, and export a report and client configuration. Compare validation and policy decisions. Automated CLI checks distinguish success, invalid configuration, policy denial, test failure, and process failure by exit status and JSON diagnostics. | Desktop and CLI · V11, V12 |
| V1-13 | Deliver generated Dockerfile and Compose inputs and self-hosted Linux service instructions, with environment-based config, health checks, graceful shutdown, structured logs, and backup and restore guidance. | On clean Linux infrastructure, deploy with the reference OAuth provider and confirm readiness and an authenticated MCP call. Follow the runbook to shut down, upgrade, roll back, and restore persisted state. Call the restored service and verify it returns the expected data. | Platform · R1, R5 |
| V1-14 | Publish versioned native installers for verified Windows, macOS, and Linux targets and versioned Linux deployment artifacts. Include checksums, SBOMs, and signing status. | Install, launch, and upgrade from release artifacts on every advertised OS target. Verify the published checksums and available signatures and record release evidence against each package. | Platform · R2, R4, R5 |
| V1-15 | Preserve access to existing project data, security controls, recovery, and standalone output when licensing is unavailable. Any entitlement logic belongs to the builder. | Open, edit, validate, test, recover, and export an existing project with no entitlement, an expired entitlement, and an unreachable licensing endpoint. Verify in each case that a denied permission still blocks the call and an already exported server still runs with its normal security controls. | Product and platform · D5, R5 |

## Desktop workflow sketches

### Create a project and add capabilities

```text
Projects                                  [New project] [Open folder]
Empty list → Create a project or open a project folder.  [Import] in v1
  New project
  Name [Catalog sample              ]  Folder [                    ] [Browse]
  Transport [stdio]                     [Cancel] [Create project]
    ↓
  Capabilities                          [Add tool ▾] [Add resource ▾] [Add prompt]
  lookup_item   REST                     [Schema] [Handler] [Permissions]
  Host is required.                      [Go to host] [Save] [Validate]
```

A resource template is available from the resource menu. Folder or name errors keep the create form open. Validation takes the author to the affected field and retains the draft. Covered by MVP-01, MVP-02, V1-01, and V1-02.

### Review permissions and launch trust

```text
Review before starting Catalog sample
  Executable [resolved path and arguments]   Working directory [resolved folder]
  Environment  CATALOG_TOKEN (reference)    Timeout [configured limit]
  lookup_item  GET [fixture host and port]  Other destinations denied
  Catalog docs Read [approved root]         Other paths and writes denied
  [Edit policy] [Cancel] [Approve this launch]
  v1  [ ] Remember this exact approval      [Manage approvals]
```

Show the revised permissions before launch when a binding or launch setting changes. V1 import uses a passive preview and leads to the same review when the user later starts a process. Deployment uses the reviewed runtime policy and resolves its own credential references. Covered by MVP-03, MVP-04, V1-02, V1-03, V1-08, and V1-09.

### Run and replay a test

```text
Playground   [Start local] → Starting… → Connected (stdio)       [Stop server]
  Capabilities [lookup_item ▾]         Scenario [Known item ▾]   Mode [Fixture ▾]
  Input {"id":"42"}                    Expect name [Sample item]
  [Run] [Cancel call] [Save scenario]   v1 [Connect profile] [Export report]
  Pass · 24 ms (example)                [Result] [Protocol trace] [Errors]
  Trace shows request, response, and timing with sensitive fields redacted.
```

A failing assertion shows expected and actual values. A timeout or connection failure shows what failed and whether to restart, reconnect, or update credentials. Show the selected fixture or live target before a run. Covered by MVP-05, MVP-07, V1-04, and V1-05.

### Export and verify

```text
Export Catalog sample
  Output [Standalone Rust ▾]             Destination [             ] [Browse]
  v1 options  Builder project · Client configuration (select client and transport)
  Review  Files · Runtime/template versions · Permissions · Required secret references
  Existing edited file → [Review changes] [Choose another folder]
  [Cancel] [Export]
    ↓
  Export complete  [Open folder] [Copy build command] [Open README]
```

Completion follows a successful write. The README gives build, test, and independent connection steps for the selected target. Covered by MVP-08, MVP-10, V1-01, V1-10, V1-11, and V1-13.

### Recover a project or failed operation

```text
Catalog sample · Save conflict
  This project was saved elsewhere while you were editing.
  Saved revision [on disk]         Your draft [available]
  [Compare changes] [Save draft as copy] [Reload saved revision]
  Failed migration → [Open original] [View validation details]
  Failed run/export → [View details] [Fix setting or destination] [Retry]
```

Confirm before discarding unsaved edits. Show the location and revision of any retained copy. Retry a run after confirming the state of any earlier operation that may have written data. Covered by MVP-09, V1-01, V1-03, and V1-05.

## Quality and release evidence

| ID | Requirement and acceptance evidence | Milestone and lead |
| --- | --- | --- |
| Q-01 | Run Rust and Angular unit tests, independent wire checks, generated build and run tests, GUI and CLI workflow checks, and security regressions. Record at least 80% line coverage for core logic and 90% for security-critical modules with justified exclusions. Test save conflicts, missing credentials, network loss, cancellation, and restore where the release supports the workflow. | MVP and v1 · QA and engineering, M10 and R3–R5 |
| Q-02 | Complete creation, review, testing, export, and recovery by keyboard. Native smoke runs check visible focus, meaningful labels, and errors conveyed in text. Use a screen reader to check control names and validation feedback on each supported desktop target. | MVP and v1 · Desktop and QA, M8 and R4 |
| Q-03 | Build from pinned inputs on clean workers. Compare repeat builds, record platform-specific differences, run dependency and secret scans, and scan release containers. Resolve critical and high findings or record acceptance by the authorized owner. Verify install, client connection, and deployment documentation on clean systems. | MVP build and v1 release · Platform and security, F2 and R2–R5 |
| Q-04 | Measure the reference workload against the proposed budgets below. Record platform, build, fixture revision, raw samples, and percentile calculation. | MVP and v1 · QA and platform, D4, M10, and R4 |

Proposed budgets for D4 and D5 use a release build on a recorded 4 vCPU, 8 GiB machine with local SSD storage, a prebuilt server, and a local upstream fixture. Measure startup from launch to successful discovery over 30 starts with a fresh process each time. Measure latency over 200 sequential calls after 20 warm-up calls. Record server resident memory after 60 seconds idle. For the v1 HTTP run use local HTTPS and a valid token with warmed provider metadata. Time installation, compilation, and browser sign-in separately.

| Measure | Proposed target |
| --- | --- |
| Start to discovery | p95 at most 2 seconds |
| REST tool round trip | p95 at most 100 ms over stdio and 150 ms over HTTPS on the reference workload |
| Idle server memory | At most 128 MiB resident memory on each reference run |
| Stop after a normal run or canceled slow case | Within 5 seconds with zero owned descendants in 10 consecutive repetitions |

## Decisions for D5

- Use D1's wire evidence to settle the SDK pin, revision and transport commitments, and client-specific support. Start import discovery with Claude Code `.mcp.json` stdio and HTTP entries. Confirm the minimum supported formats and round-trip behavior alongside the broader client export matrix.
- Use D3 to settle approval scope and invalidation, credential resolution, redaction and retention rules, executable supervision, the reference OAuth provider, and platform-specific enforcement. Keep passive import, deployment policy, and local launch approval explicit in the shared contracts.
- Use D4 to confirm OS and architecture targets, signing and client account dependencies, the benchmark method and budgets, and which evidence is automated or collected by native smoke tests.
- Finalize portable manifest and local catalog boundaries, connection and scenario schemas, recoverable saves and regeneration ownership, IPC and CLI diagnostics, and report formats. Assign owners for the acceptance checks and place licensing and redistribution decisions in the shared backlog. [Commercial model](commercial-model.md)

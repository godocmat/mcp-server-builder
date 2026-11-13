# MCP Server Builder progress

13 November 2026 · F2 and M2 integration · [Roadmap](PLAN.md) · [Backlog](TASKS.md) · [Decisions](DECISIONS.md)

Phase 0 is complete. The coordinator reviewed and integrated D5 (`7767057`) on top of the D1–D4 baseline (`780b107`) on `master`. The design selects the portable, runtime, desktop and CLI contracts. Implementation and release tasks own the verification below.

D5 checks covered relative links and anchors, Markdown tables, all 29 requirement mappings, task dependency cycles, the 18 brief checklist items, both JSON examples and Mermaid syntax for the four architecture diagrams.

| Assignment | Phase 0 evidence |
| --- | --- |
| D1 · MCP and clients | [Protocol lifecycles, SDK and client source findings, wire matrix](docs/mcp-compatibility.md) |
| D2 · Product and UX | [Requirements and workflow sketches](docs/requirements.md) · [Competitive landscape, pilot and pricing hypotheses](docs/commercial-model.md) |
| D3 · Security | [Threat model, trust boundaries, provider shortlist and TM-001–TM-008](docs/security.md) |
| D4 · Platform and QA | [Native packages, ingress and release plan](docs/deployment.md) · [Fixtures, checks, benchmark method and evidence dependencies](docs/testing.md) |
| D5 · Shared design | [Architecture](docs/architecture.md) · [Contracts](docs/contracts.md) · [Traceability](docs/traceability.md) · [ADRs](DECISIONS.md) · [Risks](PLAN.md#risk-register) · [Prioritized dependencies](TASKS.md#prioritized-dependencies) |

## Pre-implementation design checklist

The coordinator confirmed that the artifacts below cover the 18 items in the product brief. Implementation evidence follows the task gates.

| Brief item | Design artifact |
| --- | --- |
| 1. Product requirements document | [Product requirements and reference project](docs/requirements.md) |
| 2. Competitive landscape analysis | [Documented alternatives and pilot comparison](docs/commercial-model.md#competitor-workflows) |
| 3. Functional requirements | [MVP and v1 product requirements](docs/requirements.md#functional-requirements-and-acceptance-checks) · [Owning tasks and checks](docs/traceability.md) |
| 4. Non-functional requirements | [Quality, evidence and benchmarks](docs/traceability.md#quality-and-release-requirements) · [Security limits](docs/contracts.md#cancellation-deadlines-and-limits) |
| 5. System architecture | [System and deployment relationships](docs/architecture.md) |
| 6. Component diagrams | [Workspace components](docs/architecture.md#components-and-workspace-boundaries) · [Project and request flow](docs/architecture.md#project-and-request-data-flow) |
| 7. Data model | [Logical data model](docs/architecture.md#logical-data-model) · [Portable project and persistence](docs/contracts.md#portable-project) |
| 8. Threat model | [Assets, trust boundaries and threats](docs/security.md#trust-boundaries-and-threats) |
| 9. Security architecture | [Policy and adapters](docs/security.md#policy-enforcement) · [Approval and credential contracts](docs/contracts.md#policy-approvals-and-credentials) |
| 10. CLI command specification | [Command operands, behavior and exit codes](docs/contracts.md#cli-command-specification) |
| 11. GUI wireframes | [Creation, import, permission review, test, export and recovery](docs/requirements.md#desktop-workflow-sketches) |
| 12. API contracts | [Service and Tauri IPC contracts](docs/contracts.md#application-services-and-tauri-ipc) · [Scenario, report and audit formats](docs/contracts.md#scenarios-results-and-evidence) |
| 13. MCP compatibility matrix | [D1 wire matrix](docs/mcp-compatibility.md#wire-tests-and-release-evidence) · [Selected revisions and SDK profile](DECISIONS.md#adr-002--dual-revision-mcp-and-sdk-boundary) · [Selected clients and provider](DECISIONS.md#adr-007--external-oauth-and-client-profiles) |
| 14. Deployment architecture | [Deployment diagram](docs/architecture.md#deployment-relationships) · [Native and Compose profiles](docs/deployment.md#generated-linux-service) |
| 15. Testing strategy | [D4 test strategy](docs/testing.md) · [Acceptance, coverage and benchmark gates](docs/traceability.md) |
| 16. Release plan | [Platform and signing choices](DECISIONS.md#adr-008--toolchain-platforms-and-packaging) · [Rollout and recovery](DECISIONS.md#adr-009--ingress-rollout-and-recovery) · [Packaging through release assignments](TASKS.md#packaging-hardening-and-release) |
| 17. Prioritized backlog | [Implementation assignments](TASKS.md) · [Prioritized dependencies](TASKS.md#prioritized-dependencies) |
| 18. Risk register | [Consolidated risks, responses and verification owners](PLAN.md#risk-register) |

## F1 entry conditions

1. The coordinator reviews the D5 checklist, confirms the scope, owning tasks and interfaces, and integrates the D5 commit into the shared branch. Record that resulting integration commit as the F1 worktree base.
2. Dispatch F1 with the [workspace layout and dependencies](docs/architecture.md#components-and-workspace-boundaries), the [portable and IPC contracts](docs/contracts.md), and the [toolchain trial](DECISIONS.md#adr-008--toolchain-platforms-and-packaging).

F1 owns the workspace manifests, initial lockfiles and shared DTOs. Use the candidate versions for the first build and record exact inputs and any compatibility changes. F1 hands off the clean build, minimal CLI and desktop smoke, and candidate dependency and toolchain evidence to F2 and M1/M2. F2 owns the cross-platform CI matrix. M3/M4 own the SDK wire and feature validation. Provider, client account, package signing and distribution evidence enter at the gates below.

## F1 workspace foundation

Branch `codex/f1-workspace-foundation` started from reviewed Phase 0 integration commit `1eb58a03d4c931741d4f96fab69f34f8fe3874bd`. It adds the nine-member Cargo workspace, a shared Rust smoke service and diagnostic DTOs, a Clap CLI, and the Tauri main window backed by strict Angular and Signals. The Rust toolchain and both lockfiles are committed. [Workspace dependencies](docs/development.md#workspace-dependencies)

Windows 11 Pro 25H2 x64 passed a fresh `cargo build --locked --workspace --features mcp-builder-desktop/custom-protocol`, the default locked workspace build, Rust formatting, Clippy and both Rust contract tests. Angular passed `npm ci --engine-strict`, formatting, production build, TypeScript and both IPC mock tests. The CLI returned human and versioned JSON results with exits `0` and `2` as specified for the tested cases. The native WebView2 app displayed its successful typed IPC reply, and the correlation ID matched the Rust trace. [Commands and observed results](docs/development.md#locked-build-and-smoke)

The tested inputs use Rust `1.99.1`, Node `24.22.3` and npm `11.19.1` as the primary pair, Angular `22.2.6` with CLI/build `22.2.7`, TypeScript `6.0.3`, Tauri Rust `2.12.5` and Tauri JavaScript `2.12.4`. Node `26.13.1` with npm `11.20.0` also passed the locked Angular checks and Tauri debug build. User-local Microsoft packages supplied the initially missing MSVC linker, C++ tools and Windows SDK. The six separate `rmcp 3.9.0` builds passed, including Rustls and Windows native TLS. [Toolchain and prerequisite evidence](docs/development.md#f1-toolchain) · [Resolved SDK feature graphs](docs/development.md#mcp-sdk-build-probes)

| Next task | F1 handoff |
| --- | --- |
| F2 · Build and test CI | Use the locked commands, shared Rust/TypeScript fixture and native smoke on the named Windows, macOS and Linux runners. Check Node and WebView compatibility and keep the TLS probes separate. |
| M1 · Local project model | Implement schema 1 and validation in `builder-project` and shared project services and the catalog in `builder-app`. Use the common reply and diagnostic contract at the shell boundary. |
| M2 · Local execution policy | Implement grants, constraints, limits and redaction in `mcp-policy`. `mcp-runtime` already has the dependency edge for shared dispatch. |
| M3/M4 · MCP | Use the optional SDK profiles in `mcp-transport` for real stdio and HTTP tests. Verify revision and session behavior, cancellation and the final SDK feature selection. |

## F2 build and test CI

The `codex/f2-build-test-ci` branch starts from the verified F1 integration `139f0920d4e0ddff2244cbf4fab5fd32e7dd8ff0`. It adds the Windows x64, macOS arm64 and Ubuntu amd64 foundation workflow, CLI and SDK checks, per-platform LCOV artifacts and a debug-only native WebView trial. [Workflow](.github/workflows/foundation.yml) · [Commands and evidence](docs/ci.md)

Local Windows 11 x64 and Ubuntu 24.04 amd64 userspace passed the Rust, Angular, CLI and six locked SDK checks, including the shared Rust/TypeScript fixture. Native WebView2 and WebKitGTK both rendered the typed IPC reply and matched the Rust trace. Rust reported 44 of 127 authored lines and Angular reported 9 of 11. [Local results](docs/ci.md#observed-local-results)

The first GitHub run supplies evidence for the named workers, including Apple silicon compilation and WKWebView. M3/M4 add wire and Inspector evidence, and M8/M10 and R2/R4 add installed native evidence. [Hosted and integration handoff](docs/ci.md#first-hosted-run)

## M2 local execution policy

Branch `codex/m2-local-execution-policy` started from verified F1 integration `139f0920d4e0ddff2244cbf4fab5fd32e7dd8ff0`. The shared policy crate implements portable grant validation, caller visibility, operator and caller authorization, verified target matching, per-call ceilings and evidence redaction. M1 can use the grant types and field diagnostics. M3 can use the evaluator and limits for dispatch, discovery and cancellation. [M2 API, checks and handoff](docs/local-execution-policy.md)

The Windows policy suite passed 15 tests, including synthetic-secret scans for structured evidence and buffered stderr. The non-desktop workspace passed 17 tests. Formatting and Clippy passed. `Cargo.lock` records the policy crate's dependencies using package versions already locked by F1.

## F2 and M2 integration

This baseline combines F2 `d643794` and M2 `b80e47c`. The combined Cargo lock retains F2's WebDriver dependencies and adds M2's dependency edges. The first GitHub Actions run still requires a GitHub remote. M1's project services and their shared policy integration are the remaining prerequisites for M3. [Hosted verification](docs/ci.md#first-hosted-run) · [MCP runtime dependencies](TASKS.md#local-mvp)

On Windows, the integrated checkout passed Rust formatting, both locked workspace builds, all 17 workspace tests with embedded Angular assets and workspace Clippy with warnings denied. The CLI passed help, version, text and versioned JSON checks. On the primary Node and npm pair, Angular passed the locked install, formatting, production build, TypeScript, two IPC component tests and coverage collection. Actionlint passed and both lockfiles stayed intact.

## Verification carried forward

| First task | Evidence still required |
| --- | --- |
| F2 · Foundation and platform | Record a hosted run on the three named workers, both coverage artifacts per worker, and the native trial outcome for each WebView. [F2 hosted handoff](docs/ci.md#first-hosted-run) |
| M1/M2/M5–M10 · Pilot owners | Prove safe saves and recovery, approved adapter and launch boundaries, Windows Job Objects and filesystem behavior, secret canaries, clean standalone build, coverage, benchmark and Windows installer/CLI and accessibility smoke. |
| M3/M4 · MCP | Verify both revisions over real stdio and HTTP, the issued legacy and sessionless comparison, cancellation and the initial SDK pin. |
| V1/V2/V5–V9/V13 · Project, runtime and application services | Verify pilot migration and passive import, native credential backends and approval invalidation, per-adapter policy and cleanup, full standalone exports and entitlement continuity. |
| V3/V4/V10 · OAuth and interoperability | Record actual provider, database and access-token settings, TLS and trust inputs, audience isolation, callback and refresh evidence, client registrations, and real client versions and wire outcomes. |
| R1/R3 · Deployment and security | Verify DNS/TLS and renewal, both Linux deployment profiles, bounded drain, secret rotation, denial, coverage and clean restore after a failed rollout. |
| R2/R4/R5 · Platform, QA and product | Verify native installers and final artifact identity and signing, benchmark and compatibility matrix, authorized redistribution terms and notices, clean install/upgrade and final documentation. Record withheld or unsupported combinations and publication evidence after an authorized release. |

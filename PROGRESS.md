# MCP Server Builder progress

12 November 2026 · Phase 0 handoff · [Roadmap](PLAN.md) · [Backlog](TASKS.md) · [Decisions](DECISIONS.md)

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

## Verification carried forward

| First task | Evidence still required |
| --- | --- |
| F1/F2 · Foundation and platform | Resolve exact Rust, Node, Angular, TypeScript, Tauri and native prerequisites on the named workers. Record candidate SDK/TLS feature graphs. Verify WebViews, DTO parity, CI reporting and native GUI automation options. |
| M1/M2/M5–M10 · Pilot owners | Prove safe saves and recovery, approved adapter and launch boundaries, Windows Job Objects and filesystem behavior, secret canaries, clean standalone build, coverage, benchmark and Windows installer/CLI and accessibility smoke. |
| M3/M4 · MCP | Verify both revisions over real stdio and HTTP, the issued legacy and sessionless comparison, cancellation and the initial SDK pin. |
| V1/V2/V5–V9/V13 · Project, runtime and application services | Verify pilot migration and passive import, native credential backends and approval invalidation, per-adapter policy and cleanup, full standalone exports and entitlement continuity. |
| V3/V4/V10 · OAuth and interoperability | Record actual provider, database and access-token settings, TLS and trust inputs, audience isolation, callback and refresh evidence, client registrations, and real client versions and wire outcomes. |
| R1/R3 · Deployment and security | Verify DNS/TLS and renewal, both Linux deployment profiles, bounded drain, secret rotation, denial, coverage and clean restore after a failed rollout. |
| R2/R4/R5 · Platform, QA and product | Verify native installers and final artifact identity and signing, benchmark and compatibility matrix, authorized redistribution terms and notices, clean install/upgrade and final documentation. Record withheld or unsupported combinations and publication evidence after an authorized release. |

# Managed local playground

The Windows pilot starts the installed builder CLI as a supervised stdio worker. The application service reads the saved schema 1 manifest, builds an inert review and checks the current revision again on admission. A review binds the project's registration and exact manifest bytes, the approved grants and prompts, the selected mode and scenario, native file-root identities, credential sources and policy limits. Native review binds the Windows operator SID, resolved executable and working directory identities, executable bytes, the argument vector and the environment.

Windows starts the reviewed console PE image on local NTFS in a kill-on-close Job Object with an explicit three-handle stdio list. The install directory supplies the working directory. The worker runs under the operator's account and shares the trust of the installed binary and its libraries. Its Job Object contains normal job members and limits their lifetime. Any other launcher in the same host must use an explicit handle list so its child cannot inherit a preview pipe. The worker uses the same runtime, policy and starter adapters as a standalone export. The current supervisor returns `playground.launch_unsupported` on Linux and macOS.

## Replay a saved fixture

Run these commands from PowerShell against a folder containing the catalog sample saved as `mcp-project.json`. The selection of grants and prompts defaults to empty. Use the same selections for review and replay.

```powershell
mcp-builder playground review 'C:\work\catalog' --scenario known_item --grant catalog_read
mcp-builder --json playground replay 'C:\work\catalog' --scenario known_item --grant catalog_read --approve 'sha256:<digest-from-review>'
```

Use `--protocol modern` for MCP `2026-07-28` or `--protocol legacy` for `2025-11-25`. Auto is the default. `--timeout-ms` bounds the call and accepts 1 through 30,000 ms. JSON output includes a versioned terminal run record and a bounded trace with metadata. Use `--trace-payloads` with `--json` to explicitly export the current sanitized trace. An assertion mismatch returns exit 4 and the affected assertion index and visible path. Omitted or stale approval returns exit 3. A matching saved error assertion is a passing run, while a missing fixture remains a failed run.

Each CLI replay explicitly supplies the current digest for that operation. In the application service the prepared review expires after five minutes. Approval and start consume their respective objects, including on failure. Prepare a fresh review after an edit or a change to approved targets. Fixture replay performs no DNS, upstream connections, file-root access or credential lookup. Named REST origins use a fixed synthetic public peer. Explicit peer ranges can deny that peer. IP literals and localhost use the corresponding address in the shared policy check.

## Use the application service

`ProjectService::prepare_playground` takes the selected registration or project folder, a trusted first-party worker path, mode and approved access. Present the returned review before calling `approve`. `ApprovedProjectPreview::start` checks the saved revision and starts the supervised session. The managed connection offers discovery, scenario replay, live invocation, a trace snapshot and stop. For a live session, configure concrete roots and choose session-only or explicitly named environment credential sources. Approved REST credentials are scoped to the project, local principal, capability, binding, reference and original upstream. Credential values travel over the private pipe handoff. The handoff runs before MCP initialization and carries the approved manifest snapshot. The worker inherits an empty environment.

The client accepts only the two supported revisions, bounds stdio frames and propagates cancellation at request scope. Start an installed worker in the same runtime that will manage its asynchronous pipes. Explicit stop closes stdin and gives the worker up to two seconds to exit. It then terminates and checks the job within the five-second shutdown budget. Dropping the session closes the job. Close a session before switching projects or policy selections.

Trace capture is in memory and defaults to metadata. The ring bounds entry count and serialized bytes, records loss and truncation, and discards transient payloads on close. The CLI snapshots the requested trace before stopping. Peer error text and raw stderr stay outside the trace. Display payloads and opt-in evidence use credential and scenario-path redaction. Assertion checks run on the bounded normalized result before redaction. Live writes conservatively report an unknown effect where the starter runtime cannot confirm an upstream commit.

M8 owns Tauri session registration, IPC and installed worker packaging. Hosts that enable tracing must apply `sdk_diagnostic_filter` to each diagnostic layer. The CLI applies it and starts the worker before initializing its own subscriber. M9 owns the full scripted validation and test commands. Per-profile run history and export reports follow the evidence-store work in the plan.

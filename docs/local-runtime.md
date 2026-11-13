# M3 shared runtime

[Project model](local-project-model.md#handoff) · [Execution contract](contracts.md#mcp-transport-and-execution) · [Policy](local-execution-policy.md)

`builder_project::runtime_definition(&ValidatedManifest)` projects the schema 1 server identity, capability declarations, binding IDs and requested grants. The registered REST operation comes from the configured method. A file resource always uses `file.read`. `mcp_runtime::Runtime::new` takes the resulting `ServerDefinition`, a separately reviewed `OperatorPolicy` and a trusted adapter. It owns an immutable registry and policy snapshot for the lifetime of a served runtime. The runtime crate builds independently of the builder project format and MCP transport SDK.

`Runtime` exposes server identity, caller-filtered capabilities, four list methods and async tool call, resource read and prompt get. Callers supply a trusted `Principal` and a request-specific `ExecutionContext` with a cancellation token and optional progress sink. Hidden entries use the same typed errors as unknown entries. Pagination sorts by names or URIs and limits pages by count and serialized size. The runtime issues opaque cursors scoped to the list kind, caller identity and current visible entries. A replaced runtime or an evicted cursor requires restarting the list.

## Adapter boundary

The registry checks the caller's visibility and validates input before calling `Adapter::prepare`. Preparation returns a `PreparedCall` that retains the target used by execution. It receives a deadline, cancellation token and correlation ID. The runtime evaluates a prepared `VerifiedTarget` against the binding operation and shared policy before invoking `execute`. A prepared REST route receives a route-only check before DNS. Its live adapter must authorize the concrete peer before use. Preparation stays free of the requested operation and credential resolution. [Starter adapters](starter-adapters.md)

`AdapterContext` supplies the permit's limits and deadline. A changed target requires `authorize_target` before use. A recheck can only narrow the original deadline and byte limit. A failed recheck returns its typed denial when the adapter returns. Adapters retain a cumulative byte budget for streaming output and apply any tightened cap to that budget. The runtime validates tool output schemas, emits a text representation alongside structured output and bounds the serialized normalized result before publication. `mcp-transport` enforces its own framing limits on complete messages.

Resource reads match normalized URIs with one declared variable per segment and validate decoded template values before adapter preparation. The pilot applies portable file-segment checks to those values. Prompt templates support escaped braces and bounded text substitution. Optional absent prompt arguments render as empty text. JSON Schema 2020-12 compilation stays offline and uses in-document references. The pilot rejects `x-mcp-header` on tool schema nodes.

Cancellation and timeout signal the execution token and discard pending work. `RuntimeError` reports a stable kind, code and correlation ID, with typed denial and effect metadata where applicable. A cancellation before execution reports no effect. Cancellation while executing reports an unknown effect unless the handler has already returned an effect. The runtime sends numeric progress only, with a cap of 32 updates, at least 50 ms between updates, strictly increasing values and no detached delivery after completion. The transport binds progress to the request token. M5, M7 and V2 own live adapter cleanup and persisted audit and trace sinks.

## Verification

Run from the worktree root with the pinned Rust toolchain. On Windows, load the F1 MSVC environment and use a dedicated target directory. [Windows setup](development.md#windows-prerequisites-and-local-setup)

```powershell
cargo fmt --all --check
cargo test --locked --workspace --exclude mcp-builder-desktop
cargo clippy --locked --workspace --exclude mcp-builder-desktop --all-targets -- -D warnings
cargo build --locked -p mcp-builder
node .github/ci/cli-smoke.mjs
```

The Windows non-desktop workspace passed 51 tests. Six runtime cases cover discovery, schema and target gating, output validation and limits, resource and prompt execution, caller-scoped pagination, per-call cancellation, bounded progress and a shortened deadline after a target recheck. The schema 1 fixture projects all four capability kinds and verifies that a REST operation comes from its configured method even with an empty grant list. Formatting, Clippy and CLI smoke passed. The stdio and loopback HTTP transports record independent wire results for both revisions. [Stdio results](stdio-sdk-probe.md#m3-transport-verification) · [HTTP results](loopback-http.md)

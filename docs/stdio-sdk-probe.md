# M3 stdio SDK probe

13 November 2026 · Windows x64 · [MCP transport contract](contracts.md#mcp-transport-and-execution) · [M3 wire cases](mcp-compatibility.md#wire-tests-and-release-evidence)

An independent Python client exchanged raw newline-delimited JSON-RPC with a disposable Rust subprocess. The fixture used `rmcp 3.9.0` with `server` and `transport-io`, Rust `1.99.1` and Tokio `1.54.1`. It used synthetic tools and no network or desktop app. The SDK remains the candidate for M3 and M4.

| Case | Observed behavior |
| --- | --- |
| Modern discovery and direct requests | `server/discover` returned both supported versions, server capabilities, `resultType: "complete"`, `ttlMs: 0` and private caching. Discovery left the connection open to legacy initialization. A direct `tools/list` selected modern behavior and returned the same result metadata. A later request missing inline metadata returned `-32602`. An unsupported version returned `-32022` with the requested version and the supported pair. |
| Legacy initialize and operation | An offer of either `2025-11-25` or `2025-03-26` selected `2025-11-25`. After `notifications/initialized`, `tools/list` used the legacy result shape and `ping` returned an empty object. The SDK also answered a modern `ping` before it selected a lifecycle, then rejected it with `-32601` once modern operation began. |
| Framing and errors | With `with_max_line_length(1_048_576)`, the SDK accepted a frame of exactly 1 MiB including LF, dropped the next frame at 1 MiB plus one byte and accepted the following CRLF frame. It dropped invalid JSON syntax. A JSON array returned `-32600` without an ID. A `tools/call` missing its name fell through to `on_custom_request` and returned `-32601` with the request ID. An override returned `-32602` for this call. |
| Output ceiling | The default transport emitted a 1,048,690-byte response despite the configured input bound. A transport wrapper counted the serialized frame and substituted a fixed 95-byte `-32603` reply with the original ID. |
| Cancellation and progress | Both revisions canceled the named handler through `RequestContext.ct` and suppressed its terminal reply. A sibling tool completed and a second sibling reached its own deadline. Unknown and repeated cancellation IDs left the connection usable. A handler could still emit progress after it observed cancellation. Checking the token before submitting progress prevented that emission in both profiles. |
| Shutdown and diagnostics | Idle EOF exited in under 20 ms. EOF during a pending tool took about five seconds with the SDK alone. A separate connection token canceled at EOF let the handler finish and the process exit in under 20 ms. The SDK logged a synthetic cancellation reason with tracing enabled. A hard filter on the stderr layer excluded it even with `RUST_LOG=rmcp=trace`. |

Use `ServerHandler` and `ServiceExt::serve` for the SDK lifecycle. Override `supported_protocol_versions()` with `V_2026_07_28` and `V_2025_11_25`. Set `ServerConfig::with_protocol_version` to `V_2025_11_25` for the initialize fallback. Keep the request version and metadata at the transport boundary. The runtime receives the trusted caller, deadline, cancellation and approved policy inputs. [SDK handler](https://github.com/modelcontextprotocol/rust-sdk/blob/rmcp-v3.9.0/crates/rmcp/src/handler/server.rs#L377-L469) · [SDK lifecycle](https://github.com/modelcontextprotocol/rust-sdk/blob/rmcp-v3.9.0/crates/rmcp/src/service/server.rs#L438-L655) · [Versioning and compatibility](https://modelcontextprotocol.io/specification/2026-07-28/basic/versioning)

Implement the production byte boundary as a bounded `Transport<RoleServer>` over newline-delimited frames. Preserve readable IDs and return fixed errors for malformed request envelopes. The project error contract calls for `-32700` on invalid JSON, which requires handling it before `AsyncRwTransport` drops the frame. Reject a modern pre-init ping before the SDK's legacy shortcut. Bound the fully encoded response, including framing and JSON-RPC overhead, before writing. Cancel connection-scoped work as soon as the reader observes EOF. [SDK framing](https://github.com/modelcontextprotocol/rust-sdk/blob/rmcp-v3.9.0/crates/rmcp/src/transport/async_rw.rs#L110-L327) · [SDK shutdown](https://github.com/modelcontextprotocol/rust-sdk/blob/rmcp-v3.9.0/crates/rmcp/src/service.rs#L1770-L1830) · [Modern stdio binding](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/stdio)

Associate progress with an active request and recheck its cancellation state when dequeuing it for stdout. The token check in the fixture guards progress submitted after cancellation. Production verification also needs the race where progress is already queued when cancellation arrives. Keep SDK request, response and notification tracing outside the stderr layer and emit the builder's sanitized diagnostics there. [SDK send and cancellation loop](https://github.com/modelcontextprotocol/rust-sdk/blob/rmcp-v3.9.0/crates/rmcp/src/service.rs#L1496-L1669) · [Progress rules](https://modelcontextprotocol.io/specification/2026-07-28/basic/patterns/progress)

## Reproduce on the probe worktree

Run from the managed `m3-stdio-sdk-probe` worktree. The disposable Rust fixture, its lock and the Python 3 client are in the ignored `target/m3-sdk-probe` directory there. The next harness should run these cases through the actual M3 server and add capability, schema, policy, cancellation races and legacy client fallback coverage.

```powershell
. (Join-Path $env:LOCALAPPDATA 'McpBuilderF1/env.ps1')
$env:CARGO_TARGET_DIR = 'D:/codex-build/mcp-builder-m3-sdk-probe'
$env:CARGO_INCREMENTAL = '0'
$env:CARGO_PROFILE_DEV_DEBUG = '0'
cargo build --locked --offline --manifest-path target/m3-sdk-probe/Cargo.toml
python.exe target/m3-sdk-probe/probe.py (Join-Path $env:CARGO_TARGET_DIR 'debug/m3-sdk-probe.exe')
```

## M3 transport verification

The `server-stdio` Cargo feature exposes `mcp_transport::stdio::serve_stdio` for the reviewed runtime and trusted local operator. It uses the same runtime for both revisions. Hosts apply `sdk_diagnostic_filter` to each tracing layer before serving. The local wire accepts up to 16 active requests per connection and reserves cancellation handling at that limit. Frames include their newline in the 1 MiB input and output limits. String request IDs and progress tokens are capped at 1 KiB so the writer can correlate bounded error replies.

```powershell
. (Join-Path $env:LOCALAPPDATA 'McpBuilderF1/env.ps1')
$env:CARGO_TARGET_DIR = Join-Path (Get-Location).Path 'target'
cargo test --locked -p mcp-transport --features server-stdio
cargo clippy --locked -p mcp-transport --all-targets --features server-stdio -- -D warnings
cargo build --locked -p mcp-transport --no-default-features --features server-stdio,server-http
```

The Windows tests run the actual transport and schema 1 runtime in a subprocess and drive raw JSON-RPC through its pipes. Six scenarios cover both versions, discovery and direct calls, tools and their schemas, concrete and expanded resource reads, prompts, policy-denial metadata, malformed and oversized frames, cancellation isolation, duplicate identifiers and progress tokens, a sibling timeout, and EOF with work in flight. They check stdout framing and scan stderr for a synthetic cancellation canary with SDK tracing requested. Three frame tests cover serialized output limits, queued and partially written frames during cancellation, and continued cancellation when request admission reaches its limit. [Wire harness](../crates/mcp-transport/tests/stdio_wire.rs) · [Frame tests](../crates/mcp-transport/src/stdio/frames/tests.rs)

The server-side legacy fallback is exercised through an older offered version and the legacy initialization path. The client-side auto probe against legacy-only error and silence fixtures, and the recognized-modern-error path, still need raw client-side evidence. M4 owns HTTP and the SDK pin decision. [Compatibility matrix](mcp-compatibility.md#wire-tests-and-release-evidence)

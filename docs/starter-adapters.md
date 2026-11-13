# M5 starter adapters

[Portable bindings](contracts.md#capability-and-binding-records) · [TM-006](security.md#tm-006-adapter-boundaries) · [Shared runtime](local-runtime.md)

M6 standalone generation and M7 managed preview can pass `builder_project::runtime_definition(&manifest)` and a separately reviewed `OperatorPolicy` to `Runtime::new`. Construct its `Arc<dyn Adapter>` with `StarterAdapter::new(manifest.document().project_id, builder_project::runtime_bindings(&manifest), local_bindings)`. Projection accepts a validated schema 1 manifest and resolves no local inputs. The transport and synthetic fixture entry points remain independent of the live adapters.

`LocalBindings` carries the reviewed root mappings, optional origin-scoped DER CA roots and an optional `SecretResolver`. The resolver receives the project ID, verified principal, capability ID, binding ID, original origin and secret reference. Its implementation owns registration or deployment-profile scoping and the explicitly selected source. Missing, unavailable and invalid credentials have fixed diagnostics. Portable documents containing a plaintext credential binding still open for editing. Live setup returns `credential.https_required`.

## REST profile

- The method, origin and path template come from the binding. Path inputs accept a nonempty string, number or boolean. The adapter encodes each input as a path segment, rejects ambiguous separators and traversal, and bounds the full path at 2 KiB. The optional body pointer selects a JSON value from the validated tool arguments.
- Route admission precedes DNS. The adapter validates every distinct address in a bounded DNS answer before connecting directly to a numeric socket, then verifies its peer and rechecks policy. The exact `localhost` name resolves directly to both loopback families and requires both peer ranges. A numeric origin lets an operator grant one family. HTTPS authenticates the configured hostname or IP on that same socket before resolving any credential. Each hop uses one HTTP/1.1 connection with no proxy, cookie store, decompression or automatic retry.
- Redirects require an exact operator-approved route and the same requested, approved and caller permissions as any other hop. The adapter follows at most four redirects for bodyless reads at the original origin and method. It rechecks the route before resolving each new hop. Redirects to a different origin, including a different port or scheme, are denied. Write requests have no retry or redirect replay. Their remote mutation outcome is unknown after send because a generic HTTP response cannot attest to commit.
- Success returns redacted UTF-8 text or JSON. A JSON object is structured tool content. Error bodies, response headers and native error strings are excluded from results and diagnostics. The adapter uses the permit's cumulative body cap and the runtime bounds the serialized MCP result. Header fields are capped at 64 and the response header block at 16 KiB. HEAD and no-content status responses return an empty text result. Actual encoded bodies are rejected.
- TLS uses Rustls with AWS-LC, the locked Mozilla CA bundle, and any reviewed extra CA for the exact configured origin. Chain, name and validity checks use the configured roots without network retrieval during verification. Public CA changes require updating the dependency and rebuilding. The adapter does not obtain online revocation data or inherit machine trust overrides. The MCP transport SDK has its separate TLS feature trials. [Mozilla root bundle](https://docs.rs/webpki-roots/1.0.11/webpki_roots/) · [CCADB data terms](https://www.ccadb.org/rootstores/usage#ccadb-data-usage-terms)

M6 should bundle the runtime adapter production sources and carry the `webpki-roots` CDLA-Permissive-2.0 license and CCADB data terms into redistribution review. The TLS test CA and disposable private key are repository-only fixtures and belong outside the generated project source and release packages.

## File-read profile

The host supplies a reviewed absolute path for each mapped `rootRef`. Setup pins the approved directory handle for the lifetime of the adapter. Only a caller-authorized, portable relative path reaches the native open. File results use bounded UTF-8 text. Cancellation discards the result and stops at the next bounded read boundary. A local filesystem operation already inside a system call may complete later. The adapter limits outstanding file and DNS workers to eight each and discards late results. The root and its contents must be controlled by the approved operating-system principal.

- The native path profile caps relative file paths and Windows mapped paths at 64 components.
- Windows uses local NTFS drive roots and exact stored names. It checks each handle for reparse or special attributes, refuses alternate streams and files with multiple links, and keeps handles without delete sharing through the read.
- Linux requires `openat2` and genuine procfs on local ext4 or tmpfs. It selects the entry with a metadata-only handle under the pinned root, refuses symlinks, nested mounts and casefolded or encrypted parent directories, and reopens the same verified single-link regular file for data.
- Unsupported operating systems and filesystem types fail closed for a mapped file root. [Windows relative open](https://learn.microsoft.com/windows/win32/api/winternl/nf-winternl-ntcreatefile) · [Linux pathname lookup](https://docs.kernel.org/filesystems/path-lookup.html)

## Verification

Windows 11 Pro 25H2 x64 (build 26200.9974) and Rust 1.99.1 passed the locked starter adapter, non-desktop workspace and transport feature suites. The starter tests exercise live HTTP and HTTPS through validated manifests and the shared registry, including TLS name and expiry checks, unknown and wrong-port trust, credential absence and echo redaction, route denial before DNS, mixed peer answers, no redirected writes, bounded output and cancellation. The file tests cover approved resource reads and rejection of out-of-root paths, case and 8.3 aliases, reparse points, unsupported path forms, hard links and rename attempts on local NTFS.

```powershell
cargo fmt --all --check
cargo test --locked -p mcp-starter-adapters
cargo test --locked --workspace --exclude mcp-builder-desktop
cargo clippy --locked --workspace --exclude mcp-builder-desktop --all-targets -- -D warnings
cargo test --locked -p mcp-transport --no-default-features --features server-stdio,server-http,client-stdio,client-http-rustls
cargo clippy --locked -p mcp-transport --no-default-features --features server-stdio,server-http,client-stdio,client-http-rustls --all-targets -- -D warnings
node .github/ci/sdk-probes.mjs
cargo build --locked -p mcp-builder
node .github/ci/cli-smoke.mjs
```

An independent Windows run at `d82eda0` called the REST adapter through `Runtime::new` with `LocalBindings::default()`. A grant for `GET /` at `https://example.com` used the default public peer policy and bundled CA roots. The call returned the expected `Example Domain` text. This live check used a temporary harness and stays outside the offline suite.

The isolated Linux native-file harness cross-checked GNU and ran as a static x64 musl executable in an offline QEMU guest with procfs. It exercised tmpfs and plain ext4 on Linux 5.15.167.4-microsoft-standard-WSL2. With the Ubuntu 24.04 kernel `6.8.0-147-generic`, all seven harness cases passed on casefold-enabled ext4. Ordinary file I/O resolved a differently cased alias in the fixture directory. The adapter refused the directory as a root and refused reads under either spelling. The tmpfs run passed the applicable checks and reported that its casefold fixture could not be set up. The integrated Linux runtime still needs verification. macOS file bindings currently fail closed. M7 and V2 still own managed credential sources and persisted audit and trace evidence.

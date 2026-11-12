# Local development

[Workspace architecture](architecture.md#components-and-workspace-boundaries) · [Shared contracts](contracts.md) · [Progress](../PROGRESS.md)

## F1 toolchain

F1 started from the reviewed Phase 0 integration at `1eb58a03d4c931741d4f96fab69f34f8fe3874bd`. The exact Rust toolchain is in `rust-toolchain.toml`. `.node-version` selects the primary Node release and `apps/desktop/package.json` pins npm, the Angular packages and the Tauri JavaScript packages. F1 generated `Cargo.lock` with `cargo generate-lockfile` and `apps/desktop/package-lock.json` with `npm install --package-lock-only --engine-strict`.

| Input | F1 version |
| --- | --- |
| Rust and Cargo | `rustc 1.99.1 (58cf8f9c3 2026-10-13)` and `cargo 1.99.1 (5f94df478 2026-08-27)`, host `x86_64-pc-windows-msvc`, with rustfmt and Clippy |
| Primary Node and npm | Node `24.22.3`, bundled npm `11.19.1` |
| Alternate Node and npm | Node `26.13.1`, bundled npm `11.20.0` |
| Angular and TypeScript | Angular framework and compiler `22.2.6`, Angular CLI and build package `22.2.7`, TypeScript `6.0.3`, Vitest `5.1.0` with jsdom `31.0.0` |
| Tauri | Rust crate `2.12.5`, build helper `2.7.4`, JavaScript API and CLI `2.12.4`, Wry `0.57.2` and Tao `0.37.2` |
| MCP SDK | `rmcp = "=3.9.0"` with default features disabled and explicit opt-in profiles in `mcp-transport` |

F1 kept the ADR-008 Rust, Node and Tauri crate candidates and selected the exact Angular, TypeScript and Tauri JavaScript patches above. The highest declared Rust requirement among external dependencies in the active Windows workspace build was `1.90`, from packages including Tauri and `tauri-build`. F1 built with `1.99.1`. [Toolchain decision](../DECISIONS.md#adr-008--toolchain-platforms-and-packaging)

### Windows prerequisites and local setup

Build in an x64 Visual Studio developer shell with the MSVC C++ build tools and Windows SDK available. The native app uses WebView2. [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) · [Target platforms](deployment.md#build-webview-and-signing-constraints)

The initial Windows 11 Pro 25H2 x64 session (build `26200.9974`) had Node `22.16.0`, npm `10.9.2` and WebView2 `157.0.4341.46`. Rust was absent from `PATH` and the standard user Cargo directory. `cl.exe`, `link.exe` and `vswhere.exe` were absent from `PATH`, and `C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe` was missing. A trivial `rustc --out-dir <local-tools> <local-tools>/link_probe.rs` returned exit `1` and `linker link.exe not found`. With compiler binaries and the Windows SDK configured, a retry reported `LNK1104: cannot open file 'msvcrt.lib'`. Adding the x64 CRT Store payload supplied the import library and the same probe compiled and ran.

F1 used process-local tool paths under `%LOCALAPPDATA%\McpBuilderF1`. It ran `rustup-init.exe -y --no-modify-path --default-host x86_64-pc-windows-msvc --default-toolchain 1.99.1 --profile minimal` with local `RUSTUP_HOME` and `CARGO_HOME`, then `rustup component add rustfmt clippy --toolchain 1.99.1`. Both Node Windows x64 ZIPs were checked against the corresponding release SHA-256 lists before extraction. The F1 shell loaded the local tools with `. (Join-Path $env:LOCALAPPDATA 'McpBuilderF1\env.ps1')`. [Rust installer and distributions](https://static.rust-lang.org/rustup/dist/x86_64-pc-windows-msvc/) · [Node 24 hashes](https://nodejs.org/dist/v24.22.3/SHASUMS256.txt) · [Node 26 hashes](https://nodejs.org/dist/v26.13.1/SHASUMS256.txt)

MSVC came from the Microsoft Visual Studio 2022 `17.14.45` catalog. F1 extracted the host-x64/target-x64 tools and English resources, CRT headers, x64 Desktop CRT and x64 Store CRT VSIX payloads. `cl.exe` and `link.exe` reported file version `14.44.35230.0`. The full catalog download differed from the channel's published hash. The individual VSIX payloads matched their catalog SHA-256 values, and Authenticode validation returned `Valid` for `cl.exe` and `link.exe`. The Windows SDK C++ and x64 NuGet packages were `10.0.26100.9457` with headers under `10.0.26100.0`. Their SHA-512 hashes matched the registry response headers and `rc.exe` passed Authenticode validation. F1 added the compiler and SDK binaries to `PATH`, compiler and SDK headers to `INCLUDE`, and x64 CRT, UCRT and UM libraries to `LIB` in the verification shell. Build tools, logs and target directories stayed outside Git. [Visual Studio release channel](https://aka.ms/vs/17/release/channel) · [Windows SDK C++](https://www.nuget.org/packages/Microsoft.Windows.SDK.CPP/10.0.26100.9457) · [Windows SDK C++ x64](https://www.nuget.org/packages/Microsoft.Windows.SDK.CPP.x64/10.0.26100.9457)

## Locked build and smoke

Run the Angular commands from `apps/desktop`. Use `npm.cmd` in Windows PowerShell.

```sh
node --version
npm --version
npm ci --engine-strict
npm run format:check
npm run build
npm run test
npm exec --offline -- tsc --noEmit -p tsconfig.spec.json
```

The Angular build writes `apps/desktop/ui/dist/browser`. The UI uses Angular's zoneless change detection, a Signal for the smoke state and strict TypeScript and template checking. The Rust serialization tests and Angular IPC mock tests consume the same fixture for the success and failure envelopes. [Shared smoke fixture](../crates/builder-app/tests/fixtures/workspace-smoke.json) · [Angular zoneless guide](https://angular.dev/guide/zoneless) · [Angular test runner](https://angular.dev/guide/testing)

Both Node and npm pairs passed the clean locked install, Prettier, production build, two Angular tests and explicit TypeScript check. The generated browser files had identical SHA-256 checksums and `package-lock.json` was unchanged. Both installs reported npm's advisory for four locked dependencies with install scripts outside an `allowScripts` list and exited `0`. Git attributes preserve LF in the lockfiles and the files checked by Prettier so that a Windows checkout also passes the formatter.

From the repository root, run the Rust checks after building Angular.

```sh
rustc -Vv
cargo -V
cargo fmt --all --check
cargo build --locked --workspace --features mcp-builder-desktop/custom-protocol
cargo test --locked --workspace --features mcp-builder-desktop/custom-protocol
cargo clippy --locked --workspace --all-targets --features mcp-builder-desktop/custom-protocol -- -D warnings
cargo build --locked --workspace
```

All six Rust commands exited `0` on Windows. The `custom-protocol` workspace build used a fresh target directory and embedded the Angular assets. The test command passed both Rust contract tests. The desktop-only command `npm run tauri -- build --debug --no-bundle -- --locked` also ran the Angular build hook and produced the debug executable on both Node and npm pairs. Run it from `apps/desktop`.

### CLI and native IPC

F1 built the CLI with `cargo build --locked -p mcp-builder` and ran the executable in PowerShell. `CARGO_TARGET_DIR` pointed at a fresh local target directory for the workspace build. Use `target` in place of `$env:CARGO_TARGET_DIR` when using Cargo's default output path.

```powershell
$cli = Join-Path $env:CARGO_TARGET_DIR 'debug/mcp-builder.exe'
& $cli --help
& $cli smoke
$env:RUST_LOG = 'builder_app=info'
& $cli smoke --json
& $cli --json smoke --contract-version 2
& $cli smoke --json --unused-fixture-flag
```

| Check | Observed result |
| --- | --- |
| CLI help and human smoke | Exit `0`. Help lists `smoke`, and the command prints `Workspace smoke passed (builder 0.1.0, contract 1).` |
| CLI JSON smoke | Exit `0`. Stdout contains one object with `version: 1`, `command: "smoke"`, `ok: true`, `data` and `diagnostics: []`. The UUID in `data.correlationId` matches `fields.correlation_id` on the JSON trace on stderr. |
| Unsupported contract version | Exit `2` with `ok: false`, `input.invalid`, field `/contractVersion` and `details.supportedVersions: [1]`. |
| Invalid CLI option | Exit `2` with `command: "cli"`, `cli.usage` and a help action. The diagnostic carries fixed text. |

F1 launched `debug/mcp-builder-desktop.exe` from the clean custom-protocol workspace build with `RUST_LOG=builder_app=info`. In the main local WebView2 window at `http://tauri.localhost/`, **Run workspace check** displayed **Workspace check passed.**, builder `0.1.0` and IPC contract `1`. The displayed correlation ID `04f36809-2bf7-4e10-814f-ac5aeaadf38a` matched the `Workspace smoke completed` event on stderr. The Angular button invokes `workspace_smoke` with `{request: {contractVersion: 1}}`. The Tauri application manifest grants that command to the `main` window through its one permission. [Tauri capabilities](https://v2.tauri.app/security/capabilities/)

On the primary Node and npm pair, `npm run tauri -- dev --no-watch -- --locked` started Angular at `127.0.0.1:1420` and rendered the development window in WebView2. A separate Windows `PickerHost.exe` window intercepted the automated click. F1 stopped the development executable, which returned `0xffffffff` to the CLI. The successful native IPC round trip above used the embedded build.

## Workspace dependencies

```text
CLI and Tauri desktop → builder-app
builder-app → builder-project, builder-generator, builder-playground
builder-generator → builder-project
builder-project → mcp-runtime
builder-playground → mcp-runtime, mcp-transport
mcp-transport → mcp-runtime, optional rmcp
mcp-runtime → mcp-policy
```

`builder-app` owns the shared smoke service, request and response DTOs, reply envelope and diagnostics. It emits `tracing` events with correlation IDs. Each shell installs its stderr subscriber and maps input and output at its boundary. The active `cargo tree --locked -p builder-app --target x86_64-pc-windows-msvc --prefix none` contained the shared crates, Serde, UUID and tracing dependencies. The default feature set of `mcp-transport` keeps SDK activation opt-in. F1 checked the active Windows workspace's dependency declarations with `cargo metadata --locked --format-version 1 --filter-platform x86_64-pc-windows-msvc --features mcp-builder-desktop/custom-protocol`.

## MCP SDK build probes

Run these commands from the repository root. F1 ran each feature selection separately on `x86_64-pc-windows-msvc` and checked the actual features with the inverted Cargo trees.

```powershell
$profiles = 'server-stdio', 'server-http', 'server-stdio,server-http', 'client-stdio', 'client-http-rustls', 'client-http-native-tls'
foreach ($profile in $profiles) {
    cargo build --locked -p mcp-transport --features $profile
    cargo tree --locked -p mcp-transport --target x86_64-pc-windows-msvc --features $profile -e features -i rmcp
}
cargo tree --locked -p mcp-transport --target x86_64-pc-windows-msvc --features client-http-rustls -e features -i reqwest
cargo tree --locked -p mcp-transport --target x86_64-pc-windows-msvc --features client-http-native-tls -e features -i reqwest
```

| Workspace features | Resolved `rmcp 3.9.0` features |
| --- | --- |
| `server-stdio` | `schemars`, `server`, `transport-async-rw`, `transport-io`, `uuid` |
| `server-http` | `base64`, `schemars`, `server`, `server-side-http`, `tower`, `transport-async-rw`, `transport-streamable-http-server`, `transport-streamable-http-server-session`, `transport-worker`, `uuid` |
| `server-stdio,server-http` | `server-http` feature set above plus `transport-io` |
| `client-stdio` | `client`, `transport-async-rw`, `transport-child-process` |
| `client-http-rustls` | `__reqwest`, `auth`, `base64`, `client`, `client-side-sse`, `reqwest`, `transport-streamable-http-client`, `transport-streamable-http-client-reqwest`, `transport-worker` |
| `client-http-native-tls` | `__reqwest`, `auth`, `base64`, `client`, `client-side-sse`, `reqwest-native-tls`, `transport-streamable-http-client`, `transport-streamable-http-client-reqwest`, `transport-worker` |

All six Windows probe builds exited `0`. Both HTTP client profiles resolved `reqwest 0.13.5`. The Rustls build selected `rustls 0.23.46`, `rustls-platform-verifier 0.7.1`, `aws-lc-rs 1.19.0` and `aws-lc-sys 0.47.0`. The AWS-LC build used its `CC` path with MSVC and emitted a NASM availability warning. The native TLS build selected `native-tls 0.2.18` and Windows `schannel 0.1.30`. Keep the two feature probes separate when verifying a backend. [SDK decision and wire gates](../DECISIONS.md#adr-002--dual-revision-mcp-and-sdk-boundary)

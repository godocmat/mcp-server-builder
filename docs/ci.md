# Foundation CI

13 November 2026 · F2 · [Workflow](../.github/workflows/foundation.yml) · [F1 local development](development.md) · [ADR-008](../DECISIONS.md#adr-008--toolchain-platforms-and-packaging)

## Hosted targets

GitHub's standard runner catalog lists these labels and architectures. The workflow logs the image revision, OS, Rust host and compiler, commit, lockfile SHA-256 values and native tool versions. Each target uses the native Node distribution and the Rust toolchain pinned in `rust-toolchain.toml`. [GitHub-hosted runner labels](https://docs.github.com/en/actions/how-tos/write-workflows/choose-where-workflows-run/choose-the-runner-for-a-job)

| Runner and Rust host | Native prerequisites and evidence |
| --- | --- |
| `windows-2025` · `x86_64-pc-windows-msvc` | Windows Server 2025. Use the image's Visual Studio 2022 x64 C++ tools and Windows SDK. Log the resolved `cl.exe`, `link.exe`, `rc.exe` and SDK version. Read the Evergreen WebView2 version from its registry entries and report a missing runtime in the logs. The native TLS probe expects Schannel. [Runner image](https://github.com/actions/runner-images/blob/main/images/windows/Windows2025-Readme.md) · [Developer PowerShell](https://learn.microsoft.com/en-us/visualstudio/ide/reference/command-prompt-powershell?view=vs-2022) · [WebView2 detection](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/distribution#detect-if-a-webview2-runtime-is-already-installed) |
| `macos-26` · `aarch64-apple-darwin` | Use the image's Xcode selection and SDK, and log the system WebKit version used by WKWebView. The native TLS probe expects Security Framework. [Arm64 runner image](https://github.com/actions/runner-images/blob/main/images/macos/macos-26-arm64-Readme.md) · [Tauri WebView versions](https://v2.tauri.app/reference/webview-versions/) |
| `ubuntu-24.04` · `x86_64-unknown-linux-gnu` | Install Tauri's Noble development packages, including WebKitGTK 4.1 and GTK 3 via its dependencies, plus OpenSSL development files and the Xvfb and D-Bus tools. Log package and glibc versions. The native TLS probe expects OpenSSL. [Runner image](https://github.com/actions/runner-images/blob/main/images/ubuntu/Ubuntu2404-Readme.md) · [Tauri system prerequisites](https://v2.tauri.app/start/prerequisites/) |

The Windows AWS-LC build can use upstream prebuilt NASM objects on x64 when NASM is unavailable. The Linux and macOS probes use their native compiler toolchains. [AWS-LC Windows requirements](https://aws.github.io/aws-lc-rs/requirements/windows.html) · [AWS-LC Linux requirements](https://aws.github.io/aws-lc-rs/requirements/linux.html)

## Reproduce the checks

Run from the repository root in PowerShell 7. If using F1's user-local Windows tools, override the target directory immediately after sourcing their environment. Each worktree then uses its own ignored `target/`. The F2 workflow uses `CARGO_INCREMENTAL=0` and `CARGO_PROFILE_DEV_DEBUG=0` to limit build outputs on hosted runners. The latter omits debug symbols while preserving debug assertions.

```powershell
. (Join-Path $env:LOCALAPPDATA 'McpBuilderF1/env.ps1')
$env:CARGO_TARGET_DIR = Join-Path (Get-Location).Path 'target'
```

With Node `24.22.3` and bundled npm `11.19.1` selected, run the following. Set `-Architecture arm64` on Apple silicon. The Angular script checks exact versions and architecture and runs `npm ci --engine-strict`, `npm run format:check`, `npm run build`, `npm run test:coverage` and `npm exec --offline -- tsc --noEmit -p tsconfig.spec.json`. Building Angular first supplies `ui/dist/browser` for Tauri to embed.

```powershell
./.github/ci/angular.ps1 -NodeVersion 24.22.3 -NpmVersion 11.19.1 -Architecture x64 -Coverage
cargo fmt --all --check
cargo build --locked --workspace --features mcp-builder-desktop/custom-protocol
cargo test --locked --workspace --features mcp-builder-desktop/custom-protocol
cargo clippy --locked --workspace --all-targets --features mcp-builder-desktop/custom-protocol -- -D warnings
cargo build --locked --workspace
node .github/ci/cli-smoke.mjs
node .github/ci/sdk-probes.mjs
cargo test --locked -p mcp-transport --no-default-features --features server-stdio
cargo clippy --locked -p mcp-transport --no-default-features --features server-stdio --all-targets -- -D warnings
cargo test --locked -p mcp-transport --no-default-features --features client-stdio
cargo clippy --locked -p mcp-transport --no-default-features --features client-stdio --all-targets -- -D warnings
cargo test --locked -p mcp-transport --no-default-features --features server-http
cargo clippy --locked -p mcp-transport --no-default-features --features server-http --all-targets -- -D warnings
cargo test --locked -p mcp-transport --no-default-features --features server-http,client-http-rustls --test http_wire http_client_auto
cargo clippy --locked -p mcp-transport --no-default-features --features server-http,client-http-rustls --test http_wire -- -D warnings
rustup component add llvm-tools-preview
./.github/ci/rust-coverage.ps1
```

The Rust and Angular tests read the same [versioned fixture](../crates/builder-app/tests/fixtures/workspace-smoke.json). The CLI check exercises help, version, human success and rejection, JSON success, unsupported version and invalid options. It checks exits `0` and `2`, stdout and stderr, JSON version and fields, and correlation IDs against the Rust trace. The SDK script runs six locked builds of `mcp-transport` with `--no-default-features` and exactly one selection per invocation. It checks the resolved features for server stdio, server HTTP, their combination, client stdio and the two HTTP clients. The Rustls and native TLS builds each assert their own reqwest feature and the absence of the other. [F1 SDK feature graphs](development.md#mcp-sdk-build-probes)

The M3 CI checks select `server-stdio` and `client-stdio` in separate invocations. The server harness exercises both MCP revisions through real stdin and stdout pipes. The client harness runs the SDK auto negotiation against a raw JSON-RPC peer over subprocess pipes, including its ten-second fallback after a silent discovery probe. The M4 step runs the independent socket suite with `server-http` and adds the Rustls client feature for the SDK Auto cases. The Auto cases use plain loopback HTTP. The native TLS build continues through the separate SDK probe. [Wire and client interoperability](testing.md#wire-and-client-interoperability) · [M4 socket verification](loopback-http.md#verification)

Switch to Node `26.13.1` and bundled npm `11.20.0`. The alternate Angular check runs the same install, format, build and type checks with the ordinary test command. The Tauri CLI repeats the production Angular build hook and produces an embedded debug binary.

```powershell
./.github/ci/angular.ps1 -NodeVersion 26.13.1 -NpmVersion 11.20.0 -Architecture x64
Push-Location apps/desktop
$npm = if ($IsWindows) { 'npm.cmd' } else { 'npm' }
& $npm run tauri -- build --debug --no-bundle --no-sign -- --locked
Pop-Location
git diff --exit-code -- Cargo.lock apps/desktop/package-lock.json
```

## Native desktop trial

Use the same locked Angular build and select the debug-only `ci-webdriver` Cargo feature. A short Node client drives WebdriverIO's pinned `tauri-plugin-wdio-webdriver` embedded provider over W3C WebDriver. The client allocates a loopback port and launches Tauri with a disposable WebView2 or XDG profile on the platforms that use those overrides. It clicks **Run workspace check** in the real system WebView, reads the rendered builder and contract versions, and matches the rendered UUID to the Rust trace. The embedded provider gives macOS a WKWebView path and avoids a dependency on the Windows image's Edge browser driver matching Evergreen WebView2. F2 trials the provider directly. M8 can assess the full WebdriverIO runner and service when the pilot flows are available. [Tauri WebDriver](https://v2.tauri.app/develop/tests/webdriver/) · [WebdriverIO embedded provider](https://webdriver.io/docs/desktop-testing/tauri/plugin-setup/#plugin-2-tauri-plugin-wdio-webdriver-optional---embedded-driver) · [GitHub runner's Edge driver discussion](https://github.com/actions/runner-images/issues/14738)

```powershell
cargo build --locked -p mcp-builder-desktop --features custom-protocol,ci-webdriver
if ($IsLinux) {
    xvfb-run -a dbus-run-session -- node .github/ci/native-smoke.mjs
} else {
    node .github/ci/native-smoke.mjs
}
```

The feature is opt-in, registration requires debug assertions and an explicit nonzero port, and the driver listens on `127.0.0.1`. The application rejects release-profile compilation with `ci-webdriver`. The foundation workflow records the native trial's raw outcome in the job summary and allows this step to fail while hosted display and driver compatibility are being evaluated. The installed workflows, OS dialogs, credentials and accessibility retain their native acceptance gates. [Native test plan](testing.md#gui-cli-and-standalone-runs)

## Coverage and signing

Each runner uploads `coverage-<runner>` with Rust and Angular LCOV files. The Rust script uses `cargo-llvm-cov 0.9.1` and the pinned toolchain's LLVM tools to run the locked workspace tests. It reports line totals per authored file, strips the trailing inline fixture test block from `crates/builder-app/src/smoke.rs` and uses repository-relative source paths. Keep future Rust test-only modules in files the collector can exclude, or record their ranges. The pinned Vitest V8 provider includes authored TypeScript under `ui/src` and excludes spec files. Angular LCOV paths are relative to `apps/desktop`. [Rust coverage exclusions](https://github.com/taiki-e/cargo-llvm-cov#exclude-file-from-coverage) · [Angular coverage options](https://angular.dev/guide/testing/code-coverage)

The LCOV reports give file and line data for the [component inventory](traceability.md#coverage-denominators). Map new files and feature or platform tests to that inventory as they land. For milestone totals, normalize paths to repository-relative names and merge executable and covered line sets across the same commit before applying the core and security-critical scopes. Foundation reports cover the default transport selection with embedded desktop assets and the Angular unit tests. Add instrumented SDK feature and native integration runs when those tests land. M10, R3 and R5 own the milestone thresholds of 80% core and 90% security-critical line coverage.

Foundation CI runs on pull requests, pushes to `master`, and manual dispatch. It uses `contents: read`, disables persisted checkout credentials and pins all Actions to full commit SHAs. The workflow syntax job verifies the `actionlint 1.7.12` archive SHA-256 before running it. Coverage reports are the workflow's only uploaded artifacts. Keep signing and notarization credentials in protected release environments with required reviewers and trusted refs. R2/R5 sign reviewed release inputs in that separate workflow and recompute checksums after signing. [GitHub Actions secure use](https://docs.github.com/en/actions/reference/security/secure-use) · [Packaging and signing plan](deployment.md#build-webview-and-signing-constraints)

## Observed local results

| Check | Windows 11 Pro 25H2 x64 · build `26200.9974` | Ubuntu 24.04.3 amd64 · Noble userspace in WSL2 |
| --- | --- | --- |
| Toolchain and native libraries | Rust/Cargo `1.99.1`, MSVC `14.44.35230.0`, SDK `10.0.26100.0`, Evergreen WebView2 `157.0.4341.46`. | Rust/Cargo `1.99.1`, WebKitGTK `2.52.6`, GTK `3.24.41`, OpenSSL `3.0.13`, glibc `2.39`. |
| Angular and Tauri CLI | Both Node/npm pairs passed the locked install, Prettier, production build, two IPC fixture tests and TypeScript. Browser asset SHA-256 values matched across the pairs. The alternate Tauri CLI debug build passed. | Both Node/npm pairs passed the locked install, Prettier, production build, two IPC fixture tests and TypeScript. The alternate Tauri CLI debug build passed. |
| Rust and CLI | Formatting, both locked workspace builds, both fixture tests, Clippy with warnings denied and all CLI assertions passed. | Same checks passed. |
| SDK profiles | Six locked builds and feature assertions passed. `reqwest 0.13.5` used AWS-LC for Rustls and Schannel for native TLS. | Six locked builds and feature assertions passed. Rustls used AWS-LC and native TLS used OpenSSL. |
| Native desktop | Embedded WebView2 at `http://tauri.localhost/` passed with IPC contract 1 and matched correlation ID `1eed4a8f-2bda-4b64-b863-a750dbda5dcf`. | Embedded WebKitGTK under Xvfb at `tauri://localhost` passed with IPC contract 1 and matched correlation ID `ea4a4890-4e5a-43bb-96f3-4e5ecfecaa52`. |
| Unit line reports | Rust `44/127` authored lines. Angular `9/11` lines. | Rust `44/127` authored lines. Angular `9/11` lines. |

`actionlint 1.7.12`, PowerShell parsing and `git diff --check` passed. On Windows the release feature guard returned Cargo exit `101` with the intended compiler error, and the ordinary desktop dependency graph omitted the WebDriver crate. Locked macOS dependency resolution selected `security-framework 3.7.0` for native TLS and `objc2-web-kit 0.3.2` for the desktop trial. That graph was resolved from Windows.

## First hosted run

This worktree has no Git remote. Use the first pull request run after the branch reaches GitHub. A push to `master` also runs the workflow, and manual dispatch becomes available once the workflow is on the default branch. Record the run URL, commit and image revisions for all three rows. Check both Node pairs, the target-specific graphs, the stdio subprocess tests, both LCOV files per runner and each native trial's reported outcome. Apple silicon compilation and WKWebView, Windows Server 2025 toolchain discovery and WebView2, Ubuntu runner provisioning, GitHub artifact upload and the actual Actions job results require that run. [GitHub workflow events](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows#workflow_dispatch)

F2 changes the root `Cargo.lock` for the optional native test provider and the desktop package lock for V8 coverage. When integrating M1 or M2, preserve the locked entries from both changes and rerun the workspace gates on the merged commit. M8/M10 own pilot GUI workflows and R2/R4 own packaged native evidence.

# Windows pilot verification

Use a clean Windows 11 x64 VM with a standard user, a local NTFS volume, and the locked toolchain for building a standalone export. Use a disposable fixture and project directory. Record the commit, installer digest and signature, Windows and WebView2 versions, and any failures or unavailable evidence. [Build the pilot installer](deployment.md#build-the-windows-pilot-installer) · [Acceptance requirements](traceability.md#local-mvp)

1. Install the pilot for the current user. Confirm that the start menu entry opens the desktop and that `mcp-builder.exe` is beside the desktop executable in the installation folder. In a new PowerShell window, invoke that copy with `--version`.
2. From **Projects**, choose **New project** and an empty local folder. In **Edit project**, configure a REST tool, file resource, resource template, prompt, their requested grants, and the saved catalog scenarios. Use the [schema 1 catalog fixture](../crates/builder-project/tests/fixtures/catalog-v1.json) as the reference for capability IDs, fields and assertions. Introduce an invalid field and record the validation message and affected field. Correct it, save, restart the desktop, and compare the reopened revision and capabilities.
3. In **Playground**, select the saved `known_item` fixture and the `catalog_read` grant. Review the launch and verify that cancelling review starts no worker. Review again, confirm the bundled executable, working directory, manifest revision and permissions, then approve and start. Exercise discovery, replay, the redacted trace, and explicit stop. For live checks use an isolated local upstream and temporary file roots, and record an allowed call, a denied operation with no target effect, a missing credential diagnostic, and absence of a synthetic credential from evidence.
4. Copy the manifest to a second local folder and validate it and replay the same saved fixture with the installed CLI. Repeat with the same reviewed inputs and compare both results. Check an invalid project, an omitted approval and a changed expectation with a new review. Record the versioned diagnostics and corresponding exits 2, 3 and 4. [CLI examples](local-cli.md#review-and-run-saved-fixtures)
5. In **Export**, choose a parent and a new folder name, review the source plan, and publish. Compare identical exports and verify that a reviewed regeneration preserves an edited project file. In a separate directory, run `cargo build --locked` and `cargo test --locked --workspace` on the export. Exercise it with an independent MCP client using both supported stdio revisions. [Standalone runtime configuration](standalone-generator.md#generate-and-build)
6. Change the saved project from a second process while a draft is open. Verify that the stale save reports a conflict and that the draft can be saved as a new project. Repeat a failed preview or export with corrected inputs and confirm that the last valid project or export still opens.
7. Run the core path by keyboard and with Windows Narrator. Record focus order and return from pickers, control names, errors, status announcements, and any blocked steps. Review the installation and removal with a saved project present. [Native and accessibility checks](testing.md#gui-cli-and-standalone-runs)

The headless checks can run from the same commit with the locked build tools. `standalone-smoke.mjs` validates the catalog, runs two Windows fixture replays, then builds and tests its exported project. Pass the full CLI path to test the packaged copy. The independent benchmark drives a release export through real stdio using a loopback REST fixture. It records 30 starts, 200 calls after 20 warmups, a 60-second idle working set and 10 stops per revision. The explicit stop after a cancelled managed call uses the native supervision checks. Run the budget comparison on the reference machine. [Coverage and performance gates](traceability.md#quality-measurement)

```powershell
node .github/ci/standalone-smoke.mjs $releaseCli
node .github/ci/stdio-pilot-benchmark.mjs $releaseCli $newReportPath
```

The Windows desktop IPC test can also verify worker packaging assumptions headlessly. Set `CARGO_TARGET_DIR` to a disposable absolute build directory, build the locked Angular assets, and run the commands below from the repository root. Tauri's mock web runtime issues the IPC commands, and the test starts the real CLI from beside its test executable. Use the same configuration for the two Cargo commands.

```powershell
cargo build --locked -p mcp-builder
Copy-Item -LiteralPath (Join-Path $env:CARGO_TARGET_DIR 'debug/mcp-builder.exe') -Destination (Join-Path $env:CARGO_TARGET_DIR 'debug/deps/mcp-builder.exe')
cargo test --locked -p mcp-builder-desktop --features custom-protocol reviewed_fixture_round_trips_through_desktop_ipc_and_real_stdio -- --ignored
```

## GitHub-hosted pilot checks

Run **Desktop release installers** from the repository's Actions tab and inspect the Windows job. The workflow provisions a disposable `windows-2025` runner, builds the unsigned NSIS installer and checks the installed CLI's digest against the build. It launches the installed desktop and uses Windows UI Automation to open the catalog, cancel and approve launch reviews, run the `known_item` fixture, check trace redaction and stop the bundled worker. The installed CLI validates and replays the catalog, generates a standalone Rust server and builds and tests it. Uninstall removes the app and preserves the portable project. After the checks pass, the workflow uploads `mcp-server-builder-<version>-windows-x64`. Its release ZIP contains the same tested installer bytes, the installer SHA-256, the source commit and the distribution notices. A separate `.zip.sha256` file verifies the ZIP. The installer is unsigned. [Desktop installer workflow](../.github/workflows/windows-pilot.yml)

The hosted image runs Windows Server 2025. The Windows 11 pilot still needs native folder picker, restart recovery and Narrator evidence under a standard user account.

On 18 November 2026, the hosted run at `b1c1861` passed the installed GUI with Evergreen WebView2 `157.0.4341.46` on Windows Server 2025. The same run passed two replays of all four saved scenarios, the locked standalone build and 23 tests. Uninstall preserved the portable manifest. The unsigned NSIS SHA-256 was `d1897be8589d8b3d8a359192ce01708d4f858337f6a27d6c739675e984e46797`. [Passing installed pilot run](https://github.com/godocmat/mcp-server-builder/actions/runs/44411591461)

The earlier run at `6ef0506` timed out creating an Edge WebDriver `156.0.4314.58` session for the installed release. Its CLI and uninstall checks passed. An earlier loopback debugging probe also received no response. GitHub-hosted Windows jobs run as administrators. WebView2 150 and later ignore user-scoped debugger overrides for elevated apps. Windows UI Automation drives the installed window through its accessibility controls. [Earlier installed run](https://github.com/godocmat/mcp-server-builder/actions/runs/44400179113) · [Earlier browser probe](https://github.com/godocmat/mcp-server-builder/actions/runs/44397321483) · [Microsoft WebView2 guidance](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/security#for-an-elevated-host-app-use-appropriate-override-flags)

Foundation CI passed its real WebView2 IPC smoke on the built Windows desktop, along with WKWebView and WebKitGTK on the other hosted runners. The separate Windows 11 evaluation guest reached Setup and was shut down before the pilot app was installed. [Foundation results](ci.md#first-hosted-run)

## Workstation evidence

The 18 November 2026 run used Windows 11 Pro x64 build 26200, local NTFS, Rust 1.99.1, Node 24.22.3 and npm 11.19.1. The unsigned NSIS package was built from the M8/M9 implementation at `1759d01`. Its SHA-256 was `a4c2b0c049484b7012568ee6a03a468c3d77c1dbd8e7732df1cd54c5fafc06eb`. The package contains the desktop and the CLI at its root. The desktop bytes match the release binary with Tauri's three-byte NSIS marker applied. The bundled CLI SHA-256 was `d1773f65c4e256e34cfba6fa40cd7f6fad70627d12f590220bd2670a92db89b2`, matching the release build and the CLI used for the catalog and standalone checks.

The initial headless Rust tests covered stdio and loopback HTTP, both protocol revisions, and the Windows launch and file boundaries. Windows LCOV counted 12,069 of 14,032 executable authored Rust lines under the workspace source directories. Angular V8 counted 716 of 828 TypeScript lines. A follow-up with additional regression tests and a separate standalone profile reached 90.13% core and 90.11% security-critical coverage. [Scoped coverage results](m10-security-coverage.md)

The independent benchmark used the generated release build and a local REST fixture on an AMD Ryzen 5 5600X workstation with 12 logical CPUs and about 32 GiB RAM. Each profile completed 30 fresh launches, 200 measured calls and 10 measured standalone stops with zero failures. The raw benchmark file records the fixture, manifest and server hashes with the per-attempt timings. [Performance method](traceability.md#performance-method-and-budgets)

| MCP revision | Discovery p95 | Call p95 | Standalone stop p95 | Idle working set |
| --- | ---: | ---: | ---: | ---: |
| 2026-07-28 | 121.04 ms | 1.20 ms | 30.77 ms | 20.83 MiB |
| 2025-11-25 | 89.44 ms | 1.30 ms | 31.91 ms | 10.49 MiB |

Clean Windows installation, the native GUI-to-CLI workflow, native recovery and Narrator evidence remain pending. Repeat the measurements on the 4 vCPU, 8 GiB reference machine and record explicit managed stop after cancellation there. The desktop application was not launched during workstation verification.

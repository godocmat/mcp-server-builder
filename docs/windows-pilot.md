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

# M10 Windows security coverage

The Windows pilot uses the line denominators in Q-01. The measurement covers the workspace, Angular and generated standalone on Windows x64. [Coverage requirements](traceability.md#coverage-denominators) · [Windows pilot](windows-pilot.md)

## Results

The 18 November 2026 run reached **90.13% core line coverage** against the 80% threshold and **90.11% security-critical line coverage** against the 90% threshold. Both thresholds apply to the combined denominator.

| Component | Core lines | Security-critical lines |
| --- | ---: | ---: |
| Overall | 13,686/15,185 (90.13%) | 13,646/15,143 (90.11%) |
| Rust | 12,938/14,357 (90.12%) | 12,907/14,326 (90.09%) |
| TypeScript | 748/828 (90.34%) | 739/817 (90.45%) |
| Policy | 1,142/1,231 | 1,142/1,231 |
| Runtime | 1,310/1,435 | 1,310/1,435 |
| Starter adapters | 993/1,096 | 993/1,096 |
| Transport | 1,244/1,342 | 1,244/1,342 |
| Project | 1,455/1,535 | 1,455/1,535 |
| Application | 1,197/1,345 | 1,166/1,314 |
| Generator | 886/965 | 886/965 |
| Playground | 2,815/3,202 | 2,815/3,202 |
| CLI | 665/777 | 665/777 |
| Desktop native | 950/1,109 | 950/1,109 |
| Desktop UI | 748/828 | 739/817 |
| Generated standalone shell | 281/320 | 281/320 |

Windows passed 162 workspace Rust tests, two opt-in native IPC tests, and twelve stdio harness cases. Angular passed all ten tests. The headless Tauri harness covered approval and revision binding, allowed and denied calls through the managed local worker, shutdown, export destination and source-plan binding, recovery isolation and credential input rejection. Angular covered expired review, denied launch, inert diagnostic rendering and permission cleanup. The generated shell passed its eight Rust tests and fifteen startup checks.

### Native and feature paths

These rows are also included in the component totals. The JSON report breaks out native and transport files. The workspace LCOV contains the build-script counters.

| Path or feature | Covered lines |
| --- | ---: |
| Desktop path picker and selection grants | 102/147 |
| Desktop permission build script | 5/5 |
| Private project library filesystem | 81/99 |
| Windows publication | 268/303 |
| Windows process supervision | 773/857 |
| Windows native file adapter | 256/275 |
| Loopback HTTP transport files | 519/554 |
| Stdio transport files | 469/523 |

Native picker completion, the WebView and installed application behavior require separate clean Windows VM evidence. The measured paths retain uncovered lines, including Windows publication and native handle errors, standalone initialization failure paths, and headless picker error and cancellation handling. The setup and results for the installed checks live with the pilot evidence. [Windows pilot](windows-pilot.md)

## Scope

The inventory selects whole authored modules, including helpers, generic error paths and unexecuted lines. Workspace Rust includes policy, runtime, transports, adapters, project and application services, generator, playground, CLI, native desktop commands, and the desktop permission build script. The standalone profile includes the three authored Rust files for configuration, handler assembly and stdio startup. The TypeScript profile includes editor and desktop review, credential entry, IPC, preview and export. The workspace smoke service, Angular bootstrap, smoke IPC and smoke component are in core only. [Line inventory](../.github/ci/m10-coverage.mjs)

Each executable line contributes once. A line covered by any supplied report for its profile counts as covered. The reporter omits test-only spans, test files and emitted copies of already inventoried runtime crates. It lists declaration-only files and authored HTML, the Tauri capability manifest and app configuration as structural evidence. Those assets have no authored line mappings. The only executable files without mappings in this profile are the Linux publication and file-adapter implementations and the unsupported publication fallback. They are outside the Windows pilot build.

## Reproduce the measurements

Use the locked Rust and Node tools, cargo-llvm-cov 0.9.1 and the LLVM tools component. Run from the repository root with fresh absolute scratch targets and the built Windows CLI from the tested source. Set `$releaseCli` to that CLI, `$workspaceTarget` to the workspace scratch target, `$standaloneTarget` to the standalone scratch target, and `$export` to a new folder with an existing parent outside the coverage targets. Start with Angular so the Tauri test harness can embed the built assets.

```powershell
./.github/ci/angular.ps1 -NodeVersion 24.22.3 -NpmVersion 11.19.1 -Architecture x64 -Coverage

$temp = New-Item -ItemType Directory -Path (Join-Path $workspaceTarget 'temp') -Force
$env:TEMP = $temp.FullName
$env:TMP = $temp.FullName
$env:CARGO_TARGET_DIR = $workspaceTarget
$features = 'mcp-builder-desktop/custom-protocol,mcp-transport/server-stdio,mcp-transport/server-http,mcp-transport/client-stdio,mcp-transport/client-http-rustls'
cargo llvm-cov --locked --workspace --features $features --no-report -- --test-threads=1
$instrumented = Join-Path $workspaceTarget 'llvm-cov-target'
Copy-Item -LiteralPath $releaseCli -Destination (Join-Path $instrumented 'debug/deps/mcp-builder.exe')
cargo llvm-cov --locked --workspace --features $features --no-report -- reviewed --ignored

$previousProfile = $env:LLVM_PROFILE_FILE
$env:CARGO_TARGET_DIR = $instrumented
$env:LLVM_PROFILE_FILE = Join-Path $instrumented 'm10-cli-smoke-%p-%m.profraw'
try {
    node .github/ci/cli-smoke.mjs
} finally {
    $env:CARGO_TARGET_DIR = $workspaceTarget
    $env:LLVM_PROFILE_FILE = $previousProfile
}
cargo llvm-cov report --include-build-script --lcov --output-path (Join-Path $workspaceTarget 'pilot-rust.lcov')
```

Build the generated server, run its stdio tests and check the actual executable with rejected and accepted operator settings. Its stdout must stay clear during startup refusal.

```powershell
$project = New-Item -ItemType Directory -Path (Join-Path $temp.FullName 'catalog-project')
Copy-Item -LiteralPath 'crates/builder-project/tests/fixtures/catalog-v1.json' -Destination (Join-Path $project.FullName 'mcp-project.json')
& $releaseCli generate $project.FullName --out $export
$env:CARGO_TARGET_DIR = $standaloneTarget
Push-Location -LiteralPath $export
try {
    cargo llvm-cov --locked -p mcp-standalone-server --no-report -- --test-threads=1
} finally { Pop-Location }

$instrumentedExport = Join-Path $standaloneTarget 'llvm-cov-target'
$env:LLVM_PROFILE_FILE = Join-Path $instrumentedExport 'm10-standalone-admission-%p-%m.profraw'
try {
    $server = Join-Path $instrumentedExport 'debug/mcp-standalone-server.exe'
    node .github/ci/m10-standalone-admission.mjs $server (Join-Path $export 'mcp-project.json')
} finally { $env:LLVM_PROFILE_FILE = $previousProfile }

$standaloneReport = Join-Path $standaloneTarget 'standalone-rust.lcov'
Push-Location -LiteralPath $export
try {
    cargo llvm-cov report --lcov --output-path $standaloneReport
} finally { Pop-Location }

$rustReport = Join-Path $workspaceTarget 'pilot-rust.lcov'
$angularReport = 'apps/desktop/coverage/desktop/lcov.info'
node .github/ci/m10-coverage.mjs --rust $rustReport --angular $angularReport --standalone $standaloneReport
```

Keep the reports and their source revision together. Each profile flag can be repeated to merge reports for the same source revision. Compare the report's `noCounters` list with the intended build and feature scope before interpreting the percentages.

# M10 Windows process and protocol checks

Run the measurements from the reviewed checkout on the 4 vCPU, 8 GiB Windows 11 x64 reference machine with local SSD storage. Use a local NTFS directory for build outputs and evidence. Record the Windows edition and build, CPU model and count, available RAM, storage, checkout commit, Rust and Node versions, and packaged CLI checksum. The benchmark builds use Rust 1.99.1, MSVC and the Windows SDK. The standalone and Inspector checks use Node 24.22.3. [Reference budgets](traceability.md#performance-method-and-budgets) · [Locked build prerequisites](deployment.md#build-webview-and-signing-constraints)

## Run the benchmarks

Set `$releaseCli` to the first-party `mcp-builder.exe` from the pilot installation. Run from the root of the checkout. Use a fresh evidence directory for each run.

```powershell
$evidence = (New-Item -ItemType Directory -Path (Join-Path $env:LOCALAPPDATA ('McpBuilderM10-' + [guid]::NewGuid()))).FullName
$env:CARGO_TARGET_DIR = Join-Path $evidence 'target'
$env:MCP_BUILDER_STOP_BENCHMARK_CLI = (Resolve-Path -LiteralPath $releaseCli).Path
$env:MCP_BUILDER_STOP_BENCHMARK_REPORT = Join-Path $evidence 'managed-stop.json'

$os = Get-CimInstance Win32_OperatingSystem
$machine = Get-CimInstance Win32_ComputerSystem
[ordered]@{
    commit = (git rev-parse HEAD).Trim()
    os = $os.Caption
    build = $os.BuildNumber
    cpu = @((Get-CimInstance Win32_Processor).Name)
    logicalProcessors = $machine.NumberOfLogicalProcessors
    memoryBytes = $machine.TotalPhysicalMemory
    filesystem = [IO.DriveInfo]::new($evidence).DriveFormat
    rustVersion = (rustc --version)
    nodeVersion = (node --version)
} | ConvertTo-Json -Depth 3 | Set-Content -LiteralPath (Join-Path $evidence 'machine.json')

cargo test --locked --release --target x86_64-pc-windows-msvc -p mcp-builder --test managed_playground m10_managed_stop_benchmark -- --ignored --nocapture
node .github/ci/stdio-pilot-benchmark.mjs $releaseCli (Join-Path $evidence 'standalone-stdio.json')
```

The managed check runs the real worker through the application service on MCP 2026-07-28 and 2025-11-25. For each revision it creates ten fresh sessions for a successful catalog call and ten for a cancelled call. The cancellation case waits for the fixture to receive the request, cancels without supplying a response, then requires a cancelled MCP result and a closed upstream socket before timing explicit stop. The timer covers `ManagedSession::stop` through the native job check. A successful stop requires a signaled worker and zero active processes in its Windows job. The benchmark independently checks the same worker with a retained process handle. The report includes every attempted run, the actual negotiated revision, request outcome, timings, observed worker exit, the job result and any failures. Each successful profile contains ten stop times of at most five seconds.

The standalone check generates the catalog and builds an isolated release server first. It then uses pipes and an independent JSON-RPC client to measure discovery, the approved item lookup, idle working set and normal stop. Installation and compilation happen before the recorded measurements. Record the guest resources and backing disk separately from the timing JSON. A workstation run supplies supporting evidence for the reference machine trial. [Standalone source](standalone-generator.md)

To measure in a guest with few installed tools, transfer the release test executable printed by `cargo test --no-run` with the managed benchmark's other Cargo flags. Transfer a reviewed standalone release executable built from the same exported runtime. Point `$managedBenchmark` and `$standalone` to those copies and keep the two benchmark environment variables set. The standalone script still generates the catalog using the guest CLI and records the transferred server checksum. Record the source commit and artifact digests before transfer. The standalone script needs Node and Git in the guest, and uses Windows PowerShell for working-set measurements.

```powershell
& $managedBenchmark m10_managed_stop_benchmark --ignored --nocapture
node .github/ci/stdio-pilot-benchmark.mjs $releaseCli (Join-Path $evidence 'standalone-stdio.json') $standalone
```

The native supervision test supplies a separate check with a deliberately live descendant that survives the exit of the first process. It verifies that the stop waits for the descendant to exit within five seconds.

```powershell
cargo test --locked --release --target x86_64-pc-windows-msvc -p builder-playground --test native_supervision stops_a_job_descendant_even_after_the_stdio_process_exits -- --exact
```

## Run the pinned Inspector smoke

Use the M4 Inspector test-tool `package.json` and `package-lock.json` in a disposable directory. The observed lock SHA-256 was `b8616cc862d792ad640cf3d6c700ff2c9a74e798febd2f10bc8ea93029ecf902`. Install with `npm ci --engine-strict --ignore-scripts --no-audit --no-fund` on Node 24.22.3. Set `$inspectorDirectory` to that install directory and `$standalone` to the release executable built by the standalone benchmark or another reviewed catalog export. [Pinned Inspector client profile](mcp-compatibility.md#client-matrix)

```powershell
$launcher = Join-Path $inspectorDirectory 'node_modules/@modelcontextprotocol/inspector/clients/launcher/build/index.js'
node .github/ci/standalone-inspector-smoke.mjs $launcher $standalone
```

The smoke starts Inspector's CLI with an isolated local profile. It asserts the negotiated revision and checks tool discovery and invocation, fixed and templated resources, and prompt discovery and rendering over real stdio. The fixture serves only on IPv4 loopback and records the two approved requests.

## Workstation results

On 18 November 2026, Windows 11 Pro x64 build 26200 on an AMD Ryzen 5 5600X with 12 logical processors, about 32 GiB RAM and local NTFS ran the managed benchmark with the pilot CLI from the unsigned installer built at `1759d01`. The worker SHA-256 was `d1773f65c4e256e34cfba6fa40cd7f6fad70627d12f590220bd2670a92db89b2`. All 40 attempts completed within five seconds, with the worker signaled and zero active processes in its job at each stop.

| Negotiated revision | Normal call stop p95 | Cancelled call stop p95 | Attempts per case |
| --- | ---: | ---: | ---: |
| 2026-07-28 | 25.534 ms | 24.434 ms | 10 |
| 2025-11-25 | 24.608 ms | 24.191 ms | 10 |

Inspector 2.15.0 with client and core 2.4.0 passed the full smoke on both revisions with Node 24.22.3. The isolated lock installation had SHA-256 `b8616cc862d792ad640cf3d6c700ff2c9a74e798febd2f10bc8ea93029ecf902`. The standalone release binary had SHA-256 `0d09e334fd6e33896a4f48fe35f04bd228fb395fe2a2f3cd3c8a2e6f0b8ea915`. The fixture observed one approved item lookup per revision.

That prebuilt standalone executable also passed the independent stdio benchmark using Node 24.22.3 and the Windows PowerShell working-set check. Each revision ran 30 starts, 200 measured calls after warm-up and 10 measured stops with zero failures.

| Negotiated revision | Discovery p95 | Call p95 | Standalone stop p95 | Idle working set |
| --- | ---: | ---: | ---: | ---: |
| 2026-07-28 | 15.792 ms | 1.030 ms | 6.204 ms | 10.383 MiB |
| 2025-11-25 | 16.212 ms | 1.128 ms | 3.966 ms | 10.438 MiB |

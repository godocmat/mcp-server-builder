# MCP Server Builder

Create a local Model Context Protocol (MCP) server with the Windows desktop editor or `mcp-builder` CLI. Define REST tools, read-only file resources and resource templates, and prompts. Review permissions, replay saved scenarios, and export an editable, standalone Rust server.

The builder and its first-party export code are free for personal and commercial use. All shipped features work without a product account, subscription, or license service. [Apache License 2.0](LICENSE) · [Notice](NOTICE)

## Install on Windows

Use Windows 11 x64 and a local NTFS folder for projects. The per-user installer includes both the desktop app and CLI. It uses Microsoft Edge WebView2 and can download Microsoft's bootstrapper if WebView2 is missing.

Download `mcp-server-builder-0.1.0-windows-x64.zip` and its companion `.zip.sha256`. Compare the published hash with the output below. Extract the verified ZIP and run `mcp-server-builder-0.1.0-windows-x64-setup.exe`. The installer is unsigned. The archive includes `SHA256SUMS.txt` with the installer's hash. [GitHub Releases](https://github.com/godocmat/mcp-server-builder/releases/latest)

```powershell
Get-FileHash -Algorithm SHA256 .\mcp-server-builder-0.1.0-windows-x64.zip
```

Open **MCP Server Builder** from Start. In PowerShell, set `$cli` to the bundled CLI path and check its version. If you chose a custom install folder, substitute that folder.

```powershell
$cli = Join-Path $env:LOCALAPPDATA 'MCP Server Builder\mcp-builder.exe'
& $cli --version
```

## Build the CLI from source

Use Git, rustup, Rust `1.99.1`, and an x64 Visual Studio developer shell with the MSVC C++ build tools and Windows SDK. The checkout pins Rust automatically through rustup. For the Windows x64 TLS dependency, the command below opts in to the locked AWS-LC crate's prebuilt NASM objects. [Install rustup](https://rustup.rs/) · [Windows build prerequisites](docs/development.md#windows-prerequisites-and-local-setup)

```powershell
git clone https://github.com/godocmat/mcp-server-builder.git
Set-Location .\mcp-server-builder
$env:AWS_LC_SYS_PREBUILT_NASM = '1'
cargo build --locked -p mcp-builder --target-dir target
$cli = (Resolve-Path -LiteralPath .\target\debug\mcp-builder.exe).Path
```

To build the desktop installer, also use PowerShell 7.3 or later, Node `24.22.3` and npm `11.19.1`. The packaging script builds the desktop and CLI from the same checkout. [Build the Windows installer](docs/deployment.md#build-the-windows-pilot-installer)

## Try the sample project

Run this in PowerShell with `$cli` set as above. It creates a fresh project directory and downloads the checked-in sample. If you already cloned the repository, you can copy `crates/builder-project/tests/fixtures/catalog-v1.json` to `$manifest` instead of downloading it.

```powershell
$sampleRoot = Join-Path $env:LOCALAPPDATA ("McpBuilderSample-" + [guid]::NewGuid().ToString('N'))
$projectDir = Join-Path $sampleRoot 'catalog'
$exportDir = Join-Path $sampleRoot 'standalone'
New-Item -Path $projectDir -ItemType Directory | Out-Null
$manifest = Join-Path $projectDir 'mcp-project.json'
$fixtureUrl = 'https://raw.githubusercontent.com/godocmat/mcp-server-builder/v0.1.0/crates/builder-project/tests/fixtures/catalog-v1.json'
Invoke-WebRequest -Uri $fixtureUrl -OutFile $manifest
& $cli project validate $projectDir
```

### Replay in the desktop app

1. Go to **Projects → Open folder** and select the catalog folder. Run `$projectDir` in PowerShell to see its path.
2. Open **Playground**, choose **Saved fixture** and **Known catalog item**, and select the `catalog_read` grant.
3. Choose **Review launch**. Inspect the executable, working directory, project revision and grant, then choose **Approve and start**.
4. Choose **Replay scenario**, check the result and trace, and select **Stop server** when done.

Saved fixtures use synthetic data. This replay requires no upstream connection or credentials. [Managed playground](docs/local-playground.md)

### Replay from the CLI

Prepare the same launch review and inspect the output before approving it.

```powershell
$selection = @($projectDir, '--scenario', 'known_item', '--grant', 'catalog_read')
$review = & $cli --json policy review launch @selection | ConvertFrom-Json
if ($LASTEXITCODE -ne 0 -or -not $review.ok) { throw 'Launch review failed.' }
$review.data | ConvertTo-Json -Depth 20
```

If the review matches what you intend to run, approve its digest with the same selection. The expected result is a passing `known_item` scenario.

```powershell
& $cli test run @selection --approve $review.data.digest
```

[CLI commands, JSON output, and exit codes](docs/local-cli.md)

### Export and build a standalone server

In the desktop app, choose **Export**, select a parent folder and enter a new folder name. Choose **Review export**, then **Export Rust source**. The CLI can preview and publish the same source layout into the new `$exportDir`.

```powershell
& $cli generate $projectDir --out $exportDir --preview
& $cli generate $projectDir --out $exportDir
if ($LASTEXITCODE -ne 0) { throw 'Source export failed.' }
```

From the exported project, use the same Rust and native linker prerequisites as the source CLI build. The emitted Cargo workspace includes tests that launch the exported stdio executable.

```powershell
Push-Location -LiteralPath $exportDir
try {
    $env:AWS_LC_SYS_PREBUILT_NASM = '1'
    cargo build --locked
    if ($LASTEXITCODE -ne 0) { throw 'Standalone build failed.' }
    cargo test --locked --workspace
} finally {
    Pop-Location
}
```

The generated README shows how to connect an MCP client and configure reviewed grants. The standalone runtime starts with an empty grant and prompt set by default. [Standalone generation and regeneration](docs/standalone-generator.md)

## Release scope

- The public installer targets Windows x64. Managed playground and CLI fixture replay use the Windows worker. GitHub's installed smoke exercised the desktop, CLI, and standalone export on Windows Server 2025. Clean Windows 11 standard-user, native folder picker, restart recovery, and Narrator checks are tracked separately. [Windows pilot evidence](docs/windows-pilot.md)
- The local builder and standalone servers use stdio with MCP `2026-07-28` and `2025-11-25`. The loopback Streamable HTTP code is exercised separately. Authenticated remote deployments are a roadmap proposal. [Protocol and transport notes](docs/mcp-compatibility.md)
- The Linux x64 CI job builds and tests standalone exports on Ubuntu 24.04. Export publication requires local NTFS on Windows or a supported filesystem with `openat2` and procfs on Linux. Additional desktop installers are roadmap proposals. [Export filesystem requirements](docs/standalone-generator.md#generate-and-build) · [Roadmap](PLAN.md)

See the development guide for locked build and test commands. Changes should preserve the shared CLI and desktop contracts and the existing project schema. [Development guide](docs/development.md) · [CI checks](docs/ci.md) · [Architecture](docs/architecture.md)

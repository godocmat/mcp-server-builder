# MCP Server Builder

Create a local Model Context Protocol (MCP) server with the desktop editor or `mcp-builder` CLI. Define REST tools, read-only file resources and resource templates, and prompts. Review permissions, replay saved scenarios, then export an editable Rust server and client configurations to launch it over stdio.

The builder and its first-party export code are free for personal and commercial use. The released features work without a product account, subscription, or license service. [Apache License 2.0](LICENSE) · [Notice](NOTICE)

## Install v0.3.0

Download your platform’s ZIP and its `.zip.sha256` sidecar from the release. Verify the ZIP before extracting it. `SHA256SUMS.txt` inside the archive records the installer’s hash. Each installer includes the desktop app and CLI. [GitHub release](https://github.com/godocmat/mcp-server-builder/releases/tag/v0.3.0)

| Platform | Download | Installer | Project filesystem |
| --- | --- | --- | --- |
| Windows 11 x64 | `mcp-server-builder-0.3.0-windows-x64.zip` | Windows per-user setup | Local NTFS |
| macOS 26 Apple silicon | `mcp-server-builder-0.3.0-macos-arm64.zip` | macOS disk image (`.dmg`) | Local APFS |
| Ubuntu 24.04 x64 | `mcp-server-builder-0.3.0-linux-x64.zip` | Debian package (`.deb`) | Local ext4 recommended |

### Windows

Run the following in the download directory and compare the result with the `.zip.sha256` file.

```powershell
Get-FileHash -Algorithm SHA256 .\mcp-server-builder-0.3.0-windows-x64.zip
```

Extract the ZIP, run `mcp-server-builder-0.3.0-windows-x64-setup.exe`, and open **MCP Server Builder** from Start. The installer is unsigned. It can download Microsoft’s WebView2 runtime if needed. If you chose a custom install directory, substitute it for the default below.

```powershell
$cli = Join-Path $env:LOCALAPPDATA 'MCP Server Builder\mcp-builder.exe'
& $cli --version
```

### macOS

Run `shasum -a 256 -c mcp-server-builder-0.3.0-macos-arm64.zip.sha256` beside the ZIP. Extract it, open the `.dmg`, and drag **MCP Server Builder** into **Applications**. The disk image is unsigned. The app is ad hoc signed and unnotarized. If Gatekeeper blocks the verified app, use Apple’s per-app **Open Anyway** procedure where your system allows it. [Apple’s instructions](https://support.apple.com/en-us/102445)

```bash
cli='/Applications/MCP Server Builder.app/Contents/MacOS/mcp-builder'
"$cli" --version
```

### Ubuntu

Run `sha256sum -c mcp-server-builder-0.3.0-linux-x64.zip.sha256` beside the ZIP, extract it, then use APT to install the package and its runtime dependencies. Open **MCP Server Builder** from your application menu.

```bash
sudo apt install ./mcp-server-builder-0.3.0-linux-x64.deb
cli=/usr/bin/mcp-builder
"$cli" --version
```

## Try the sample project

Use the CLI path from the installation steps. These commands download a small catalog project into a fresh directory. It includes a tool, a read-only resource, and a prompt with fixture responses.

**Windows PowerShell**

```powershell
$sampleRoot = Join-Path $env:LOCALAPPDATA ("McpBuilderSample-" + [guid]::NewGuid().ToString('N'))
$projectDir = Join-Path $sampleRoot 'catalog'
$exportDir = Join-Path $sampleRoot 'standalone'
New-Item -Path $projectDir -ItemType Directory | Out-Null
$manifest = Join-Path $projectDir 'mcp-project.json'
$fixtureUrl = 'https://raw.githubusercontent.com/godocmat/mcp-server-builder/v0.3.0/crates/builder-project/tests/fixtures/catalog-v1.json'
Invoke-WebRequest -Uri $fixtureUrl -OutFile $manifest
& $cli project validate $projectDir
```

**macOS and Ubuntu Bash**

```bash
sample_root=$(mktemp -d "$HOME/McpBuilderSample.XXXXXX")
project_dir="$sample_root/catalog"
export_dir="$sample_root/standalone"
fixture_url='https://raw.githubusercontent.com/godocmat/mcp-server-builder/v0.3.0/crates/builder-project/tests/fixtures/catalog-v1.json'
mkdir "$project_dir"
curl -fL "$fixture_url" -o "$project_dir/mcp-project.json"
"$cli" project validate "$project_dir"
```

### Replay in the desktop app

1. Choose **Projects → Open folder** and select the catalog folder printed by `$projectDir` in PowerShell or `printf '%s\n' "$project_dir"` in Bash.
2. Open **Playground**, choose **Saved fixture** and **Known catalog item**, and select the `catalog_read` grant.
3. Choose **Review launch**. Inspect the executable, working directory, project revision, and grant. Choose **Approve and start**.
4. Choose **Run saved fixture** and check the result. Open **Protocol trace** and choose **Refresh trace**, then select **Stop server**.

Saved fixture replay uses synthetic data and works without upstream connections or credentials. [Managed playground](docs/local-playground.md)

### Replay from the CLI

Inspect the launch review before approving its digest. Use the same scenario and grant for both commands. The expected result is a passing `known_item` scenario.

**Windows PowerShell**

```powershell
$selection = @($projectDir, '--scenario', 'known_item', '--grant', 'catalog_read')
$review = & $cli --json policy review launch @selection | ConvertFrom-Json
if ($LASTEXITCODE -ne 0 -or -not $review.ok) { throw 'Launch review failed.' }
$review.data | ConvertTo-Json -Depth 20
```

```powershell
& $cli test run @selection --approve $review.data.digest
```

**macOS and Ubuntu Bash**

```bash
selection=("$project_dir" --scenario known_item --grant catalog_read)
"$cli" --json policy review launch "${selection[@]}"
```

```bash
read -r -p 'Paste the reviewed data.digest value ' digest
"$cli" test run "${selection[@]}" --approve "$digest"
```

[CLI commands, JSON output, and exit codes](docs/local-cli.md)

### Export and build a standalone server

In the desktop app, open **Export**, select an existing parent folder, and enter a new folder name. Choose **Review export**, then **Export Rust source**. To use the CLI instead, preview the plan and publish to the fresh `standalone` folder.

**Windows PowerShell**

```powershell
& $cli generate $projectDir --out $exportDir --preview
& $cli generate $projectDir --out $exportDir
if ($LASTEXITCODE -ne 0) { throw 'Source export failed.' }
```

**macOS and Ubuntu Bash**

```bash
"$cli" generate "$project_dir" --out "$export_dir" --preview
"$cli" generate "$project_dir" --out "$export_dir"
```

With the Rust and native compiler prerequisites below, enter the exported directory and run `cargo build --locked` and `cargo test --locked --workspace`. The tests launch the exported stdio executable for both protocol revisions. The generated README describes connecting a client and enabling selected capabilities. The server starts with empty grant and prompt sets. [Standalone generation and regeneration](docs/standalone-generator.md)

### Export a client configuration

Use the executable you built from the standalone export and the `mcp-project.json` next to its source. To try the sample's `review_item` prompt, copy `runtime-config.example.json` to `runtime-config.local.json` in your sample directory. Review the manifest and prompt, set `manifestRevision` to the digest for the exported manifest from `generation.json`, set `approvedPromptIds` to `["review_item"]`, and leave `approvedGrantIds` empty. The generated README explains how to review the exact manifest digest and additional permissions. [Standalone runtime setup](crates/builder-generator/template/README.md#review-the-manifest-and-configure-access)

**Desktop.** In **Export → MCP client configuration**, select a client and enter a server name. Choose the server executable, exported manifest, and optional reviewed runtime config. Choose **Generate client configuration**, then choose **Copy client configuration** or **Save client configuration**.

**Windows PowerShell**

```powershell
$profileFile = Join-Path $sampleRoot 'mcp-catalog.mcp-client.json'
$server = Join-Path $exportDir 'target/debug/mcp-standalone-server.exe'
$manifest = Join-Path $exportDir 'mcp-project.json'
$runtime = Join-Path $sampleRoot 'runtime-config.local.json'
& $cli client export generic --name catalog --executable $server --manifest $manifest --runtime-config $runtime --out $profileFile
```

**macOS and Ubuntu Bash**

```bash
profile_file="$sample_root/mcp-catalog.mcp-client.json"
"$cli" client export generic --name catalog \
  --executable "$export_dir/target/debug/mcp-standalone-server" \
  --manifest "$export_dir/mcp-project.json" \
  --runtime-config "$sample_root/runtime-config.local.json" --out "$profile_file"
```

If Cargo built the executable in a different target directory, select that executable. Omit `--out` to print the configuration to stdout, or `--runtime-config` to launch with empty approvals. Every profile uses absolute paths and separate arguments. Re-export after moving the executable or input files. The builder writes only to a new file, and the snippet contains paths and arguments without credential values or a copy of the runtime settings. Supply any required runtime environment variables through your client.

| Client profile | Where to use it |
| --- | --- |
| `claude-code` | Merge the server entry into `mcpServers` in your project's `.mcp.json`. [Claude Code MCP guide](https://code.claude.com/docs/en/mcp#project-scope) |
| `codex` | Add the TOML table to `~/.codex/config.toml` or `.codex/config.toml` in a trusted project. [Codex MCP guide](https://learn.chatgpt.com/docs/extend/mcp?surface=cli#configure-with-configtoml) |
| `opencode` | Merge the entry into `mcp.servers` in your OpenCode V2 `opencode.json` or `opencode.jsonc`. [OpenCode V2 MCP guide](https://github.com/anomalyco/opencode/blob/v2.2.7/services/site/src/docs/content/mcp-servers.mdx#config) |
| `generic` | Load the file directly in MCP Inspector, or use its `command` and `args` fields in another stdio client. [MCP Inspector CLI guide](https://modelcontextprotocol.io/docs/2026-07-28/tools/inspector/cli#choosing-a-server) |

To confirm the reviewed sample prompt with MCP Inspector, use Node `24.22.3` and substitute the path to the generated file if needed. Run from the directory containing the profile. The result lists `review_item`. [Inspector protocol options](https://modelcontextprotocol.io/docs/2026-07-28/tools/inspector/protocol-eras)

```text
npx --yes @modelcontextprotocol/inspector@2.15.0 --cli --config ./mcp-catalog.mcp-client.json --server catalog --protocol-era modern --method prompts/list
```

## Build from source

The checkout and standalone export pin Rust `1.99.1` through rustup. Use an x64 Visual Studio developer shell with the C++ build tools and Windows SDK on Windows, Xcode Command Line Tools on macOS, or a C toolchain such as `build-essential` on Ubuntu. On Windows x64, install NASM or opt into AWS-LC’s locked prebuilt objects by setting `$env:AWS_LC_SYS_PREBUILT_NASM = '1'` before running Cargo. [Install rustup](https://rustup.rs/) · [Development prerequisites](docs/development.md)

```bash
git clone --branch v0.3.0 https://github.com/godocmat/mcp-server-builder.git
cd mcp-server-builder
cargo build --locked -p mcp-builder --target-dir target
```

The CLI is in `target/debug/mcp-builder` on macOS or Ubuntu and `target/debug/mcp-builder.exe` on Windows. Building the desktop installer also uses Node `24.22.3` and npm `11.19.1`, plus the Tauri dependencies for your platform. [Desktop packaging guide](docs/deployment.md) · [CI checks](docs/ci.md)

## Release scope

- The release covers Windows x64, macOS Apple silicon, and Ubuntu x64. Each package ships the local stdio playground and standalone Rust export with MCP `2026-07-28` and `2025-11-25`. [Protocol and transport](docs/mcp-compatibility.md)
- Choose a supported local filesystem for projects and export destinations. Linux export requires `openat2`, procfs, and a filesystem with exclusive directory rename. Managed file roots require local NTFS on Windows, APFS on macOS, or ext4 or tmpfs on Linux. Keep roots and destinations in folders you control. [Export requirements](docs/standalone-generator.md#generate-and-build) · [File-resource limits](docs/starter-adapters.md#file-read-profile)
- Authenticated remote hosting and broader platform coverage remain roadmap items. [Roadmap](PLAN.md)

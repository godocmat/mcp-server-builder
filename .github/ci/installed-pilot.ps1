#Requires -Version 7.3
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

if (!$IsWindows -or $env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted') {
    throw 'Run this installer smoke on a disposable GitHub-hosted Windows runner.'
}

$repo = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$release = Join-Path $env:CARGO_TARGET_DIR 'x86_64-pc-windows-msvc/release'
$packages = @(Get-ChildItem -LiteralPath (Join-Path $release 'bundle/nsis') -Filter '*-setup.exe' -File)
if ($packages.Count -ne 1) { throw 'Expected one fresh Windows pilot installer.' }
$installer = $packages[0].FullName
$checksum = (Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash.ToLowerInvariant()
$signature = (Get-AuthenticodeSignature -LiteralPath $installer).Status
"Installer SHA-256: $checksum"
"Installer signature: $signature"

$scratch = Join-Path $env:RUNNER_TEMP "mcp-builder-pilot-$env:GITHUB_RUN_ID-$env:GITHUB_RUN_ATTEMPT"
New-Item -ItemType Directory -Path $scratch | Out-Null
$install = Join-Path $scratch 'installed'
$project = Join-Path $scratch 'catalog'
New-Item -ItemType Directory -Path $project | Out-Null
$manifest = Join-Path $project 'mcp-project.json'
$catalog = Join-Path $repo 'crates/builder-project/tests/fixtures/catalog-v1.json'
$document = Get-Content -Raw -LiteralPath $catalog | ConvertFrom-Json -AsHashtable
$document.scenarios[0].fixtures[0].body['private'] = 'm10-installed-webview-synthetic-marker'
$document | ConvertTo-Json -Depth 60 | Set-Content -LiteralPath $manifest -Encoding utf8

# NSIS expects /D= as the last argument and accepts the full unquoted path.
$setup = Start-Process -FilePath $installer -ArgumentList "/S /D=$install" -WindowStyle Hidden -Wait -PassThru
if ($setup.ExitCode -ne 0) { throw "The pilot installer returned $($setup.ExitCode)." }
$cli = Join-Path $install 'mcp-builder.exe'
$desktop = Join-Path $install 'mcp-builder-desktop.exe'
$uninstall = Join-Path $install 'uninstall.exe'
if (!(Test-Path -LiteralPath $desktop -PathType Leaf)) { throw 'The desktop executable is missing.' }
if (!(Test-Path -LiteralPath $uninstall -PathType Leaf)) { throw 'The uninstaller is missing.' }
$cliHash = (Get-FileHash -LiteralPath $cli -Algorithm SHA256).Hash
if ($cliHash -ne (Get-FileHash -LiteralPath (Join-Path $release 'mcp-builder.exe') -Algorithm SHA256).Hash) {
    throw 'The installed CLI does not match the release build.'
}

try {
    & $cli --version
    # Register the portable catalog with the same profile the installed desktop opens.
    $registration = & $cli --json project open $project | ConvertFrom-Json
    if (!$registration.ok -or $registration.data.document.name -ne 'Catalog sample') {
        throw 'The installed CLI could not register the reference catalog.'
    }
    & (Join-Path $PSScriptRoot 'installed-windows-uia.ps1') -Desktop $desktop
    node (Join-Path $PSScriptRoot 'standalone-smoke.mjs') $cli
} finally {
    $removal = Start-Process -FilePath $uninstall -ArgumentList '/S' -WindowStyle Hidden -Wait -PassThru
    if ($removal.ExitCode -ne 0 -or (Test-Path -LiteralPath $desktop) -or !(Test-Path -LiteralPath $manifest)) {
        throw 'The installed pilot could not be removed while retaining the portable project.'
    }
}

"Installed pilot passed on $env:RUNNER_OS at commit $env:GITHUB_SHA. Unsigned installer SHA-256 $checksum." |
    Add-Content -LiteralPath $env:GITHUB_STEP_SUMMARY
"installer-sha256=$checksum" | Add-Content -LiteralPath $env:GITHUB_OUTPUT -Encoding utf8

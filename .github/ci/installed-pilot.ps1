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
function Invoke-PilotWebViewPolicy([string] $Policy, [string] $Value, [scriptblock] $Test) {
    $registry = [Microsoft.Win32.RegistryKey]::OpenBaseKey('LocalMachine', 'Registry64')
    $path = "SOFTWARE\Policies\Microsoft\Edge\WebView2\$Policy"
    $name = Split-Path -Leaf $desktop
    $key = $null
    $applied = $false
    try {
        $key = $registry.OpenSubKey($path, $true)
        $created = $null -eq $key
        if ($created) { $key = $registry.CreateSubKey($path) }
        $hadValue = $key.GetValueNames() -contains $name
        if ($hadValue) {
            $previous = $key.GetValue($name, $null, 'DoNotExpandEnvironmentNames')
            $kind = $key.GetValueKind($name)
        }
        $key.SetValue($name, $Value, 'String')
        $applied = $true
        & $Test
    } finally {
        try {
            if ($key) {
                try {
                    if ($applied -and $hadValue) { $key.SetValue($name, $previous, $kind) }
                    if ($applied -and !$hadValue) { $key.DeleteValue($name, $false) }
                    $removeKey = $created -and $key.ValueCount -eq 0 -and $key.SubKeyCount -eq 0
                } finally {
                    $key.Dispose()
                }
                if ($removeKey) { $registry.DeleteSubKey($path, $false) }
            }
        } finally {
            $registry.Dispose()
        }
    }
}

try {
    foreach ($binary in @($cli, $desktop)) {
        $built = Join-Path $release (Split-Path -Leaf $binary)
        if ((Get-FileHash -LiteralPath $binary).Hash -ne (Get-FileHash -LiteralPath $built).Hash) {
            throw "The installed $(Split-Path -Leaf $binary) does not match the release build."
        }
    }
    & $cli --version
    # Register the portable catalog with the same profile the installed desktop opens.
    $registration = & $cli --json project open $project | ConvertFrom-Json
    if (!$registration.ok -or $registration.data.document.name -ne 'Catalog sample') {
        throw 'The installed CLI could not register the reference catalog.'
    }
    $groups = whoami.exe /groups /fo csv /nh | Out-String
    $integrity = [regex]::Match($groups, 'S-1-16-\d+').Value
    Write-Host "Installed desktop launch integrity SID: $integrity"
    $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
    $listener.Start()
    try { $port = $listener.LocalEndpoint.Port } finally { $listener.Stop() }

    # Elevated WebView2 hosts ignore environment and user policy overrides.
    # The hosted VM applies these settings to the installed executable during this test only.
    Invoke-PilotWebViewPolicy 'UserDataFolder' (Join-Path $scratch 'webview') {
        Invoke-PilotWebViewPolicy 'AdditionalBrowserArguments' "--remote-debugging-address=127.0.0.1 --remote-debugging-port=$port" {
            node (Join-Path $PSScriptRoot 'installed-webview.mjs') $desktop $port
        }
    }
    node (Join-Path $PSScriptRoot 'standalone-smoke.mjs') $cli
} finally {
    $removal = Start-Process -FilePath $uninstall -ArgumentList '/S' -WindowStyle Hidden -Wait -PassThru
    if ($removal.ExitCode -ne 0 -or (Test-Path -LiteralPath $desktop) -or !(Test-Path -LiteralPath $manifest)) {
        throw 'The installed pilot could not be removed while retaining the portable project.'
    }
}

"Installed pilot passed on $env:RUNNER_OS at commit $env:GITHUB_SHA. Unsigned installer SHA-256 $checksum." |
    Add-Content -LiteralPath $env:GITHUB_STEP_SUMMARY

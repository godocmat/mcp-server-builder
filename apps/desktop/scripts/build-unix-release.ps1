#Requires -Version 7.3
[CmdletBinding()]
param([string] $TargetDirectory)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

$architecture = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
if ($IsMacOS -and $architecture -eq 'Arm64') {
    $triple = 'aarch64-apple-darwin'
    $bundle = 'dmg'
} elseif ($IsLinux -and $architecture -eq 'X64') {
    $triple = 'x86_64-unknown-linux-gnu'
    $bundle = 'deb'
} else {
    throw 'Use a native Apple silicon macOS or Linux x64 build host.'
}

$desktop = Split-Path -Parent $PSScriptRoot
$repository = [IO.Path]::GetFullPath((Join-Path $desktop '../..'))
$previousTarget = $env:CARGO_TARGET_DIR
if (!$TargetDirectory) {
    $TargetDirectory = if ($previousTarget) { $previousTarget } else { 'target' }
}
$target = [IO.Path]::GetFullPath($TargetDirectory, $repository)
$staging = Join-Path $desktop 'src-tauri/binaries'

Push-Location -LiteralPath $repository
try {
    $env:CARGO_TARGET_DIR = $target
    cargo build --locked --release --target $triple -p mcp-builder
    $worker = Join-Path $target "$triple/release/mcp-builder"
    New-Item -ItemType Directory -Path $staging -Force | Out-Null
    Copy-Item -LiteralPath $worker -Destination (Join-Path $staging "mcp-builder-$triple")
    chmod 0755 (Join-Path $staging "mcp-builder-$triple")

    Set-Location -LiteralPath $desktop
    $config = Join-Path $desktop 'src-tauri/tauri.unix-release.conf.json'
    npm run tauri -- build --config $config --target $triple --bundles $bundle -- --locked
} finally {
    $env:CARGO_TARGET_DIR = $previousTarget
    Pop-Location
}

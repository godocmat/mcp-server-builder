#Requires -Version 7.3
[CmdletBinding()]
param([string] $TargetDirectory)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

if (!$IsWindows -or [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture -ne 'X64') {
    throw 'The pilot installer requires a Windows x64 build host.'
}

$desktop = Split-Path -Parent $PSScriptRoot
$repository = [System.IO.Path]::GetFullPath((Join-Path $desktop '../..'))
$triple = 'x86_64-pc-windows-msvc'
$priorTargetDirectory = $env:CARGO_TARGET_DIR
if (!$TargetDirectory) {
    $TargetDirectory = if ($priorTargetDirectory) { $priorTargetDirectory } else { 'target' }
}
$target = [System.IO.Path]::GetFullPath($TargetDirectory, $repository)
$staging = Join-Path $desktop 'src-tauri/binaries'

Push-Location -LiteralPath $repository
try {
    $env:CARGO_TARGET_DIR = $target
    cargo build --locked --release --target $triple -p mcp-builder
    $worker = Join-Path $target "$triple/release/mcp-builder.exe"
    New-Item -ItemType Directory -Path $staging -Force | Out-Null
    Copy-Item -LiteralPath $worker -Destination (Join-Path $staging "mcp-builder-$triple.exe") -Force

    Set-Location -LiteralPath $desktop
    $config = Join-Path $desktop 'src-tauri/tauri.pilot.conf.json'
    npm.cmd run tauri -- build --config $config --target $triple --no-sign -- --locked
}
finally {
    $env:CARGO_TARGET_DIR = $priorTargetDirectory
    Pop-Location
}

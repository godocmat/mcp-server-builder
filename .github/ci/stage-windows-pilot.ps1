#Requires -Version 7.3
[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [ValidatePattern('^[0-9a-f]{64}$')]
    [string] $VerifiedInstallerSha256
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$triple = 'x86_64-pc-windows-msvc'
$target = if ($env:CARGO_TARGET_DIR) {
    [IO.Path]::GetFullPath($env:CARGO_TARGET_DIR, $repo)
} else {
    Join-Path $repo 'target'
}
$packages = @(Get-ChildItem -LiteralPath (Join-Path $target "$triple/release/bundle/nsis") -Filter '*-setup.exe' -File)
if ($packages.Count -ne 1) { throw 'Expected one Windows pilot installer.' }
$installer = $packages[0].FullName
$installerHash = (Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash.ToLowerInvariant()
if ($installerHash -ne $VerifiedInstallerSha256) {
    throw 'The release installer differs from the installer used in the installed pilot.'
}
if ((Get-AuthenticodeSignature -LiteralPath $installer).Status -ne 'NotSigned') {
    throw 'The installer does not match the unsigned pilot distribution profile.'
}

# Include build dependencies since generated code and native libraries can enter the binaries.
Push-Location -LiteralPath $repo
try {
    $metadata = cargo metadata --locked --format-version 1 --filter-platform $triple --features mcp-builder-desktop/custom-protocol | ConvertFrom-Json -AsHashtable
    $rustRoot = rustc --print sysroot
    $commit = git rev-parse --verify HEAD
} finally {
    Pop-Location
}
$packagesById = @{}
foreach ($package in $metadata.packages) {
    $packagesById[$package.id] = $package
}
$nodesById = @{}
foreach ($node in $metadata.resolve.nodes) {
    $nodesById[$node.id] = $node
}
$roots = @($metadata.workspace_members | Where-Object {
    $packagesById[$_].name -in @('mcp-builder', 'mcp-builder-desktop')
})
if ($roots.Count -ne 2) { throw 'Both release executables must be present in the Cargo workspace.' }
$versions = @($roots | ForEach-Object { $packagesById[$_].version } | Select-Object -Unique)
if ($versions.Count -ne 1) { throw 'The desktop and CLI versions must match.' }

$visited = [Collections.Generic.HashSet[string]]::new()
$pending = [Collections.Generic.Queue[string]]::new()
foreach ($id in $roots) { $pending.Enqueue($id) }
while ($pending.Count -gt 0) {
    $id = $pending.Dequeue()
    if (!$visited.Add($id)) { continue }
    foreach ($dependency in $nodesById[$id].deps) {
        if ($dependency.dep_kinds | Where-Object { $_.kind -ne 'dev' }) {
            $pending.Enqueue($dependency.pkg)
        }
    }
}

$name = "mcp-server-builder-$($versions[0])-windows-x64"
$payload = (New-Item -ItemType Directory -Path (Join-Path $target 'windows-pilot-payload')).FullName
$output = (New-Item -ItemType Directory -Path (Join-Path $target 'windows-pilot-release')).FullName
$installerName = "$name-setup.exe"
$stagedInstaller = Join-Path $payload $installerName
Copy-Item -LiteralPath $installer -Destination $stagedInstaller
if ((Get-FileHash -LiteralPath $stagedInstaller -Algorithm SHA256).Hash -ne $installerHash) {
    throw 'The staged installer hash changed during the copy.'
}
"$installerHash  $installerName" | Set-Content -LiteralPath (Join-Path $payload 'SHA256SUMS.txt') -Encoding utf8
@("Commit: $commit", "Installer: unsigned Windows x64 NSIS", "Target: $triple") |
    Set-Content -LiteralPath (Join-Path $payload 'BUILD-INFO.txt') -Encoding utf8
Copy-Item -LiteralPath (Join-Path $repo 'LICENSE'),(Join-Path $repo 'NOTICE') -Destination $payload

$licenses = (New-Item -ItemType Directory -Path (Join-Path $payload 'licenses')).FullName
$noticeSources = Join-Path $PSScriptRoot 'windows-pilot-licenses'
Copy-Item -LiteralPath (Join-Path $noticeSources 'THIRD-PARTY-NOTICES.md') -Destination $payload
Copy-Item -LiteralPath (Join-Path $noticeSources 'installer') -Destination $licenses -Recurse
Copy-Item -LiteralPath (Join-Path $repo 'apps/desktop/ui/dist/3rdpartylicenses.txt') -Destination (Join-Path $licenses 'angular-bundle.txt')
$rustDocs = Join-Path $rustRoot 'share/doc/rust'
$rustLicenses = (New-Item -ItemType Directory -Path (Join-Path $licenses 'rust')).FullName
Copy-Item -LiteralPath (Join-Path $rustDocs 'COPYRIGHT-library.html'),(Join-Path $rustDocs 'licenses') -Destination $rustLicenses -Recurse
$nsis = Join-Path $env:LOCALAPPDATA 'tauri/NSIS'
Copy-Item -LiteralPath (Join-Path $nsis 'COPYING') -Destination (Join-Path $licenses 'installer/nsis-3.13-COPYING.txt')

$crates = @($visited | ForEach-Object { $packagesById[$_] } | Where-Object source | Sort-Object name,version)
$registryPackages = @{}
foreach ($package in $crates) {
    $registryPackages["$($package.name)@$($package.version)"] = Split-Path -Parent $package.manifest_path
}

# These exact registry releases omit terms provided by their upstream project.
$fallbacks = @{
    'alloc-stdlib@0.3.0' = Join-Path $noticeSources 'rust/alloc-stdlib-BSD-3-Clause.txt'
    'jsonschema-regex@0.61.2' = Join-Path $registryPackages['jsonschema@0.61.2'] 'LICENSE'
    'jsonschema-value@0.61.2' = Join-Path $registryPackages['jsonschema@0.61.2'] 'LICENSE'
    'selectors@0.38.0' = Join-Path $registryPackages['cssparser@0.37.0'] 'LICENSE'
    'tauri-plugin@2.7.2' = Join-Path $noticeSources 'rust/tauri-plugin-MIT.txt'
    'uuid-simd@0.8.0' = Join-Path $noticeSources 'rust/simd-MIT.txt'
    'vsimd@0.8.0' = Join-Path $noticeSources 'rust/simd-MIT.txt'
    'webview2-com@0.39.1' = Join-Path $noticeSources 'rust/webview2-rs-MIT.txt'
    'webview2-com-macros@0.8.1' = Join-Path $noticeSources 'rust/webview2-rs-MIT.txt'
    'webview2-com-sys@0.39.1' = Join-Path $noticeSources 'rust/webview2-rs-MIT.txt'
}
$inventory = foreach ($package in $crates) {
    if (!$package.source.StartsWith('registry+')) { throw "Unexpected crate source for $($package.id)." }
    $key = "$($package.name)@$($package.version)"
    $source = $registryPackages[$key]
    $destination = Join-Path $licenses "cargo/$key"
    $licenseFiles = @(Get-ChildItem -LiteralPath $source -File -Recurse | Where-Object {
        $_.Name -match '^(LICENSE|LICENCE|COPYING|COPYRIGHT|NOTICE|UNLICENSE)([._-]|$)' -or
        [IO.Path]::GetRelativePath($source, $_.FullName) -match '(^|[\\/])(licenses|notices)[\\/]'
    } | ForEach-Object { $_.FullName })
    if ($package.license_file) {
        $declared = [IO.Path]::GetFullPath($package.license_file, $source)
        $relative = [IO.Path]::GetRelativePath($source, $declared)
        if ($relative -match '^\.\.([\\/]|$)' -or [IO.Path]::IsPathRooted($relative)) {
            throw "The declared license for $key is outside its crate."
        }
        $licenseFiles += $declared
    }
    if (!$licenseFiles) {
        if (!$fallbacks.ContainsKey($key)) { throw "Missing license text for $key." }
        New-Item -ItemType Directory -Path $destination -Force | Out-Null
        Copy-Item -LiteralPath $fallbacks[$key] -Destination (Join-Path $destination 'LICENSE')
    } else {
        foreach ($file in ($licenseFiles | Sort-Object -Unique)) {
            $relative = [IO.Path]::GetRelativePath($source, $file)
            $copy = Join-Path $destination $relative
            New-Item -ItemType Directory -Path (Split-Path -Parent $copy) -Force | Out-Null
            Copy-Item -LiteralPath $file -Destination $copy
        }
    }
    $download = "https://static.crates.io/crates/$($package.name)/$($package.name)-$($package.version).crate"
    "$($package.name) $($package.version) | $($package.license ?? 'See the license file') | $download"
}
@('Locked Windows Cargo normal and build dependencies', '', 'Package | Terms | Exact source archive') + $inventory |
    Set-Content -LiteralPath (Join-Path $licenses 'cargo-inventory.txt') -Encoding utf8

$archive = Join-Path $output "$name.zip"
[IO.Compression.ZipFile]::CreateFromDirectory($payload, $archive)
$archiveHash = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
"$archiveHash  $name.zip" | Set-Content -LiteralPath "$archive.sha256" -Encoding utf8
"Release archive SHA-256: $archiveHash"
if ($env:GITHUB_OUTPUT) {
    "artifact-name=$name" | Add-Content -LiteralPath $env:GITHUB_OUTPUT -Encoding utf8
}

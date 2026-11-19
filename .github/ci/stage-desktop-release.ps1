#Requires -Version 7.3
[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [ValidateSet('x86_64-pc-windows-msvc', 'aarch64-apple-darwin', 'x86_64-unknown-linux-gnu')]
    [string] $TargetTriple,
    [Parameter(Mandatory)]
    [ValidatePattern('^[0-9a-f]{64}$')]
    [string] $VerifiedInstallerSha256
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$layout = switch ($TargetTriple) {
    'x86_64-pc-windows-msvc' {
        @{ platform = 'windows-x64'; bundle = 'nsis'; suffix = '-setup.exe'; signature = 'Unsigned Windows x64 NSIS' }
    }
    'aarch64-apple-darwin' {
        @{ platform = 'macos-arm64'; bundle = 'dmg'; suffix = '.dmg'; signature = 'Unsigned DMG with an ad hoc signed, unnotarized app' }
    }
    'x86_64-unknown-linux-gnu' {
        @{ platform = 'linux-x64'; bundle = 'deb'; suffix = '.deb'; signature = 'Unsigned Ubuntu amd64 DEB' }
    }
}
$target = if ($env:CARGO_TARGET_DIR) {
    [IO.Path]::GetFullPath($env:CARGO_TARGET_DIR, $repo)
} else {
    Join-Path $repo 'target'
}
$packages = @(Get-ChildItem -LiteralPath (Join-Path $target "$TargetTriple/release/bundle/$($layout.bundle)") -Filter "*$($layout.suffix)" -File)
if ($packages.Count -ne 1) { throw "Expected one fresh $($layout.platform) installer." }
$installer = $packages[0].FullName
$installerHash = (Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash.ToLowerInvariant()
if ($installerHash -ne $VerifiedInstallerSha256) {
    throw 'The release installer differs from the installer used in the installed smoke.'
}
if ($TargetTriple -eq 'x86_64-pc-windows-msvc' -and (Get-AuthenticodeSignature -LiteralPath $installer).Status -ne 'NotSigned') {
    throw 'The Windows installer is expected to be unsigned.'
}

# Include build dependencies since generated code and native libraries can enter the binaries.
Push-Location -LiteralPath $repo
try {
    $metadata = cargo metadata --locked --format-version 1 --filter-platform $TargetTriple --features mcp-builder-desktop/custom-protocol | ConvertFrom-Json -AsHashtable
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

$name = "mcp-server-builder-$($versions[0])-$($layout.platform)"
$payload = (New-Item -ItemType Directory -Path (Join-Path $target 'desktop-release-payload')).FullName
$output = (New-Item -ItemType Directory -Path (Join-Path $target 'desktop-release')).FullName
$installerName = "$name$($layout.suffix)"
$stagedInstaller = Join-Path $payload $installerName
Copy-Item -LiteralPath $installer -Destination $stagedInstaller
if ((Get-FileHash -LiteralPath $stagedInstaller -Algorithm SHA256).Hash -ne $installerHash) {
    throw 'The staged installer hash changed during the copy.'
}
"$installerHash  $installerName" | Set-Content -LiteralPath (Join-Path $payload 'SHA256SUMS.txt') -Encoding utf8
@("Commit: $commit", "Version: $($versions[0])", "Target: $TargetTriple", "Installer: $($layout.signature)") |
    Set-Content -LiteralPath (Join-Path $payload 'BUILD-INFO.txt') -Encoding utf8
Copy-Item -LiteralPath (Join-Path $repo 'LICENSE'),(Join-Path $repo 'NOTICE') -Destination $payload

$licenses = (New-Item -ItemType Directory -Path (Join-Path $payload 'licenses')).FullName
$noticeSources = Join-Path $PSScriptRoot 'desktop-release-licenses'
Copy-Item -LiteralPath (Join-Path $noticeSources 'THIRD-PARTY-NOTICES.md') -Destination $payload
if ($TargetTriple -eq 'x86_64-pc-windows-msvc') {
    Copy-Item -LiteralPath (Join-Path $noticeSources 'WINDOWS-INSTALLER-NOTICES.md') -Destination $payload
    Copy-Item -LiteralPath (Join-Path $noticeSources 'installer') -Destination $licenses -Recurse
    $nsis = Join-Path $env:LOCALAPPDATA 'tauri/NSIS'
    Copy-Item -LiteralPath (Join-Path $nsis 'COPYING') -Destination (Join-Path $licenses 'installer/nsis-3.13-COPYING.txt')
}
Copy-Item -LiteralPath (Join-Path $repo 'apps/desktop/ui/dist/3rdpartylicenses.txt') -Destination (Join-Path $licenses 'angular-bundle.txt')
$rustDocs = Join-Path $rustRoot 'share/doc/rust'
$rustLicenses = (New-Item -ItemType Directory -Path (Join-Path $licenses 'rust')).FullName
Copy-Item -LiteralPath (Join-Path $rustDocs 'COPYRIGHT-library.html'),(Join-Path $rustDocs 'licenses') -Destination $rustLicenses -Recurse
$crates = @($visited | ForEach-Object { $packagesById[$_] } | Where-Object source | Sort-Object name,version)
$registryPackages = @{}
foreach ($package in $crates) {
    $registryPackages["$($package.name)@$($package.version)"] = Split-Path -Parent $package.manifest_path
}

# These exact registry releases omit terms provided by their upstream project.
$fallbacks = @{
    'alloc-stdlib@0.3.0' = Join-Path $noticeSources 'rust/alloc-stdlib-BSD-3-Clause.txt'
    'dlopen2@0.8.2' = Join-Path $noticeSources 'rust/dlopen2-MIT.txt'
    'dlopen2_derive@0.4.3' = Join-Path $noticeSources 'rust/dlopen2-MIT.txt'
    'tauri-plugin@2.7.2' = Join-Path $noticeSources 'rust/tauri-plugin-MIT.txt'
    'uuid-simd@0.8.0' = Join-Path $noticeSources 'rust/simd-MIT.txt'
    'vsimd@0.8.0' = Join-Path $noticeSources 'rust/simd-MIT.txt'
    'webview2-com@0.39.1' = Join-Path $noticeSources 'rust/webview2-rs-MIT.txt'
    'webview2-com-macros@0.8.1' = Join-Path $noticeSources 'rust/webview2-rs-MIT.txt'
    'webview2-com-sys@0.39.1' = Join-Path $noticeSources 'rust/webview2-rs-MIT.txt'
}
# These objc2 workspace revisions use the same upstream licensing notice.
$objc2Terms = @(
    (Join-Path $noticeSources 'rust/objc2-TERMS.md'),
    (Join-Path $noticeSources 'rust/objc2-MIT.txt')
)
foreach ($key in @(
    'block2@0.6.2', 'dispatch2@0.3.1', 'objc2@0.6.5', 'objc2-encode@4.1.0',
    'objc2-exception-helper@0.1.1', 'objc2-foundation@0.3.2', 'objc2-app-kit@0.3.2',
    'objc2-core-foundation@0.3.2', 'objc2-core-graphics@0.3.2', 'objc2-io-surface@0.3.2',
    'objc2-quartz-core@0.3.2', 'objc2-web-kit@0.3.2'
)) {
    $fallbacks[$key] = $objc2Terms
}

$sharedLicense = @{
    'jsonschema-regex@0.61.2' = @('jsonschema@0.61.2', 'LICENSE')
    'jsonschema-value@0.61.2' = @('jsonschema@0.61.2', 'LICENSE')
    'libappindicator-sys@0.9.0' = @('libappindicator@0.9.0', 'LICENSE-MIT')
    'selectors@0.38.0' = @('cssparser@0.37.0', 'LICENSE')
}

$inventory = foreach ($package in $crates) {
    if (!$package.source.StartsWith('registry+')) { throw "Unexpected crate source for $($package.id)." }
    $key = "$($package.name)@$($package.version)"
    $source = $registryPackages[$key]
    $destination = Join-Path $licenses "cargo/$key"
    $licenseFiles = @(Get-ChildItem -LiteralPath $source -File -Recurse | Where-Object {
        $_.Extension -ne '.rs' -and (
            $_.Name -match '^(LICENSE|LICENCE|COPYING|COPYRIGHT|NOTICE|UNLICENSE)([._-]|$)' -or
            [IO.Path]::GetRelativePath($source, $_.FullName) -match '(^|[\\/])(licenses|notices)[\\/]'
        )
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
        $fallback = if ($fallbacks.ContainsKey($key)) {
            $fallbacks[$key]
        } elseif ($sharedLicense.ContainsKey($key)) {
            $sourcePackage, $licenseName = $sharedLicense[$key]
            Join-Path $registryPackages[$sourcePackage] $licenseName
        } else {
            throw "Missing license text for $key."
        }
        New-Item -ItemType Directory -Path $destination -Force | Out-Null
        Copy-Item -LiteralPath $fallback -Destination $destination
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
@("Locked Cargo normal and build dependencies for $TargetTriple", '', 'Package | Terms | Exact source archive') + $inventory |
    Set-Content -LiteralPath (Join-Path $licenses 'cargo-inventory.txt') -Encoding utf8

$archive = Join-Path $output "$name.zip"
[IO.Compression.ZipFile]::CreateFromDirectory($payload, $archive)
$archiveHash = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
"$archiveHash  $name.zip" | Set-Content -LiteralPath "$archive.sha256" -Encoding utf8
"Release archive SHA-256: $archiveHash"
if ($env:GITHUB_OUTPUT) {
    "artifact-name=$name" | Add-Content -LiteralPath $env:GITHUB_OUTPUT -Encoding utf8
}

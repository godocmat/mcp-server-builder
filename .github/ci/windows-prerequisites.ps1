$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
$install = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (!$install) { throw 'Visual Studio x64 C++ build tools are missing' }

# Let the installed developer shell resolve the compiler and SDK together.
$developerShell = Join-Path $install 'Common7/Tools/Launch-VsDevShell.ps1'
& $developerShell -Arch amd64 -HostArch amd64 -SkipAutomaticLocation
$tools = Get-Command cl.exe,link.exe,rc.exe -CommandType Application
foreach ($tool in $tools) {
    "$($tool.Name) $((Get-Item -LiteralPath $tool.Source).VersionInfo.FileVersion) ($($tool.Source))"
}
if ($env:GITHUB_PATH) {
    $tools.Source | Split-Path -Parent | Select-Object -Unique | Add-Content -LiteralPath $env:GITHUB_PATH -Encoding utf8
    foreach ($name in 'INCLUDE', 'LIB', 'LIBPATH', 'VCToolsInstallDir', 'WindowsSdkDir', 'WindowsSDKVersion', 'VSCMD_ARG_TGT_ARCH') {
        "$name=$([Environment]::GetEnvironmentVariable($name))" | Add-Content -LiteralPath $env:GITHUB_ENV -Encoding utf8
    }
}
if ($env:RUNNER_ENVIRONMENT -eq 'github-hosted') {
    # The runner's user TEMP can use a DOS short-name alias. Use the explicit
    # runner workspace volume for tests that verify physical NTFS paths.
    $nativeTemp = New-Item -ItemType Directory -Path (Join-Path $env:RUNNER_TEMP 'mcp-builder-native-temp')
    "Test temporary directory: $($nativeTemp.FullName)"
    "TEMP=$($nativeTemp.FullName)", "TMP=$($nativeTemp.FullName)" |
        Add-Content -LiteralPath $env:GITHUB_ENV -Encoding utf8
}
"Windows SDK: $env:WindowsSDKVersion"

$webviewId = '{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
$keys = @(
    "HKLM:/SOFTWARE/WOW6432Node/Microsoft/EdgeUpdate/Clients/$webviewId"
    "HKCU:/Software/Microsoft/EdgeUpdate/Clients/$webviewId"
)
$versions = foreach ($key in $keys) {
    $version = Get-ItemPropertyValue -LiteralPath $key -Name pv -ErrorAction SilentlyContinue
    if ($version -and $version -ne '0.0.0.0') { $version }
}
if ($versions) {
    "Evergreen WebView2: $($versions -join ', ')"
} else {
    Write-Warning 'Evergreen WebView2 Runtime was not found in the registry'
}

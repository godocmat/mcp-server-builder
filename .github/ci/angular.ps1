param(
    [Parameter(Mandatory)][string] $NodeVersion,
    [Parameter(Mandatory)][string] $NpmVersion,
    [Parameter(Mandatory)][string] $Architecture,
    [switch] $Coverage
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$npm = if ($IsWindows) { 'npm.cmd' } else { 'npm' }

Push-Location (Join-Path $PSScriptRoot '../../apps/desktop')
try {
    $actualNode = node --version
    $actualNpm = & $npm --version
    $actualArch = node -p process.arch
    "Node $actualNode, npm $actualNpm, architecture $actualArch"
    if ($actualNode -ne "v$NodeVersion" -or $actualNpm -ne $NpmVersion -or $actualArch -ne $Architecture) {
        throw "Expected Node v$NodeVersion, npm $NpmVersion and architecture $Architecture"
    }
    & $npm ci --engine-strict
    & $npm run format:check
    & $npm run build
    if ($Coverage) {
        & $npm run test:coverage
        if (!(Test-Path -LiteralPath coverage/desktop/lcov.info -PathType Leaf)) {
            throw 'Angular coverage report is missing'
        }
    } else {
        & $npm run test
    }
    & $npm exec --offline -- tsc --noEmit -p tsconfig.spec.json
} finally {
    Pop-Location
}

#Requires -Version 7.3
param([Parameter(Mandatory)][string] $Desktop)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
if (!$IsWindows -or $env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted') {
    throw 'Run the installed GUI smoke on a disposable GitHub-hosted Windows runner.'
}

$binary = Get-Item -LiteralPath $Desktop
$framework = Join-Path $env:SystemRoot 'Microsoft.NET/Framework64/v4.0.30319'
$compiler = Join-Path $framework 'csc.exe'
$helper = Join-Path $env:RUNNER_TEMP "mcp-installed-uia-$env:GITHUB_RUN_ID-$env:GITHUB_RUN_ATTEMPT.exe"
$source = Join-Path $PSScriptRoot 'installed-windows-uia.cs'
$references = @('UIAutomationClient.dll', 'UIAutomationTypes.dll', 'WindowsBase.dll') |
    ForEach-Object { '/reference:' + (Join-Path $framework "WPF/$_") }
& $compiler /nologo /warnaserror+ "/out:$helper" @references $source

$app = $null
$test = $null
try {
    $app = Start-Process -FilePath $binary.FullName -WorkingDirectory $binary.DirectoryName -PassThru
    Write-Host "Installed app PID $($app.Id), session $($app.SessionId); interactive runner: $([Environment]::UserInteractive)"
    $test = Start-Process -FilePath $helper -ArgumentList "$($app.Id)" -NoNewWindow -PassThru
    if (!$test.WaitForExit(240000)) { throw 'The installed UI Automation test exceeded four minutes.' }
    if ($test.ExitCode -ne 0) { throw "The installed UI Automation test exited with code $($test.ExitCode)." }
} finally {
    if ($test -and !$test.HasExited) {
        $test.Kill()
        [void] $test.WaitForExit(5000)
    }
    if ($app -and !$app.HasExited) {
        if (!$app.CloseMainWindow() -or !$app.WaitForExit(5000)) {
            if (!$app.HasExited) { $app.Kill($true) }
            [void] $app.WaitForExit(5000)
        }
    }
    Remove-Item -LiteralPath $helper -ErrorAction SilentlyContinue
}

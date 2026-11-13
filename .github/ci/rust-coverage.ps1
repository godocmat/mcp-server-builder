$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

Push-Location (Join-Path $PSScriptRoot '../..')
try {
    cargo llvm-cov --locked --workspace --features mcp-builder-desktop/custom-protocol --lcov --output-path target/rust-raw.lcov

    # Rust's fixture tests are inline. LCOV still counts their own source lines.
    # Exclude this exact block from the authored production line report.
    $fixtureSource = 'crates/builder-app/src/smoke.rs'
    $source = @(Get-Content -LiteralPath $fixtureSource -ErrorAction SilentlyContinue)
    $testStart = [Array]::IndexOf($source, 'mod tests {')
    $testEnd = -1
    if ($testStart -gt 0 -and $source[$testStart - 1] -ceq '#[cfg(test)]') {
        $testEnd = [Array]::IndexOf($source, '}', $testStart + 1)
        if ($testEnd -lt 0) { throw 'Update the coverage exclusion for the inline smoke fixture tests' }
    }

    $file = ''
    $covered = 0
    $total = 0
    $workspaceCovered = 0
    $workspaceTotal = 0
    $root = (Get-Location).Path.Replace('\', '/') + '/'
    $report = foreach ($line in Get-Content -LiteralPath target/rust-raw.lcov) {
        if ($line -match '^SF:(.+)$') {
            $path = $Matches[1].Replace('\', '/')
            $file = if ($path.StartsWith($root, [StringComparison]::OrdinalIgnoreCase)) {
                $path.Substring($root.Length)
            } else {
                $path
            }
            $covered = 0
            $total = 0
            "SF:$file"
        } elseif ($line -match '^DA:(\d+),(\d+)') {
            $sourceLine = [int]$Matches[1] - 1
            if ($file -eq $fixtureSource -and $sourceLine -ge $testStart -and $sourceLine -le $testEnd) { continue }
            $total++
            if ($Matches[2] -ne '0') { $covered++ }
            $line
        } elseif ($line -eq 'end_of_record') {
            "LF:$total"
            "LH:$covered"
            $line
            $workspaceCovered += $covered
            $workspaceTotal += $total
            Write-Host "$file ${covered}/${total} lines"
        } elseif ($line -eq 'TN:') {
            $line
        }
    }
    if ($workspaceTotal -eq 0) { throw 'Rust coverage report has no authored lines' }
    $report | Set-Content -LiteralPath target/rust.lcov -Encoding utf8
    "Rust authored lines covered: $workspaceCovered/$workspaceTotal"
} finally {
    Pop-Location
}

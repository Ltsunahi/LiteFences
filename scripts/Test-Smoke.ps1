[CmdletBinding()]
param([ValidateRange(1, 100)] [int]$Iterations = 5, [string]$ExecutablePath)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path $PSScriptRoot -Parent
$executable = if ($ExecutablePath) { [IO.Path]::GetFullPath($ExecutablePath) } else { Join-Path $projectRoot 'target\release\litefences.exe' }
if (-not (Test-Path -LiteralPath $executable)) { throw 'Build release before running the smoke test.' }
$results = @()
for ($iteration = 1; $iteration -le $Iterations; $iteration++) {
    $watch = [Diagnostics.Stopwatch]::StartNew()
    $process = Start-Process -FilePath $executable -ArgumentList '--smoke-test' -WindowStyle Hidden -PassThru
    try {
        if (-not $process.WaitForExit(10000)) {
            # This is only the exact child launched above, never Explorer or another app.
            $process.Kill()
            $process.WaitForExit()
            throw "Smoke iteration $iteration timed out."
        }
        $process.Refresh()
        if ($process.ExitCode -ne 0) { throw "Smoke iteration $iteration failed: exit $($process.ExitCode)." }
        $watch.Stop()
        $results += [ordered]@{ iteration = $iteration; exit_code = $process.ExitCode; elapsed_ms = $watch.Elapsed.TotalMilliseconds }
    } finally {
        $process.Dispose()
    }
}
[ordered]@{
    test = 'P0-01 hidden HWND lifecycle'
    scope = 'create, resize, WM_PAINT dispatch, WM_CLOSE, WM_DESTROY, message-loop exit'
    not_tested = @('visual appearance', 'mixed DPI', 'Win+D', 'desktop parenting', 'Explorer restart', 'S1 performance')
    results = $results
} | ConvertTo-Json -Depth 5

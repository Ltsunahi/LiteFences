[CmdletBinding()]
param(
    [string]$OutputDirectory,
    [ValidateRange(1, 60)] [int]$TimeoutSeconds = 15
)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path $PSScriptRoot -Parent
$executable = Join-Path $projectRoot 'target\release\desktop-probe.exe'
if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) { throw 'Build desktop-probe first.' }
if (-not $OutputDirectory) {
    $OutputDirectory = Join-Path $projectRoot ("artifacts\desktop-{0}-{1}" -f (Get-Date -Format 'yyyyMMdd-HHmmss'), [Guid]::NewGuid().ToString('N').Substring(0, 8))
}
$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
if (Test-Path -LiteralPath $OutputDirectory) { throw 'Output directory must not already exist.' }
$null = New-Item -ItemType Directory -Path $OutputDirectory
foreach ($mode in @('snapshot', 'self-test')) {
    $jsonPath = Join-Path $OutputDirectory "$mode.json"
    $stderrPath = Join-Path $OutputDirectory "$mode.stderr.txt"
    $process = Start-Process -FilePath $executable -ArgumentList "--$mode" -WindowStyle Hidden -PassThru -RedirectStandardOutput $jsonPath -RedirectStandardError $stderrPath
    try {
        if (-not $process.WaitForExit($TimeoutSeconds * 1000)) {
            # Kill only this exact short-lived diagnostic child. Never kill Explorer.
            $process.Kill()
            $process.WaitForExit()
            throw "$mode timed out; partial evidence retained in $OutputDirectory."
        }
        $process.WaitForExit()
        $process.Refresh()
        if ($process.ExitCode -ne 0) { throw "$mode failed ($($process.ExitCode)); see $stderrPath." }
        $result = Get-Content -LiteralPath $jsonPath -Raw | ConvertFrom-Json
        if ($result.schema_version -ne 1) { throw 'Unexpected diagnostic schema.' }
        if ($mode -eq 'self-test' -and ($result.passed -ne $true -or $result.cycles -ne 20 -or $result.explorer_restarts -ne 0)) { throw 'Mock recovery test failed.' }
        if ($mode -eq 'snapshot' -and ($result.shell_hwnd -eq 0 -or $result.folder_view.status -ne 'ok')) {
            Write-Warning 'Shell snapshot incomplete. This is not a passed desktop compatibility test.'
        }
        Write-Output ([pscustomobject]@{ mode = $mode; evidence = $jsonPath; result = $result })
    } finally {
        $process.Dispose()
    }
}

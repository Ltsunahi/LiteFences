[CmdletBinding()]
param(
    [ValidateRange(1, 10)] [int]$Rounds = 3,
    [ValidateRange(0, 120)] [int]$WarmupSeconds = 5,
    [ValidateRange(2, 600)] [int]$DurationSeconds = 20,
    [ValidateRange(100, 5000)] [int]$IntervalMilliseconds = 1000,
    [string]$OutputDirectory,
    [switch]$SmokeOnly
)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path $PSScriptRoot -Parent
$executable = Join-Path $projectRoot 'target\release\render-bench.exe'
if (-not (Test-Path -LiteralPath $executable)) { throw 'Build --release --features render-bench first.' }
if (-not $OutputDirectory) { $OutputDirectory = Join-Path $projectRoot ('artifacts\render-' + [DateTime]::UtcNow.ToString('yyyyMMdd-HHmmss') + '-' + [guid]::NewGuid().ToString('N').Substring(0, 6)) }
if (Test-Path -LiteralPath $OutputDirectory) { throw 'Output directory already exists; refusing to overwrite results.' }
$directory = New-Item -ItemType Directory -Path $OutputDirectory
$OutputDirectory = $directory.FullName
. (Join-Path $PSScriptRoot 'Native-Metrics.ps1')

function Get-SystemSnapshot([int]$TargetProcessId = 0) {
    $hostProcesses = @(Get-Process -Name explorer,dwm -ErrorAction SilentlyContinue | ForEach-Object {
        $hostProcess = $_
        # Protected processes can expose memory counters without exposing StartTime.
        # Keep optional identity metadata failure separate from the counter reads.
        try {
            $snapshot = [ordered]@{ name = $hostProcess.ProcessName; process_id = $hostProcess.Id; private_bytes = $hostProcess.PrivateMemorySize64; working_set = $hostProcess.WorkingSet64; start_utc = $null; identity_status = 'start time unavailable'; status = 'collected' }
            try { $snapshot.start_utc = $hostProcess.StartTime.ToUniversalTime().ToString('o'); $snapshot.identity_status = 'pid and start time available' } catch { }
            $snapshot
        } catch { [ordered]@{ name = $hostProcess.ProcessName; process_id = $hostProcess.Id; status = 'unavailable' } }
    })
    $gpu = [ordered]@{ status = 'unavailable'; dedicated_bytes = $null; shared_bytes = $null; instances = @() }
    if ($TargetProcessId -gt 0) {
        try {
            $instances = @(Get-CimInstance -ClassName Win32_PerfFormattedData_GPUPerformanceCounters_GPUProcessMemory -OperationTimeoutSec 5 -ErrorAction Stop | Where-Object { $_.Name -match "^pid_${TargetProcessId}_" })
            if ($instances.Count) {
                $gpu.status = 'collected: GPUProcessMemory across adapter instances'
                $gpu.instances = @($instances | ForEach-Object { [ordered]@{ name = $_.Name; dedicated_bytes = [double]$_.DedicatedUsage; shared_bytes = [double]$_.SharedUsage } })
                $gpu.dedicated_bytes = ($gpu.instances.dedicated_bytes | Measure-Object -Sum).Sum
                $gpu.shared_bytes = ($gpu.instances.shared_bytes | Measure-Object -Sum).Sum
            } else { $gpu.status = 'no matching GPU instance; not interpreted as zero' }
        } catch { $gpu.status = 'unavailable: GPU performance provider query failed' }
    }
    [ordered]@{ utc = [DateTime]::UtcNow.ToString('o'); host_processes = $hostProcesses; gpu = $gpu }
}

$scenarios = @('empty','gdi-surface','gdi-icons','d2d-surface','d2d-icons')
$runs = [Collections.Generic.List[object]]::new()
$environment = & (Join-Path $PSScriptRoot 'Get-Environment.ps1') | ConvertFrom-Json
$report = [ordered]@{
    schema_version = 1; kind = if ($SmokeOnly) { 'smoke' } else { 'P0-02 synthetic render microbenchmark; NOT S1 acceptance' }
    created_utc = [DateTime]::UtcNow.ToString('o'); executable_sha256 = (Get-FileHash -LiteralPath $executable -Algorithm SHA256).Hash
    environment = $environment; settings = [ordered]@{ rounds = $Rounds; warmup_seconds = $WarmupSeconds; duration_seconds = $DurationSeconds; interval_ms = $IntervalMilliseconds; scenario_order = 'rotate by round' }
    runs = @()
}
$resultPath = Join-Path $OutputDirectory 'results.json'
for ($round = 1; $round -le $Rounds; $round++) {
    # Rotate the start scenario to reduce always-first startup/cache bias.
    $offset = ($round - 1) % $scenarios.Count
    foreach ($index in 0..($scenarios.Count - 1)) {
        $scenario = $scenarios[($index + $offset) % $scenarios.Count]
        $ready = Join-Path $OutputDirectory ("$round-$scenario-ready.json")
        $run = [ordered]@{ scenario = $scenario; round = $round; status = 'failed'; sample_count = 0; metrics = @{} }
        $process = $null
        try {
            $before = Get-SystemSnapshot
            $arguments = @('--scenario', $scenario, '--ready-file', ('"' + $ready + '"'), '--lifetime-seconds', ($WarmupSeconds + $DurationSeconds + 45))
            if ($SmokeOnly) { $arguments += '--smoke-test' }
            $process = Start-Process -FilePath $executable -ArgumentList $arguments -WindowStyle Hidden -PassThru
            $deadline = [Diagnostics.Stopwatch]::StartNew()
            while (-not (Test-Path -LiteralPath $ready)) {
                if ($process.HasExited) { throw "Benchmark exited before ready, code $($process.ExitCode)." }
                if ($deadline.Elapsed.TotalSeconds -gt 20) { throw 'Timed out waiting for first rendered frame.' }
                Start-Sleep -Milliseconds 50
            }
            # File existence can precede the final write; retry parsing briefly.
            $metadata = $null
            for ($retry = 0; $retry -lt 20 -and $null -eq $metadata; $retry++) {
                try { $metadata = Get-Content -LiteralPath $ready -Raw | ConvertFrom-Json } catch { Start-Sleep -Milliseconds 50 }
            }
            if ($null -eq $metadata -or $metadata.pid -ne $process.Id -or $metadata.scenario -ne $scenario) { throw 'Invalid ready metadata.' }
            if ($metadata.client_width_px -ne 1120 -or $metadata.client_height_px -ne 680) { throw 'Unexpected surface dimensions.' }
            $expectedIcons = if ($scenario -like '*-icons') { 200 } else { 0 }
            if ($metadata.icon_count -ne $expectedIcons) { throw 'Unexpected icon count.' }
            $run.metadata = $metadata; $run.before = $before
            if (-not $SmokeOnly) {
                Start-Sleep -Seconds $WarmupSeconds
                $run.steady_system = Get-SystemSnapshot -TargetProcessId $process.Id
                $measurement = & (Join-Path $PSScriptRoot 'Measure-Process.ps1') -ProcessId $process.Id -DurationSeconds $DurationSeconds -IntervalMilliseconds $IntervalMilliseconds -IncludeSamples | ConvertFrom-Json
                $run.measurement = $measurement; $run.sample_count = $measurement.sample_count
                if ($measurement.status -ne 'completed' -or $measurement.sample_count -lt 2) { throw 'Incomplete measurement.' }
                if (@($measurement.samples | Where-Object { -not $_.data.main_window_visible -or $_.data.main_window_minimized }).Count) { throw 'Window was hidden or minimized during sampling; run invalid.' }
                $run.metrics = [ordered]@{
                    private_bytes_p95 = $measurement.memory.private_bytes.p95; working_set_p95 = $measurement.memory.working_set.p95
                    private_working_set_p95 = $measurement.memory.private_working_set.p95; cpu_normalized_percent = $measurement.cpu.normalized_percent
                    gdi_objects = $measurement.gdi_objects.p95; user_objects = $measurement.user_objects.p95
                }
                if ($process.HasExited) { throw 'Benchmark exited early.' }
                [void]$process.CloseMainWindow()
            }
            if (-not $process.WaitForExit(10000)) { throw 'Benchmark failed to close.' }
            $process.Refresh()
            if ($process.ExitCode -ne 0) { throw "Benchmark exited with code $($process.ExitCode)." }
            $run.exit_code = $process.ExitCode; $run.status = if ($SmokeOnly) { 'smoke_passed' } else { 'completed' }
            $run.after = Get-SystemSnapshot
        } catch { $run.error = $_.Exception.Message }
        finally {
            if ($process) {
                if (-not $process.HasExited) { $process.Kill(); $process.WaitForExit() }
                $process.Dispose()
            }
        }
        $runs.Add($run); $report.runs = @($runs.ToArray())
        # Generated measurement data, checkpoint after each run so interruptions retain evidence.
        $report | ConvertTo-Json -Depth 20 | Set-Content -LiteralPath $resultPath -Encoding utf8
        Write-Output ("round={0} scenario={1} status={2}" -f $round,$scenario,$run.status)
        if ($run.status -eq 'failed') { throw "Run failed: $($run.error). Partial report: $resultPath" }
    }
}
if (-not $SmokeOnly) {
    & (Join-Path $PSScriptRoot 'Summarize-RenderBench.ps1') -ResultsPath $resultPath | Set-Content -LiteralPath (Join-Path $OutputDirectory 'summary.json') -Encoding utf8
}
Write-Output "Results: $resultPath"

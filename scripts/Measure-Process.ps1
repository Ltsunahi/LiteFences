[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)] [ValidateRange(1, [int]::MaxValue)] [int]$ProcessId,
    [ValidateRange(1, 86400)] [int]$DurationSeconds = 30,
    [ValidateRange(1, 3600000)] [int]$IntervalMilliseconds = 1000,
    [switch]$IncludeSamples
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'Native-Metrics.ps1')
$logical = [LiteFences.BenchCounters]::GetActiveProcessorCount(0xffff)
if ($logical -eq 0) { throw 'Could not determine active logical processors across all processor groups.' }
$samples = New-Object System.Collections.Generic.List[object]
$status = 'completed'; $message = $null; $process = $null
$stopwatch = [Diagnostics.Stopwatch]::StartNew()

function Get-Sample {
    param([Diagnostics.Process]$Process)
    try {
        $Process.Refresh()
        if ($Process.HasExited) { return $null }
        # Read required Process metrics first. Failures here mean the sample itself is unusable.
        $privateBytesValue = $Process.PrivateMemorySize64
        $workingSetValue = $Process.WorkingSet64
        $cpuTimeValue = $Process.TotalProcessorTime
        $handleCountValue = $Process.HandleCount
        if ($null -eq $privateBytesValue -or $null -eq $workingSetValue -or
            $null -eq $cpuTimeValue -or $null -eq $handleCountValue) {
            throw 'Required Process counters were unavailable'
        }
        $privateBytes = [int64]$privateBytesValue
        $workingSet = [int64]$workingSetValue
        $cpuSeconds = [double]$cpuTimeValue.TotalSeconds

        $sample = [pscustomobject]@{
            private_bytes = $privateBytes; working_set = $workingSet
            private_working_set = $null; cpu_seconds = $cpuSeconds; handle_count = [int]$handleCountValue
            gdi_objects = $null; user_objects = $null
            main_window_visible = $null; main_window_minimized = $null
        }

        # Optional native counters must never invalidate the required sample.
        try {
            $native = [LiteFences.BenchCounters]::Read($Process.Handle)
            if ($native) { $sample.private_working_set = [double]$native.PrivateWorkingSetSize.ToUInt64() }
        } catch { }
        try {
            $value = [int][LiteFences.BenchCounters]::GetGuiResources($Process.Handle, 0)
            if ($value -gt 0) { $sample.gdi_objects = $value }
        } catch { }
        try {
            $value = [int][LiteFences.BenchCounters]::GetGuiResources($Process.Handle, 1)
            if ($value -gt 0) { $sample.user_objects = $value }
        } catch { }
        try {
            $windowHandle = [IntPtr]$Process.MainWindowHandle
            if ($windowHandle -ne [IntPtr]::Zero) {
                try { $sample.main_window_visible = [LiteFences.BenchCounters]::IsWindowVisible($windowHandle) } catch { }
                try { $sample.main_window_minimized = [LiteFences.BenchCounters]::IsIconic($windowHandle) } catch { }
            }
        } catch { }
        return $sample
    } catch { return $null }
}

try {
    try { $process = [Diagnostics.Process]::GetProcessById($ProcessId) } catch [System.ArgumentException] { throw $_ }
    $sample = Get-Sample $process
    if ($sample) { $samples.Add([pscustomobject]@{ elapsed_seconds = $stopwatch.Elapsed.TotalSeconds; data = $sample }) } else { $status = 'access_failed'; $message = 'process could not be read' }
    while ($status -eq 'completed' -and $stopwatch.Elapsed.TotalSeconds -lt $DurationSeconds) {
        $remaining = [Math]::Max(1, [int](($DurationSeconds - $stopwatch.Elapsed.TotalSeconds) * 1000))
        Start-Sleep -Milliseconds ([Math]::Min($IntervalMilliseconds, $remaining))
        $sample = Get-Sample $process
        if ($sample) { $samples.Add([pscustomobject]@{ elapsed_seconds = $stopwatch.Elapsed.TotalSeconds; data = $sample }) }
        elseif ($process.HasExited) { $status = 'process_exited'; $message = 'process exited during measurement'; break }
        else { $status = 'access_failed'; $message = 'process sample could not be read'; break }
    }
} catch [System.ArgumentException] { $status = 'process_not_found'; $message = 'specified process was not found' }
catch { $status = 'access_failed'; $message = 'process could not be opened or read' }
finally { $stopwatch.Stop(); if ($process) { $process.Dispose() } }

$data = @($samples | ForEach-Object { $_.data })
function Get-Percentile([double[]]$Values, [double]$P) {
    if (-not $Values -or $Values.Count -eq 0) { return $null }
    $sorted = @($Values | Sort-Object); [double]$sorted[[Math]::Max(0, [int]([Math]::Ceiling($P * $sorted.Count) - 1))]
}
function Get-Metric([string]$Name) {
    $values = @($data | Where-Object { $null -ne $_.$Name } | ForEach-Object { [double]$_.$Name })
    [ordered]@{
        p95 = Get-Percentile $values 0.95
        peak = if ($values.Count) { ($values | Measure-Object -Maximum).Maximum } else { $null }
        valid_sample_count = $values.Count
        status = if ($values.Count -eq $data.Count -and $values.Count -gt 0) { 'collected' } elseif ($values.Count -gt 0) { 'partial' } else { 'unavailable' }
    }
}

$firstSample = if ($data.Count) { $data[0] } else { $null }; $lastSample = if ($data.Count) { $data[-1] } else { $null }
$sampleElapsed = if ($samples.Count -ge 2) { [double]$samples[-1].elapsed_seconds - [double]$samples[0].elapsed_seconds } else { $null }
$cpuDelta = if ($samples.Count -ge 2) { [double]$lastSample.cpu_seconds - [double]$firstSample.cpu_seconds } else { $null }
$pwsCount = @($data | Where-Object { $null -ne $_.private_working_set }).Count
$windowCount = @($data | Where-Object { $null -ne $_.main_window_visible -and $null -ne $_.main_window_minimized }).Count

[ordered]@{
    process_id = $ProcessId; status = $status; message = $message; sample_count = $data.Count; elapsed_seconds = [double]$stopwatch.Elapsed.TotalSeconds
    memory = [ordered]@{
        private_bytes = Get-Metric 'private_bytes'; working_set = Get-Metric 'working_set'
        private_working_set = [ordered]@{
            p95 = (Get-Metric 'private_working_set').p95; peak = (Get-Metric 'private_working_set').peak
            valid_sample_count = $pwsCount; status = if ($pwsCount -eq $data.Count -and $pwsCount -gt 0) { 'collected: PROCESS_MEMORY_COUNTERS_EX2' } elseif ($pwsCount -gt 0) { 'partial' } else { 'unavailable' }
        }
    }
    cpu = [ordered]@{ total_seconds_delta = $cpuDelta; sample_elapsed_seconds = $sampleElapsed; logical_processors = $logical; source = 'GetActiveProcessorCount(ALL_PROCESSOR_GROUPS)'; normalized_percent = if ($null -ne $cpuDelta -and $sampleElapsed -gt 0) { $cpuDelta / $sampleElapsed / $logical * 100 } else { $null } }
    handle_count = if ($lastSample) { $lastSample.handle_count } else { $null }
    gdi_objects = Get-Metric 'gdi_objects'; user_objects = Get-Metric 'user_objects'
    optional_availability = [ordered]@{
        gdi_objects = if (@($data | Where-Object { $null -ne $_.gdi_objects }).Count) { 'available; zero readings treated as unavailable' } else { 'unavailable; zero is ambiguous or call failed' }
        user_objects = if (@($data | Where-Object { $null -ne $_.user_objects }).Count) { 'available; zero readings treated as unavailable' } else { 'unavailable; zero is ambiguous or call failed' }
        window_visibility = if ($windowCount -gt 0) { 'available' } else { 'unavailable' }
        private_working_set = (Get-Metric 'private_working_set').status
    }
    window = [ordered]@{
        valid_sample_count = $windowCount
        status = if ($windowCount -eq $data.Count -and $windowCount -gt 0) { 'collected' } elseif ($windowCount -gt 0) { 'partial' } else { 'unavailable' }
        visible = if ($lastSample) { $lastSample.main_window_visible } else { $null }
        minimized = if ($lastSample) { $lastSample.main_window_minimized } else { $null }
    }
    samples = if ($IncludeSamples) { @($samples.ToArray()) } else { $null }
} | ConvertTo-Json -Depth 8 -Compress

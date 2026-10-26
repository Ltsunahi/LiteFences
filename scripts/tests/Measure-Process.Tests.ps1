Describe 'Measure-Process optional counters' {
    $scriptPath = Join-Path $PSScriptRoot '..\Measure-Process.ps1'
    $source = Get-Content -LiteralPath $scriptPath -Raw
    if (-not ('LiteFences.MeasureTestCounters' -as [type])) {
        Add-Type -TypeDefinition @'
using System;
namespace LiteFences {
    public static class MeasureTestCounters {
        public static uint GetActiveProcessorCount(ushort group) { return 1; }
        public static object Read(IntPtr handle) { throw new InvalidOperationException("injected"); }
        public static uint GetGuiResources(IntPtr handle, uint flags) { throw new InvalidOperationException("injected"); }
        public static bool IsWindowVisible(IntPtr handle) { throw new InvalidOperationException("injected"); }
        public static bool IsIconic(IntPtr handle) { throw new InvalidOperationException("injected"); }
    }
}
'@
    }
    # Only replace native counter calls in this trusted local script, not Process reads.
    $source = $source.Replace(". (Join-Path `$PSScriptRoot 'Native-Metrics.ps1')", '')
    $source = $source.Replace('LiteFences.BenchCounters', 'LiteFences.MeasureTestCounters')
    $injectedScript = [scriptblock]::Create($source)

    It 'retains required samples when every optional native counter fails' {
        $result = (& $injectedScript -ProcessId $PID -DurationSeconds 1 -IntervalMilliseconds 250 -IncludeSamples) | ConvertFrom-Json
        $result.status | Should Be 'completed'
        ($result.sample_count -ge 2) | Should Be $true
        $result.memory.private_bytes.status | Should Be 'collected'
        ($result.memory.private_bytes.p95 -gt 0) | Should Be $true
        ($result.memory.working_set.p95 -gt 0) | Should Be $true
        ($null -ne $result.cpu.normalized_percent) | Should Be $true
        ($result.handle_count -gt 0) | Should Be $true
        $result.memory.private_working_set.status | Should Be 'unavailable'
        $result.gdi_objects.status | Should Be 'unavailable'
        $result.user_objects.status | Should Be 'unavailable'
        $result.window.status | Should Be 'unavailable'
        foreach ($sample in $result.samples) {
            ($null -eq $sample.data.gdi_objects) | Should Be $true
            ($null -eq $sample.data.user_objects) | Should Be $true
            ($null -eq $sample.data.private_working_set) | Should Be $true
            ($null -eq $sample.data.main_window_visible) | Should Be $true
            ($null -eq $sample.data.main_window_minimized) | Should Be $true
        }
    }

    It 'does not invent samples for a nonexistent process' {
        $result = (& $injectedScript -ProcessId ([int]::MaxValue) -DurationSeconds 1) | ConvertFrom-Json
        $result.status | Should Be 'process_not_found'
        $result.sample_count | Should Be 0
        ($null -eq $result.memory.private_bytes.p95) | Should Be $true
        ($null -eq $result.cpu.normalized_percent) | Should Be $true
        ($null -eq $result.handle_count) | Should Be $true
    }
}

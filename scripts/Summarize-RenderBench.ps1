[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string] $ResultsPath
)

$ErrorActionPreference = 'Stop'

function Fail([string] $Message) {
    throw "Summarize-RenderBench: $Message"
}

if (-not (Test-Path -LiteralPath $ResultsPath -PathType Leaf)) {
    Fail "ResultsPath does not exist: $ResultsPath"
}

try {
    $json = Get-Content -LiteralPath $ResultsPath -Raw
    $document = $json | ConvertFrom-Json
} catch {
    if ($_.Exception.Message -like 'Summarize-RenderBench:*') { throw }
    Fail "could not parse JSON at '$ResultsPath': $($_.Exception.Message)"
}

if ($null -eq $document -or $document.schema_version -ne 1) {
    Fail 'schema_version must be 1'
}
if ($null -eq $document.runs -or $document.runs -isnot [array]) {
    Fail 'runs must be a JSON array'
}

$scenarios = @('empty', 'gdi-surface', 'gdi-icons', 'd2d-surface', 'd2d-icons')
$metrics = @(
    'private_bytes_p95',
    'working_set_p95',
    'private_working_set_p95',
    'cpu_normalized_percent',
    'gdi_objects',
    'user_objects'
)
$seen = @{}
$completed = @{}

foreach ($run in @($document.runs)) {
    if ($null -eq $run.scenario -or $run.scenario -notin $scenarios) {
        Fail "invalid scenario: $($run.scenario)"
    }
    if ($null -eq $run.round -or $run.round -is [bool] -or
        $run.round -isnot [int] -and $run.round -isnot [long] -and
        $run.round -isnot [uint32] -and $run.round -isnot [uint64] -or
        [double]$run.round -le 0 -or [double]$run.round % 1 -ne 0) {
        Fail "invalid round for scenario '$($run.scenario)'"
    }
    if ($null -eq $run.sample_count -or $run.sample_count -is [bool] -or
        $run.sample_count -isnot [int] -and $run.sample_count -isnot [long] -and
        $run.sample_count -isnot [uint32] -and $run.sample_count -isnot [uint64] -or
        [double]$run.sample_count -lt 0 -or [double]$run.sample_count % 1 -ne 0) {
        Fail "invalid sample_count for scenario '$($run.scenario)', round $($run.round)"
    }
    $key = "{0}|{1}" -f $run.scenario, $run.round
    if ($seen.ContainsKey($key)) {
        Fail "duplicate scenario+round: $($run.scenario), round $($run.round)"
    }
    $seen[$key] = $true

    if ($run.status -eq 'completed' -and $run.sample_count -is [ValueType] -and
        -not ($run.sample_count -is [bool]) -and [double]$run.sample_count -ge 2) {
        if (-not $completed.ContainsKey($run.scenario)) { $completed[$run.scenario] = @() }
        $completed[$run.scenario] += $run
    }
}

function Get-Numbers($runs, [string] $Metric) {
    $values = @()
    foreach ($run in @($runs)) {
        $value = $run.metrics.$Metric
        if ($null -eq $value -or $value -is [bool] -or
            ($value -isnot [byte] -and $value -isnot [sbyte] -and
             $value -isnot [int16] -and $value -isnot [uint16] -and
             $value -isnot [int32] -and $value -isnot [uint32] -and
             $value -isnot [int64] -and $value -isnot [uint64] -and
             $value -isnot [single] -and $value -isnot [double] -and
             $value -isnot [decimal])) { continue }
        $number = [double]$value
        if (-not [double]::IsNaN($number) -and -not [double]::IsInfinity($number)) { $values += $number }
    }
    return $values
}

function Get-Median([object[]] $InputValues) {
    $sorted = @($InputValues | Sort-Object)
    if ($sorted.Count -eq 0) { return $null }
    $middle = [int][math]::Floor($sorted.Count / 2)
    if (($sorted.Count % 2) -eq 1) { return $sorted[$middle] }
    return (($sorted[$middle - 1] + $sorted[$middle]) / 2.0)
}

$scenarioOutput = [ordered]@{}
foreach ($scenario in $scenarios) {
    $runs = if ($completed.ContainsKey($scenario)) { @($completed[$scenario]) } else { @() }
    $metricOutput = [ordered]@{}
    foreach ($metric in $metrics) {
        $metricOutput[$metric] = Get-Median -InputValues @(Get-Numbers $runs $metric)
    }
    $scenarioOutput[$scenario] = [ordered]@{ runs = $runs.Count; metrics = $metricOutput }
}

$pairs = @(
    @{ name = 'gdi-surface-empty'; left = 'gdi-surface'; right = 'empty' },
    @{ name = 'd2d-surface-empty'; left = 'd2d-surface'; right = 'empty' },
    @{ name = 'gdi-icons-gdi-surface'; left = 'gdi-icons'; right = 'gdi-surface' },
    @{ name = 'd2d-icons-d2d-surface'; left = 'd2d-icons'; right = 'd2d-surface' }
)
$incrementOutput = [ordered]@{}
foreach ($pair in $pairs) {
    $differences = @()
    $rightByRound = @{}
    foreach ($run in @($completed[$pair.right])) { $rightByRound[[string]$run.round] = $run }
    foreach ($run in @($completed[$pair.left])) {
        $right = $rightByRound[[string]$run.round]
        if ($null -eq $right) { continue }
        $leftValue = Get-Numbers @($run) 'private_bytes_p95'
        $rightValue = Get-Numbers @($right) 'private_bytes_p95'
        if (@($leftValue).Count -eq 1 -and @($rightValue).Count -eq 1) {
            $differences += ([double]$leftValue[0] - [double]$rightValue[0])
        }
    }
    $incrementOutput[$pair.name] = [ordered]@{
        difference_median = Get-Median -InputValues @($differences)
        valid_pair_count = @($differences).Count
    }
}

[ordered]@{
    schema_version = 1
    scenarios = $scenarioOutput
    increments = $incrementOutput
} | ConvertTo-Json -Depth 10 -Compress

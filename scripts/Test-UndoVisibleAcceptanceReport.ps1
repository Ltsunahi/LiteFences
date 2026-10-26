param(
    [Parameter(Mandatory = $true)]
    [string] $DataDir,

    [ValidateSet('passed', 'failed')]
    [string] $ExpectedStatus = 'passed'
)

$ErrorActionPreference = 'Stop'

function Add-Check([System.Collections.Generic.List[object]] $Checks, [string] $Name, [bool] $Passed) {
    $Checks.Add([ordered]@{ name = $Name; passed = $Passed })
}

function Test-JsonEqual($Left, $Right) {
    if ($null -eq $Left -or $null -eq $Right) { return ($null -eq $Left -and $null -eq $Right) }
    if ($Left -is [pscustomobject] -or $Right -is [pscustomobject]) {
        if ($Left -isnot [pscustomobject] -or $Right -isnot [pscustomobject]) { return $false }
        $leftNames = @($Left.PSObject.Properties.Name)
        $rightNames = @($Right.PSObject.Properties.Name)
        if ($leftNames.Count -ne $rightNames.Count) { return $false }
        foreach ($name in $leftNames) {
            if ($rightNames -cnotcontains $name -or -not (Test-JsonEqual $Left.$name $Right.$name)) { return $false }
        }
        return $true
    }
    if ($Left -is [System.Collections.IDictionary] -or $Right -is [System.Collections.IDictionary]) {
        if ($Left -isnot [System.Collections.IDictionary] -or $Right -isnot [System.Collections.IDictionary]) { return $false }
        if ($Left.Count -ne $Right.Count) { return $false }
        foreach ($key in $Left.Keys) {
            if (-not $Right.Contains($key) -or -not (Test-JsonEqual $Left[$key] $Right[$key])) { return $false }
        }
        return $true
    }
    if ($Left -is [array] -or $Right -is [array]) {
        if ($Left -isnot [array] -or $Right -isnot [array] -or $Left.Length -ne $Right.Length) { return $false }
        for ($i = 0; $i -lt $Left.Length; $i++) {
            if (-not (Test-JsonEqual $Left[$i] $Right[$i])) { return $false }
        }
        return $true
    }
    if ($Left -is [bool] -or $Right -is [bool]) { return ($Left -is [bool] -and $Right -is [bool] -and $Left -eq $Right) }
    if ($Left -is [string] -or $Right -is [string]) { return ($Left -is [string] -and $Right -is [string] -and $Left -ceq $Right) }
    if ($Left -is [ValueType] -and $Right -is [ValueType]) { return ($Left.GetType() -eq $Right.GetType() -and $Left -eq $Right) }
    return $false
}

function Test-IntegerJsonValue($Value) {
    return ($Value -is [byte] -or $Value -is [sbyte] -or $Value -is [int16] -or $Value -is [uint16] -or
        $Value -is [int32] -or $Value -is [uint32] -or $Value -is [int64] -or $Value -is [uint64])
}

$checks = [System.Collections.Generic.List[object]]::new()
$projectRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$artifactsRoot = [System.IO.Path]::GetFullPath((Join-Path $projectRoot 'artifacts'))
$hasDotSegment = @($DataDir -split '[\\/]') | Where-Object { $_ -eq '.' -or $_ -eq '..' }
if ($DataDir -notmatch '^[A-Za-z]:\\' -or $hasDotSegment.Count -gt 0) {
    throw 'DataDir must be a canonical absolute drive path without dot segments.'
}
$fullDataDir = [System.IO.Path]::GetFullPath($DataDir)
if (-not [string]::Equals($DataDir, $fullDataDir, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw 'DataDir must already be in canonical absolute form.'
}
$parent = [System.IO.Path]::GetDirectoryName($fullDataDir)
$leaf = [System.IO.Path]::GetFileName($fullDataDir)
if (-not [string]::Equals($parent, $artifactsRoot, [System.StringComparison]::OrdinalIgnoreCase) -or
    $leaf -notmatch '^undo-(group-)?visible-(\d+)-(\d+)$') {
    throw 'DataDir must be a directly contained undo-visible artifact directory.'
}
$directoryPid = [uint64]::Parse($Matches[2], [Globalization.CultureInfo]::InvariantCulture)
$directoryKind = if ($Matches[1]) { 'group' } else { 'reference' }

$directory = Get-Item -LiteralPath $fullDataDir -Force
if (-not $directory.PSIsContainer -or ($directory.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
    throw 'DataDir must be a regular directory, not a reparse point.'
}
$reportPath = Join-Path $fullDataDir 'undo-visible-report.json'
$configPath = Join-Path $fullDataDir 'config.json'
$seedPath = Join-Path $fullDataDir 'seed-config.json'
$sourcePath = Join-Path $fullDataDir 'source.txt'
foreach ($path in @($reportPath, $configPath, $seedPath, $sourcePath)) {
    $item = Get-Item -LiteralPath $path -Force
    if ($item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "Fixture file must be a regular file, not a reparse point: $($item.Name)"
    }
}

$report = Get-Content -LiteralPath $reportPath -Raw | ConvertFrom-Json
$config = Get-Content -LiteralPath $configPath -Raw | ConvertFrom-Json
$seed = Get-Content -LiteralPath $seedPath -Raw | ConvertFrom-Json
if ($report -isnot [pscustomobject] -or $config -isnot [pscustomobject] -or $seed -isnot [pscustomobject]) {
    throw 'Report, config and seed JSON must be objects.'
}
foreach ($field in @('status', 'undo_kind', 'scope', 'data_dir', 'process_id', 'undo_fixture_seeded', 'undo_fixture_restored', 'source_unchanged', 'disk_restored_exact', 'timeout_fired')) {
    if (@($report.PSObject.Properties.Name) -cnotcontains $field) { throw "Report is missing required field: $field" }
}
$reportStatus = $report.status
if ($reportStatus -isnot [string] -or $reportStatus -notin @('passed', 'failed')) { throw 'Report status must be passed or failed.' }
$kindMatches = ($report.undo_kind -is [string] -and $report.undo_kind -ceq $directoryKind)
$scopeMatches = ($report.scope -is [string] -and -not [string]::IsNullOrWhiteSpace($report.scope))
$reportedDirMatches = $report.data_dir -is [string] -and [string]::Equals($report.data_dir, $fullDataDir, [StringComparison]::OrdinalIgnoreCase)
$pidIsInteger = Test-IntegerJsonValue $report.process_id
$pidMatches = $pidIsInteger -and ([uint64]$report.process_id -eq $directoryPid)
Add-Check $checks 'report_status' ($reportStatus -ceq $ExpectedStatus)
Add-Check $checks 'report_kind_matches_directory' $kindMatches
Add-Check $checks 'scope_is_nonempty' $scopeMatches
Add-Check $checks 'data_dir_matches' $reportedDirMatches
Add-Check $checks 'process_id_matches_directory' $pidMatches
Add-Check $checks 'timeout_fired_is_boolean' ($report.timeout_fired -is [bool])

$flags = @{}
foreach ($field in @('undo_fixture_seeded', 'undo_fixture_restored', 'source_unchanged', 'disk_restored_exact')) {
    $isBoolean = $report.$field -is [bool]
    Add-Check $checks "${field}_is_boolean" $isBoolean
    if (-not $isBoolean) { throw "Report field $field must be a JSON boolean." }
    $flags[$field] = $report.$field
}
$sourceBytes = [IO.File]::ReadAllBytes($sourcePath)
$expectedSource = [Text.Encoding]::ASCII.GetBytes("LiteFences undo visible acceptance fixture.`r`nThis file must remain unchanged.`r`n")
$sourceMatches = [Convert]::ToBase64String($sourceBytes) -ceq [Convert]::ToBase64String($expectedSource)
$configMatchesSeed = Test-JsonEqual $config $seed
Add-Check $checks 'source_bytes_match_fixture' $sourceMatches
Add-Check $checks 'reported_source_unchanged_matches_bytes' ($flags.source_unchanged -eq $sourceMatches)
Add-Check $checks 'reported_disk_exact_matches_structure' ($flags.disk_restored_exact -eq $configMatchesSeed)

$allTrue = $flags.undo_fixture_seeded -and $flags.undo_fixture_restored -and $flags.source_unchanged -and $flags.disk_restored_exact
if ($ExpectedStatus -eq 'passed') {
    $semanticPass = $allTrue -and $sourceMatches -and $configMatchesSeed
} else {
    $semanticPass = (-not $allTrue) -and ($reportStatus -ceq 'failed')
}
$allChecksPass = $semanticPass -and (@($checks | Where-Object { -not $_.passed }).Count -eq 0)
Add-Check $checks 'expected_status_semantics' $semanticPass
$verificationPassed = $allChecksPass
[ordered]@{
    expected_status = $ExpectedStatus
    report_status = $reportStatus
    verification_passed = $verificationPassed
    checks = @($checks)
} | ConvertTo-Json -Depth 8
if (-not $verificationPassed) { exit 1 }
exit 0

[CmdletBinding()]
param([string]$ExecutablePath)

$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path $PSScriptRoot -Parent
$executable = if ($ExecutablePath) { [IO.Path]::GetFullPath($ExecutablePath) } else { Join-Path $projectRoot 'target\release\litefences.exe' }
if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) { throw "Executable not found: $executable" }

$root = Join-Path $projectRoot ('artifacts\item-drag-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $root | Out-Null
$results = [Collections.Generic.List[object]]::new()

function New-Case([string]$name) {
    $path = Join-Path $root $name
    New-Item -ItemType Directory -Path $path | Out-Null
    return $path
}
function Write-Utf8([string]$path, [string]$value) {
    [IO.File]::WriteAllText($path, $value, [Text.UTF8Encoding]::new($false))
}
function Check([bool]$condition, [string]$message) { if (-not $condition) { throw $message } }

function Same-Item($actual, $expected) {
    if ($actual.id -cne $expected.id -or $actual.target.kind -cne $expected.target.kind) { return $false }
    $actualPath = @($actual.target.path_utf16)
    $expectedPath = @($expected.target.path_utf16)
    if ($actualPath.Count -ne $expectedPath.Count) { return $false }
    for ($index = 0; $index -lt $actualPath.Count; $index++) {
        if ([int]$actualPath[$index] -ne [int]$expectedPath[$index]) { return $false }
    }
    return $true
}
function Same-Group($actual, $expected) {
    if ($actual.id -cne $expected.id -or $actual.title -cne $expected.title -or
        $actual.rect.x -ne $expected.rect.x -or $actual.rect.y -ne $expected.rect.y -or
        $actual.rect.width -ne $expected.rect.width -or $actual.rect.height -ne $expected.rect.height -or
        $actual.collapsed -ne $expected.collapsed -or
        (ConvertTo-Json -InputObject $actual.color -Compress) -cne (ConvertTo-Json -InputObject $expected.color -Compress)) { return $false }
    if ($null -eq $actual.monitor_key_utf16 -or $null -eq $expected.monitor_key_utf16) {
        if ($null -ne $actual.monitor_key_utf16 -or $null -ne $expected.monitor_key_utf16) { return $false }
    } else {
        $actualMonitor = @($actual.monitor_key_utf16)
        $expectedMonitor = @($expected.monitor_key_utf16)
        if ($actualMonitor.Count -ne $expectedMonitor.Count) { return $false }
        for ($index = 0; $index -lt $actualMonitor.Count; $index++) {
            if ([int]$actualMonitor[$index] -ne [int]$expectedMonitor[$index]) { return $false }
        }
    }
    $actualItems = @($actual.items)
    $expectedItems = @($expected.items)
    if ($actualItems.Count -ne $expectedItems.Count) { return $false }
    for ($index = 0; $index -lt $actualItems.Count; $index++) {
        if (-not (Same-Item $actualItems[$index] $expectedItems[$index])) { return $false }
    }
    return $true
}
function Same-Config($actual, $expected) {
    if ($actual.schema_version -ne $expected.schema_version -or
        $actual.next_group_id -cne $expected.next_group_id -or $actual.next_item_id -cne $expected.next_item_id -or
        $actual.settings.launch_at_login -ne $expected.settings.launch_at_login) { return $false }
    $actualGroups = @($actual.groups)
    $expectedGroups = @($expected.groups)
    if ($actualGroups.Count -ne $expectedGroups.Count) { return $false }
    for ($index = 0; $index -lt $actualGroups.Count; $index++) {
        if (-not (Same-Group $actualGroups[$index] $expectedGroups[$index])) { return $false }
    }
    return $true
}
function Run-Case([string]$name, [string]$path, [string]$mode) {
    $watch = [Diagnostics.Stopwatch]::StartNew()
    $arguments = @('--data-dir', ('"' + $path + '"'), '--storage-smoke', $mode)
    $process = Start-Process -FilePath $executable -WindowStyle Hidden -PassThru -ArgumentList $arguments
    try {
        if (-not $process.WaitForExit(10000)) {
            # Terminate only the exact child process this invocation launched.
            $process.Kill()
            if (-not $process.WaitForExit(5000)) { throw "$name did not exit after terminating its own timed-out child." }
            throw "$name timed out after 10 seconds; artifacts retained at $root"
        }
        $process.Refresh()
        Check ($process.ExitCode -eq 0) "$name exited with code $($process.ExitCode). Artifacts retained at $root"
        $results.Add([ordered]@{ name = $name; mode = $mode; exit_code = $process.ExitCode; elapsed_ms = [math]::Round($watch.Elapsed.TotalMilliseconds, 2) })
    } finally { $process.Dispose() }
}

$scenarios = @(
    [pscustomobject]@{ name = 'ui-transfer'; mode = 'ui-transfer'; expectedMove = $true },
    [pscustomobject]@{ name = 'ui-transfer-cancel'; mode = 'ui-transfer-cancel'; expectedMove = $false },
    [pscustomobject]@{ name = 'ui-transfer-outside'; mode = 'ui-transfer-outside'; expectedMove = $false },
    [pscustomobject]@{ name = 'ui-transfer-capture'; mode = 'ui-transfer-capture'; expectedMove = $false }
)

foreach ($scenario in $scenarios) {
    $case = New-Case $scenario.name
    $sourcePaths = @()
    $sourceSnapshots = @()
    foreach ($id in 1..4) {
        $file = Join-Path $case ("fixture-item-$id.txt")
        $content = "Fixture $($scenario.name), source item $id; immutable bytes.`n"
        Write-Utf8 $file $content
        $bytes = [IO.File]::ReadAllBytes($file)
        $sourcePaths += $file
        $sourceSnapshots += [pscustomobject]@{ path = $file; hash = (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash; bytes = [Convert]::ToBase64String($bytes) }
    }

    $itemsSource = @()
    foreach ($id in 1..3) {
        $itemsSource += [ordered]@{ id = [string]$id; target = [ordered]@{ kind = 'file'; path_utf16 = @($sourcePaths[$id - 1].ToCharArray() | ForEach-Object { [int]$_ }) } }
    }
    $itemDest = [ordered]@{ id = '4'; target = [ordered]@{ kind = 'file'; path_utf16 = @($sourcePaths[3].ToCharArray() | ForEach-Object { [int]$_ }) } }
    $fixture = [ordered]@{
        schema_version = 1; next_group_id = '3'; next_item_id = '5'
        settings = [ordered]@{ launch_at_login = $false }
        groups = @(
            [ordered]@{ id = '1'; title = 'Source'; color = @(30, 90, 150); rect = [ordered]@{ x = 64; y = 64; width = 420; height = 240 }; monitor_key_utf16 = $null; collapsed = $false; items = $itemsSource },
            [ordered]@{ id = '2'; title = 'Destination'; color = @(70, 130, 90); rect = [ordered]@{ x = 64; y = 340; width = 420; height = 240 }; monitor_key_utf16 = $null; collapsed = $false; items = @($itemDest) }
        )
    }
    $primary = Join-Path $case 'config.json'
    Write-Utf8 $primary ($fixture | ConvertTo-Json -Depth 16 -Compress)
    $originalJson = [IO.File]::ReadAllText($primary)
    $expected = $fixture | ConvertTo-Json -Depth 16 -Compress | ConvertFrom-Json
    $before = Get-Content -LiteralPath $primary -Raw | ConvertFrom-Json
    Check (Same-Config $before $expected) "$($scenario.name): generated fixture differs from expected model."

    Run-Case "$($scenario.name)-hidden-smoke" $case $scenario.mode
    $afterSmoke = Get-Content -LiteralPath $primary -Raw | ConvertFrom-Json
    if ($scenario.expectedMove) {
        $movedItem = $expected.groups[0].items[0]
        $expected.groups[0].items = @($expected.groups[0].items | Where-Object { $_.id -cne '1' })
        $expected.groups[1].items = @($expected.groups[1].items) + @($movedItem)
        Check (Same-Config $afterSmoke $expected) "$($scenario.name): transfer changed fields, paths, IDs, or item order."
    } else {
        Check (Same-Config $afterSmoke $expected) "$($scenario.name): cancelled transfer changed configuration."
        Check ([IO.File]::ReadAllText($primary) -ceq $originalJson) "$($scenario.name): cancellation unexpectedly saved a snapshot."
        Check (-not (Test-Path -LiteralPath (Join-Path $case 'config.bak.json'))) "$($scenario.name): cancellation unexpectedly created a backup."
    }
    Check ($afterSmoke.next_group_id -ceq '3' -and $afterSmoke.next_item_id -ceq '5') "$($scenario.name): transfer changed ID allocators."
    foreach ($snapshot in $sourceSnapshots) {
        $currentBytes = [IO.File]::ReadAllBytes($snapshot.path)
        Check ((Get-FileHash -LiteralPath $snapshot.path -Algorithm SHA256).Hash -ceq $snapshot.hash -and
            [Convert]::ToBase64String($currentBytes) -ceq $snapshot.bytes) "$($scenario.name): source bytes changed: $($snapshot.path)"
    }

    Run-Case "$($scenario.name)-writable-restart" $case 'writable'
    $afterRestart = Get-Content -LiteralPath $primary -Raw | ConvertFrom-Json
    Check (Same-Config $afterRestart $expected) "$($scenario.name): persisted state differs after writable restart."
    foreach ($snapshot in $sourceSnapshots) {
        $currentBytes = [IO.File]::ReadAllBytes($snapshot.path)
        Check ((Get-FileHash -LiteralPath $snapshot.path -Algorithm SHA256).Hash -ceq $snapshot.hash -and
            [Convert]::ToBase64String($currentBytes) -ceq $snapshot.bytes) "$($scenario.name): source bytes changed after restart: $($snapshot.path)"
    }
}

$report = [ordered]@{
    test = 'Hidden native item-drag smoke scenarios; no physical desktop operation performed.'
    scenarios = @($scenarios | ForEach-Object { $_.name })
    executions = $results.ToArray()
    execution_count = $results.Count
    result = 'passed'
}
$json = $report | ConvertTo-Json -Depth 8
Write-Utf8 (Join-Path $root 'results.json') $json
$summary = "Item drag hidden smoke: PASS`nScenarios: $($scenarios.Count)`nChild executions: $($results.Count) (hidden smoke + writable restart per scenario)`nPhysical desktop operation: not performed`nArtifacts: $root`n"
Write-Utf8 (Join-Path $root 'summary.txt') $summary
Write-Output $json
Write-Output "Artifacts retained: $root"

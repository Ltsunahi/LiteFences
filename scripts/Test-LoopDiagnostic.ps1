$ErrorActionPreference = 'Stop'

$projectRoot = [IO.Path]::GetDirectoryName($PSScriptRoot)
$executable = Join-Path $projectRoot 'target\release\loop-diagnostic.exe'
if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
    throw "Build loop-diagnostic first: $executable"
}

$artifactsRoot = Join-Path $projectRoot 'artifacts'
$stamp = [DateTime]::UtcNow.Ticks
$diagnosticDirectory = Join-Path $artifactsRoot ("loop-script-check-{0}-{1}" -f $PID, $stamp)
$null = New-Item -ItemType Directory -Path $diagnosticDirectory

function Invoke-LoopDiagnostic {
    param(
        [Parameter(Mandatory)] [string]$Name,
        [Parameter(Mandatory)] [string[]]$Arguments
    )

    $stdoutPath = Join-Path $diagnosticDirectory "$Name.stdout.json"
    $stderrPath = Join-Path $diagnosticDirectory "$Name.stderr.txt"
    $child = Start-Process -FilePath $executable -ArgumentList $Arguments -WindowStyle Hidden `
        -PassThru -RedirectStandardOutput $stdoutPath -RedirectStandardError $stderrPath
    try {
        if (-not $child.WaitForExit(10000)) {
            # Stop only the exact process started by this invocation.
            $child.Kill()
            $child.WaitForExit()
            throw "$Name timed out after 10 seconds; diagnostics retained in $diagnosticDirectory."
        }
        $child.WaitForExit()
        $child.Refresh()
        $capturedOutput = [IO.File]::ReadAllText($stdoutPath).Trim()
        $capturedError = [IO.File]::ReadAllText($stderrPath).Trim()
        [pscustomobject]@{
            exit_code = $child.ExitCode
            stdout = $capturedOutput
            stderr = $capturedError
            stdout_path = $stdoutPath
            stderr_path = $stderrPath
        }
    } finally {
        $child.Dispose()
    }
}

function Assert-Condition {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw $Message }
}

$beforeRoots = @()
if (Test-Path -LiteralPath $artifactsRoot -PathType Container) {
    $beforeRoots = @(Get-ChildItem -LiteralPath $artifactsRoot -Directory -Filter 'loop-diagnostic-*' |
        ForEach-Object { $_.FullName })
}

$positive = Invoke-LoopDiagnostic -Name 'positive' -Arguments @('--seconds', '1')
Assert-Condition ($positive.exit_code -eq 0) "Positive run exited $($positive.exit_code): $($positive.stderr)"
Assert-Condition ((@($positive.stdout -split "`r?`n").Count) -eq 1 -and $positive.stdout.Length -gt 0) 'Positive stdout must be one JSON line.'
try { $report = $positive.stdout | ConvertFrom-Json } catch { throw "Positive stdout was not valid JSON: $($_.Exception.Message)" }

$requiredFields = @('schema_version','process_id','data_dir','status','seconds_requested','hidden_requested',
    'elapsed_seconds','cpu_seconds_delta','logical_processors','cpu_normalized_percent','counters')
foreach ($field in $requiredFields) {
    Assert-Condition ($report.PSObject.Properties.Name -contains $field) "Report is missing '$field'."
}
Assert-Condition ($report.schema_version -eq 1) 'Unexpected schema_version.'
Assert-Condition ($report.process_id -is [ValueType] -and $report.process_id -gt 0) 'Invalid process_id.'
Assert-Condition ($report.status -ceq 'completed') 'Diagnostic did not complete.'
Assert-Condition ($report.timeout_fired -eq $true) 'Diagnostic ended without its timed close request.'
Assert-Condition ($report.seconds_requested -eq 1) 'Diagnostic did not honor --seconds 1.'
Assert-Condition ($report.hidden_requested -eq $true) 'Diagnostic was not hidden.'
Assert-Condition ($report.initial_window.visible -eq $false -and $report.closing_window.visible -eq $false) 'Diagnostic window unexpectedly became visible.'
Assert-Condition ($report.elapsed_seconds -is [ValueType] -and $report.elapsed_seconds -gt 0) 'Invalid elapsed_seconds.'
Assert-Condition ($report.cpu_seconds_delta -is [ValueType] -and $report.cpu_seconds_delta -ge 0) 'Invalid cpu_seconds_delta.'
Assert-Condition ($report.logical_processors -is [ValueType] -and $report.logical_processors -gt 0) 'Invalid logical_processors.'
Assert-Condition ($null -ne $report.cpu_normalized_percent) 'cpu_normalized_percent is null.'
Assert-Condition ($null -ne $report.environment) 'Environment diagnostics are missing.'
foreach ($field in @('station_name','desktop_name')) {
    Assert-Condition ($report.environment.PSObject.Properties.Name -contains $field) "Environment $field is missing."
    Assert-Condition (($null -eq $report.environment.$field) -or ($report.environment.$field -is [string])) "Environment $field must be null or a string."
}
Assert-Condition ($report.environment.PSObject.Properties.Name -contains 'receives_input') 'Environment receives_input is missing.'
Assert-Condition (($null -eq $report.environment.receives_input) -or ($report.environment.receives_input -is [bool])) 'Environment receives_input must be null or boolean.'

$dataDir = [IO.Path]::GetFullPath([string]$report.data_dir)
$expectedParent = [IO.Path]::GetFullPath($artifactsRoot).TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
Assert-Condition ($dataDir.StartsWith($expectedParent, [StringComparison]::OrdinalIgnoreCase)) 'data_dir is outside the project artifacts root.'
Assert-Condition ([IO.Path]::GetDirectoryName($dataDir).TrimEnd([IO.Path]::DirectorySeparatorChar).Equals(
    [IO.Path]::GetFullPath($artifactsRoot).TrimEnd([IO.Path]::DirectorySeparatorChar), [StringComparison]::OrdinalIgnoreCase)) 'data_dir is not a direct artifacts child.'
Assert-Condition ([IO.Path]::GetFileName($dataDir) -match ('^loop-diagnostic-' + [regex]::Escape([string]$report.process_id) + '-\d+$')) 'data_dir name does not match the expected isolated fixture.'
$reportPath = Join-Path $dataDir 'loop-report.json'
$configPath = Join-Path $dataDir 'config.json'
Assert-Condition (Test-Path -LiteralPath $reportPath -PathType Leaf) 'Persistent loop-report.json is missing.'
Assert-Condition (Test-Path -LiteralPath $configPath -PathType Leaf) 'Fixture config.json is missing.'
Assert-Condition (-not (Test-Path -LiteralPath (Join-Path $dataDir 'config.pending'))) 'Fixture has an unfinished pending save.'
$persisted = Get-Content -LiteralPath $reportPath -Raw | ConvertFrom-Json
Assert-Condition ($persisted.schema_version -eq 1 -and $persisted.process_id -eq $report.process_id) 'Persisted report does not match stdout.'
Assert-Condition ($persisted.data_dir -eq $report.data_dir) 'Persisted report data_dir does not match stdout.'
Assert-Condition ($persisted.status -ceq 'completed') 'Persisted report is not completed.'
Assert-Condition ($persisted.seconds_requested -eq 1 -and $persisted.hidden_requested -eq $true) 'Persisted report has unexpected run settings.'

$config = Get-Content -LiteralPath $configPath -Raw | ConvertFrom-Json
$groups = @($config.groups)
Assert-Condition ($groups.Count -eq 1) 'Fixture must contain exactly one group.'
$references = 0
foreach ($group in $groups) {
    $references += @($group.items).Count
}
Assert-Condition ($references -eq 0) 'Fixture group must contain zero references.'

Assert-Condition ($report.status -ceq 'completed' -and $persisted.status -ceq 'completed') 'Diagnostic is pending or incomplete.'
$counterNames = @('wait_worker','wait_input','wait_other','session_changes','icon_changes','window_messages','paints','refresh_timers','cross_thread_messages','cross_thread_getobject')
foreach ($name in $counterNames) {
    Assert-Condition ($report.counters.PSObject.Properties.Name -contains $name) "Counters are missing '$name'."
    Assert-Condition ($report.counters.$name -is [ValueType] -and $report.counters.$name -ge 0) "Counter '$name' is invalid."
}
Assert-Condition ($report.counters.session_changes -ge 2) 'Expected at least two session changes.'
Assert-Condition ($report.counters.icon_changes -eq 0) 'Unexpected icon changes were observed.'
Assert-Condition ($report.counters.cross_thread_getobject -le $report.counters.cross_thread_messages -and $report.counters.cross_thread_messages -le $report.counters.window_messages) 'Cross-thread message counts exceed their containing totals.'
Assert-Condition ($report.PSObject.Properties.Name -contains 'ui_thread_cpu_seconds_delta') 'UI thread CPU field is missing.'
Assert-Condition ($null -eq $report.ui_thread_cpu_seconds_delta -or ($report.ui_thread_cpu_seconds_delta -is [ValueType] -and $report.ui_thread_cpu_seconds_delta -ge 0)) 'UI thread CPU must be null or nonnegative.'
$histogramFields = @('top_parent_messages','parent_message_kinds','parent_message_list_truncated','high_parent_message_ids')
foreach ($field in $histogramFields) {
    Assert-Condition ($report.PSObject.Properties.Name -contains $field) "Report is missing '$field'."
}
Assert-Condition ($report.parent_message_kinds -is [ValueType] -and $report.parent_message_kinds -ge 0 -and $report.parent_message_kinds -le 1024) 'parent_message_kinds must be between 0 and 1024.'
Assert-Condition ($report.parent_message_list_truncated -is [bool]) 'parent_message_list_truncated must be boolean.'
Assert-Condition ($report.high_parent_message_ids -is [ValueType] -and $report.high_parent_message_ids -ge 0) 'high_parent_message_ids must be nonnegative.'
$topMessages = @($report.top_parent_messages)
$expectedTopCount = [Math]::Min([int]$report.parent_message_kinds, 32)
Assert-Condition ($topMessages.Count -eq $expectedTopCount) 'top_parent_messages length does not match the bounded histogram size.'
Assert-Condition ($report.parent_message_list_truncated -eq ($report.parent_message_kinds -gt 32)) 'parent_message_list_truncated does not match parent_message_kinds.'
$seenMessageIds = [Collections.Generic.HashSet[long]]::new()
$listedMessageCount = [decimal]0
$previousCount = [decimal]::MaxValue
$previousId = [long]-1
foreach ($message in $topMessages) {
    foreach ($field in @('message_id','message_id_hex','count')) {
        Assert-Condition ($message.PSObject.Properties.Name -contains $field) "A top message is missing '$field'."
    }
    Assert-Condition ($message.message_id -is [ValueType] -and $message.message_id -ge 0 -and $message.message_id -le 1023) 'A message_id is outside 0..1023.'
    Assert-Condition ($message.count -is [ValueType] -and $message.count -gt 0) 'A message count must be positive.'
    $messageId = [long]$message.message_id
    Assert-Condition ($seenMessageIds.Add($messageId)) "Duplicate message_id $messageId."
    $hex = [string]$message.message_id_hex
    Assert-Condition ($hex -match '^(?:0[xX])?[0-9a-fA-F]+$') "Invalid message_id_hex '$hex'."
    $hexValue = [Convert]::ToInt64(($hex -replace '^0[xX]', ''), 16)
    Assert-Condition ($hexValue -eq $messageId) "message_id_hex does not match message_id $messageId."
    $messageCount = [decimal]$message.count
    Assert-Condition (($messageCount -lt $previousCount) -or (($messageCount -eq $previousCount) -and ($messageId -gt $previousId))) 'top_parent_messages are not sorted by descending count, then ascending ID.'
    $previousCount = $messageCount
    $previousId = $messageId
    $listedMessageCount += [decimal]$messageCount
}
$allReportedMessages = $listedMessageCount + [decimal]$report.high_parent_message_ids
if ($report.parent_message_list_truncated) {
    Assert-Condition ($allReportedMessages -le [decimal]$report.counters.window_messages) 'Reported message histogram exceeds window_messages.'
} else {
    Assert-Condition ($allReportedMessages -eq [decimal]$report.counters.window_messages) 'Complete message histogram does not sum to window_messages.'
}
$waitTotal = [double]$report.counters.wait_worker + [double]$report.counters.wait_input + [double]$report.counters.wait_other
Assert-Condition ($waitTotal -gt 0) 'No wait events were recorded.'

$negativeCases = @(
    @{ name = 'seconds-zero'; args = @('--seconds', '0') },
    @{ name = 'data-dir'; args = @('--data-dir', 'user-placeholder') },
    @{ name = 'duplicate-seconds'; args = @('--seconds', '1', '--seconds', '2') }
)
$negativeResults = @()
foreach ($case in $negativeCases) {
    $result = Invoke-LoopDiagnostic -Name $case.name -Arguments $case.args
    Assert-Condition ($result.exit_code -eq 2) "$($case.name) should exit 2, got $($result.exit_code)."
    $negativeResults += [pscustomobject]@{ name = $case.name; exit_code = $result.exit_code }
}
$afterRoots = @()
if (Test-Path -LiteralPath $artifactsRoot -PathType Container) {
    $afterRoots = @(Get-ChildItem -LiteralPath $artifactsRoot -Directory -Filter 'loop-diagnostic-*' |
        ForEach-Object { $_.FullName })
}
$newFixtureRoots = @($afterRoots | Where-Object { $_ -notin $beforeRoots })
Assert-Condition ($newFixtureRoots.Count -eq 1 -and $newFixtureRoots[0] -eq $dataDir) 'Negative cases created an unexpected fixture root.'

[pscustomobject]@{
    schema_version = 1
    passed = $true
    positive = [pscustomobject]@{ process_id = $report.process_id; data_dir = $dataDir; status = $report.status; seconds_requested = $report.seconds_requested; hidden_requested = $report.hidden_requested; counters = $report.counters }
    negative_cases = $negativeResults
    diagnostics_dir = $diagnosticDirectory
} | ConvertTo-Json -Depth 8 -Compress

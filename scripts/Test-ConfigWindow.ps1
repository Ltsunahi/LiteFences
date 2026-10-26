[CmdletBinding()]
param([string]$ExecutablePath)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path $PSScriptRoot -Parent
$executable = if ($ExecutablePath) { [IO.Path]::GetFullPath($ExecutablePath) } else { Join-Path $projectRoot 'target\release\litefences.exe' }
if (-not (Test-Path -LiteralPath $executable)) { throw 'Build release with config-store before testing.' }
$root = Join-Path $projectRoot ('artifacts\config-window-' + [guid]::NewGuid().ToString('N'))
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
function Same-EmptyGroup($actual, $expected) {
    return $actual.id -ceq $expected.id -and $actual.title -ceq $expected.title -and
        $actual.rect.x -eq $expected.rect.x -and $actual.rect.y -eq $expected.rect.y -and
        $actual.rect.width -eq $expected.rect.width -and $actual.rect.height -eq $expected.rect.height -and
        $actual.collapsed -eq $expected.collapsed -and
        $null -eq $actual.monitor_key_utf16 -and $null -eq $expected.monitor_key_utf16 -and
        @($actual.items).Count -eq 0 -and @($expected.items).Count -eq 0 -and
        (ConvertTo-Json -InputObject $actual.color -Compress) -ceq (ConvertTo-Json -InputObject $expected.color -Compress)
}
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
function Run-Case([string]$name, [string]$path, [string]$expected, [int]$exitCode = 0) {
    $watch = [Diagnostics.Stopwatch]::StartNew()
    $process = Start-Process -FilePath $executable -WindowStyle Hidden -PassThru -ArgumentList @('--data-dir', ('"' + $path + '"'), '--storage-smoke', $expected)
    try {
        if (-not $process.WaitForExit(15000)) {
            $process.Kill() # Only the exact diagnostic child launched above.
            $process.WaitForExit()
            throw "$name timed out; artifacts retained at $root"
        }
        $process.Refresh()
        Check ($process.ExitCode -eq $exitCode) "$name exit $($process.ExitCode), expected $exitCode"
        $results.Add([ordered]@{ name = $name; exit_code = $process.ExitCode; elapsed_ms = $watch.Elapsed.TotalMilliseconds })
    } finally { $process.Dispose() }
}
$empty = New-Case 'empty with spaces'
Run-Case 'empty-save-close' $empty 'writable'
$primary = Join-Path $empty 'config.json'
Check (Test-Path -LiteralPath $primary) 'Empty config was not saved.'
$valid = [IO.File]::ReadAllText($primary)
Run-Case 'reopen-save-close' $empty 'writable'
Check ([IO.File]::ReadAllText((Join-Path $empty 'config.bak.json')) -ceq $valid) 'Backup rotation mismatch.'

$populated = New-Case 'populated'
$source = Join-Path $populated 'source.txt'
Write-Utf8 $source 'Source content must not change.'
$sourceHash = (Get-FileHash -LiteralPath $source).Hash
$fixture = [ordered]@{
    schema_version = 1; next_group_id = '2'; next_item_id = '2'
    settings = @{ launch_at_login = $false }
    groups = @(@{ id = '1'; title = '测试分组'; color = @(30,60,90)
        rect = @{ x = 0; y = 0; width = 360; height = 240 }; monitor_key_utf16 = $null; collapsed = $false
        items = @(@{ id = '1'; target = @{ kind = 'file'; path_utf16 = @($source.ToCharArray() | ForEach-Object { [int]$_ }) } }) })
}
$populatedPrimary = Join-Path $populated 'config.json'
Write-Utf8 $populatedPrimary ($fixture | ConvertTo-Json -Depth 12 -Compress)
Run-Case 'populated-roundtrip' $populated 'writable'
$roundtrip = Get-Content -LiteralPath $populatedPrimary -Raw | ConvertFrom-Json
Check ($roundtrip.groups.Count -eq 1 -and $roundtrip.groups[0].title -ceq '测试分组' -and $roundtrip.groups[0].items.Count -eq 1) 'Group/reference roundtrip failed.'
Check ((Get-FileHash -LiteralPath $source).Hash -ceq $sourceHash) 'Referenced source changed.'

$recovery = New-Case 'recovery'
Write-Utf8 (Join-Path $recovery 'config.json') '{broken'
Write-Utf8 (Join-Path $recovery 'config.bak.json') $valid
Run-Case 'corrupt-primary-recovery' $recovery 'writable'
Check ([IO.File]::ReadAllText((Join-Path $recovery 'config.corrupt.json')) -ceq '{broken') 'Bad primary not preserved exactly.'

foreach ($scenario in @('future','pending','both-bad')) {
    $path = New-Case $scenario
    $content = if ($scenario -eq 'future') { '{"schema_version":2}' } elseif ($scenario -eq 'both-bad') { '{broken' } else { $valid }
    $file = Join-Path $path 'config.json'
    Write-Utf8 $file $content
    if ($scenario -eq 'pending') { Write-Utf8 (Join-Path $path 'config.pending') 'uncommitted' }
    $before = (Get-FileHash -LiteralPath $file).Hash
    Run-Case $scenario $path 'readonly'
    Check ((Get-FileHash -LiteralPath $file).Hash -ceq $before) "$scenario changed original bytes."
}
$missing = Join-Path $root 'missing'
Run-Case 'missing-directory' $missing 'error'
Check (-not (Test-Path -LiteralPath $missing)) 'Explicit missing directory was created.'
$busy = New-Case 'busy'
$lock = [IO.File]::Open((Join-Path $busy '.writer.lock'), [IO.FileMode]::CreateNew, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
try { Run-Case 'busy-writer' $busy 'error' } finally { $lock.Dispose() }
Run-Case 'wrong-expectation-fails' $empty 'error' 1
$logUnknown = New-Case 'unknown-log'
Write-Utf8 (Join-Path $logUnknown 'diagnostics.log') 'Preserve unrelated log bytes.'
Run-Case 'unavailable-log-still-saves' $logUnknown 'writable'
Check ([IO.File]::ReadAllText((Join-Path $logUnknown 'diagnostics.log')) -ceq 'Preserve unrelated log bytes.') 'Unknown log was modified.'
$logFull = New-Case 'full-log'
$header = "LiteFences diagnostics v1`n"
$fullContent = $header + ('x' * (65536 - $header.Length - 1)) + "`n"
Write-Utf8 (Join-Path $logFull 'diagnostics.log') $fullContent
Run-Case 'full-log-still-saves' $logFull 'writable'
Check ([IO.File]::ReadAllText((Join-Path $logFull 'diagnostics.log')) -ceq $fullContent) 'Full log was modified.'
Check ([IO.File]::ReadAllText((Join-Path $empty 'diagnostics.log')).Contains('save revision=1 result=ok')) 'Worker diagnostic save record absent.'
$groups = New-Case 'group-create'
Run-Case 'create-group-and-close' $groups 'group'
$groupConfig = Get-Content -LiteralPath (Join-Path $groups 'config.json') -Raw | ConvertFrom-Json
Check ($groupConfig.groups.Count -eq 1 -and $groupConfig.groups[0].id -ceq '1' -and $groupConfig.groups[0].title -ceq '新分组 1' -and $groupConfig.next_group_id -ceq '2') 'Auto-saved group identity mismatch.'
Run-Case 'reopen-existing-group' $groups 'writable'
$reopenedGroup = Get-Content -LiteralPath (Join-Path $groups 'config.json') -Raw | ConvertFrom-Json
Check ($reopenedGroup.groups.Count -eq 1 -and $reopenedGroup.groups[0].id -ceq '1') 'Group was not restored.'
Run-Case 'rename-existing-group' $groups 'rename'
$renamedGroup = Get-Content -LiteralPath (Join-Path $groups 'config.json') -Raw | ConvertFrom-Json
Check ($renamedGroup.groups.Count -eq 1 -and $renamedGroup.groups[0].id -ceq '1' -and $renamedGroup.groups[0].title -ceq '已重命名的分组' -and $renamedGroup.next_group_id -ceq '2') 'Renamed group identity or title mismatch.'
Run-Case 'reopen-renamed-group' $groups 'writable'
$renamedReopen = Get-Content -LiteralPath (Join-Path $groups 'config.json') -Raw | ConvertFrom-Json
Check ($renamedReopen.groups.Count -eq 1 -and $renamedReopen.groups[0].title -ceq '已重命名的分组') 'Renamed group was not restored.'
$multiRename = New-Case 'rename-multiple-groups'
$multiFixture = [ordered]@{
    schema_version = 1; next_group_id = '8'; next_item_id = '1'
    settings = @{ launch_at_login = $false }
    groups = @(
        @{ id = '1'; title = '待重命名的第一组'; color = @(16,128,240)
            rect = @{ x = 0; y = 0; width = 360; height = 240 }; monitor_key_utf16 = $null; collapsed = $false
            items = @() },
        @{ id = '7'; title = '保持不变的第二组'; color = @(20,40,60)
            rect = @{ x = 400; y = 0; width = 360; height = 240 }; monitor_key_utf16 = $null; collapsed = $false
            items = @() }
    )
}
$uiRemove = New-Case 'storage-smoke-ui-remove'
$removeSource = Join-Path $uiRemove 'source.txt'
$removeSourceContent = 'UI remove must preserve source bytes.'
Write-Utf8 $removeSource $removeSourceContent
$removeSourceHash = (Get-FileHash -LiteralPath $removeSource -Algorithm SHA256).Hash
$removeFixture = $multiFixture | ConvertTo-Json -Depth 12 -Compress | ConvertFrom-Json
$removeFixture.next_item_id = '2'
$removeFixture.groups[1].items = @([pscustomobject]@{
    id = '1'
    target = [pscustomobject]@{
        kind = 'file'
        path_utf16 = @($removeSource.ToCharArray() | ForEach-Object { [int]$_ })
    }
})
$removePrimary = Join-Path $uiRemove 'config.json'
Write-Utf8 $removePrimary ($removeFixture | ConvertTo-Json -Depth 12 -Compress)
$removeBefore = Get-Content -LiteralPath $removePrimary -Raw | ConvertFrom-Json
Check ($removeBefore.groups.Count -eq 2 -and $removeBefore.groups[0].id -ceq '1' -and $removeBefore.groups[0].items.Count -eq 0 -and $removeBefore.groups[1].id -ceq '7' -and $removeBefore.groups[1].items.Count -eq 1 -and $removeBefore.groups[1].items[0].id -ceq '1' -and $removeBefore.next_item_id -ceq '2') 'UI remove fixture is invalid.'
$removeStoredSource = -join @($removeBefore.groups[1].items[0].target.path_utf16 | ForEach-Object { [char][int]$_ })
Check ($removeStoredSource -ceq $removeSource) 'UI remove fixture stored the wrong source path.'
Run-Case 'storage-smoke-ui-remove-second-group-reference' $uiRemove 'ui-remove'
$removeAfter = Get-Content -LiteralPath $removePrimary -Raw | ConvertFrom-Json
Check ($removeAfter.groups.Count -eq 2 -and $removeAfter.next_group_id -ceq '8' -and $removeAfter.next_item_id -ceq '2') 'UI remove changed group count or ID allocators.'
Check (Same-EmptyGroup $removeAfter.groups[0] $removeFixture.groups[0]) 'UI remove changed the first group.'
Check ($removeAfter.groups[1].id -ceq '7' -and $removeAfter.groups[1].title -ceq $multiFixture.groups[1].title -and $removeAfter.groups[1].rect.x -eq 400 -and $removeAfter.groups[1].rect.y -eq 0 -and $removeAfter.groups[1].rect.width -eq 360 -and $removeAfter.groups[1].rect.height -eq 240 -and $removeAfter.groups[1].collapsed -eq $false -and $removeAfter.groups[1].items.Count -eq 0) 'UI remove changed second-group fields or retained its reference.'
Check ((ConvertTo-Json -InputObject $removeAfter.groups[1].color -Compress) -ceq (ConvertTo-Json -InputObject $multiFixture.groups[1].color -Compress)) 'UI remove changed the second-group color.'
Check ((Get-FileHash -LiteralPath $removeSource -Algorithm SHA256).Hash -ceq $removeSourceHash -and [IO.File]::ReadAllText($removeSource) -ceq $removeSourceContent) 'UI remove changed source.txt bytes.'
Run-Case 'reopen-storage-smoke-ui-remove' $uiRemove 'writable'
$removeReopen = Get-Content -LiteralPath $removePrimary -Raw | ConvertFrom-Json
Check ($removeReopen.groups.Count -eq 2 -and $removeReopen.next_group_id -ceq '8' -and $removeReopen.next_item_id -ceq '2') 'UI remove state or ID allocators did not survive restart.'
Check (Same-EmptyGroup $removeReopen.groups[0] $removeFixture.groups[0]) 'First group did not survive UI-remove restart unchanged.'
Check ($removeReopen.groups[1].id -ceq '7' -and $removeReopen.groups[1].title -ceq $multiFixture.groups[1].title -and $removeReopen.groups[1].rect.x -eq 400 -and $removeReopen.groups[1].rect.y -eq 0 -and $removeReopen.groups[1].rect.width -eq 360 -and $removeReopen.groups[1].rect.height -eq 240 -and $removeReopen.groups[1].collapsed -eq $false -and $removeReopen.groups[1].items.Count -eq 0) 'Second group or removal state did not survive UI-remove restart.'
Check ((ConvertTo-Json -InputObject $removeReopen.groups[1].color -Compress) -ceq (ConvertTo-Json -InputObject $multiFixture.groups[1].color -Compress)) 'Second-group color did not survive UI-remove restart unchanged.'
Check ((Get-FileHash -LiteralPath $removeSource -Algorithm SHA256).Hash -ceq $removeSourceHash -and [IO.File]::ReadAllText($removeSource) -ceq $removeSourceContent) 'UI remove restart changed source.txt bytes.'
$undoRemove = New-Case 'storage-smoke-ui-undo-remove'
$undoRemoveSource = Join-Path $undoRemove 'source.txt'
$undoRemoveSourceContent = 'Undo remove must preserve this independent source.'
Write-Utf8 $undoRemoveSource $undoRemoveSourceContent
$undoRemoveSourceHash = (Get-FileHash -LiteralPath $undoRemoveSource -Algorithm SHA256).Hash
$undoRemoveFixture = $multiFixture | ConvertTo-Json -Depth 12 -Compress | ConvertFrom-Json
$undoRemoveFixture.next_item_id = '2'
$undoRemoveFixture.groups[1].items = @([pscustomobject]@{
    id = '1'
    target = [pscustomobject]@{
        kind = 'file'
        path_utf16 = @($undoRemoveSource.ToCharArray() | ForEach-Object { [int]$_ })
    }
})
$undoRemovePrimary = Join-Path $undoRemove 'config.json'
Write-Utf8 $undoRemovePrimary ($undoRemoveFixture | ConvertTo-Json -Depth 12 -Compress)
$undoRemoveBefore = Get-Content -LiteralPath $undoRemovePrimary -Raw | ConvertFrom-Json
Check ($undoRemoveBefore.groups.Count -eq 2 -and $undoRemoveBefore.next_group_id -ceq '8' -and $undoRemoveBefore.next_item_id -ceq '2' -and (Same-Group $undoRemoveBefore.groups[0] $undoRemoveFixture.groups[0]) -and (Same-Group $undoRemoveBefore.groups[1] $undoRemoveFixture.groups[1])) 'UI undo-remove fixture is invalid.'
Run-Case 'storage-smoke-ui-undo-remove-reference' $undoRemove 'ui-undo-remove'
$undoRemoveAfter = Get-Content -LiteralPath $undoRemovePrimary -Raw | ConvertFrom-Json
Check ($undoRemoveAfter.groups.Count -eq 2 -and $undoRemoveAfter.next_group_id -ceq '8' -and $undoRemoveAfter.next_item_id -ceq '2' -and (Same-Group $undoRemoveAfter.groups[0] $undoRemoveFixture.groups[0]) -and (Same-Group $undoRemoveAfter.groups[1] $undoRemoveFixture.groups[1])) 'UI undo-remove did not restore both groups, fields, reference, IDs, and order.'
Check ((Get-FileHash -LiteralPath $undoRemoveSource -Algorithm SHA256).Hash -ceq $undoRemoveSourceHash -and [IO.File]::ReadAllText($undoRemoveSource) -ceq $undoRemoveSourceContent) 'UI undo-remove changed its independent source.txt.'
Run-Case 'reopen-storage-smoke-ui-undo-remove' $undoRemove 'writable'
$undoRemoveReopen = Get-Content -LiteralPath $undoRemovePrimary -Raw | ConvertFrom-Json
Check ($undoRemoveReopen.groups.Count -eq 2 -and $undoRemoveReopen.next_group_id -ceq '8' -and $undoRemoveReopen.next_item_id -ceq '2' -and (Same-Group $undoRemoveReopen.groups[0] $undoRemoveFixture.groups[0]) -and (Same-Group $undoRemoveReopen.groups[1] $undoRemoveFixture.groups[1])) 'UI undo-remove state did not survive writable restart exactly.'
Check ((Get-FileHash -LiteralPath $undoRemoveSource -Algorithm SHA256).Hash -ceq $undoRemoveSourceHash -and [IO.File]::ReadAllText($undoRemoveSource) -ceq $undoRemoveSourceContent) 'UI undo-remove restart changed its independent source.txt.'
$undoDelete = New-Case 'storage-smoke-ui-undo-delete'
$undoDeleteSource = Join-Path $undoDelete 'source.txt'
$undoDeleteSourceContent = 'Undo delete must preserve this independent source.'
Write-Utf8 $undoDeleteSource $undoDeleteSourceContent
$undoDeleteSourceHash = (Get-FileHash -LiteralPath $undoDeleteSource -Algorithm SHA256).Hash
$undoDeleteFixture = $multiFixture | ConvertTo-Json -Depth 12 -Compress | ConvertFrom-Json
$undoDeleteFixture.next_item_id = '2'
$undoDeleteFixture.groups[1].items = @([pscustomobject]@{
    id = '1'
    target = [pscustomobject]@{
        kind = 'file'
        path_utf16 = @($undoDeleteSource.ToCharArray() | ForEach-Object { [int]$_ })
    }
})
$undoDeletePrimary = Join-Path $undoDelete 'config.json'
Write-Utf8 $undoDeletePrimary ($undoDeleteFixture | ConvertTo-Json -Depth 12 -Compress)
$undoDeleteBefore = Get-Content -LiteralPath $undoDeletePrimary -Raw | ConvertFrom-Json
Check ($undoDeleteBefore.groups.Count -eq 2 -and $undoDeleteBefore.next_group_id -ceq '8' -and $undoDeleteBefore.next_item_id -ceq '2' -and (Same-Group $undoDeleteBefore.groups[0] $undoDeleteFixture.groups[0]) -and (Same-Group $undoDeleteBefore.groups[1] $undoDeleteFixture.groups[1])) 'UI undo-delete fixture is invalid.'
Run-Case 'storage-smoke-ui-undo-delete-group' $undoDelete 'ui-undo-delete'
$undoDeleteAfter = Get-Content -LiteralPath $undoDeletePrimary -Raw | ConvertFrom-Json
Check ($undoDeleteAfter.groups.Count -eq 2 -and $undoDeleteAfter.next_group_id -ceq '8' -and $undoDeleteAfter.next_item_id -ceq '2' -and (Same-Group $undoDeleteAfter.groups[0] $undoDeleteFixture.groups[0]) -and (Same-Group $undoDeleteAfter.groups[1] $undoDeleteFixture.groups[1])) 'UI undo-delete did not restore both groups, fields, reference, IDs, and order.'
Check ((Get-FileHash -LiteralPath $undoDeleteSource -Algorithm SHA256).Hash -ceq $undoDeleteSourceHash -and [IO.File]::ReadAllText($undoDeleteSource) -ceq $undoDeleteSourceContent) 'UI undo-delete changed its independent source.txt.'
Run-Case 'reopen-storage-smoke-ui-undo-delete' $undoDelete 'writable'
$undoDeleteReopen = Get-Content -LiteralPath $undoDeletePrimary -Raw | ConvertFrom-Json
Check ($undoDeleteReopen.groups.Count -eq 2 -and $undoDeleteReopen.next_group_id -ceq '8' -and $undoDeleteReopen.next_item_id -ceq '2' -and (Same-Group $undoDeleteReopen.groups[0] $undoDeleteFixture.groups[0]) -and (Same-Group $undoDeleteReopen.groups[1] $undoDeleteFixture.groups[1])) 'UI undo-delete state did not survive writable restart exactly.'
Check ((Get-FileHash -LiteralPath $undoDeleteSource -Algorithm SHA256).Hash -ceq $undoDeleteSourceHash -and [IO.File]::ReadAllText($undoDeleteSource) -ceq $undoDeleteSourceContent) 'UI undo-delete restart changed its independent source.txt.'
$multiPrimary = Join-Path $multiRename 'config.json'
Write-Utf8 $multiPrimary ($multiFixture | ConvertTo-Json -Depth 12 -Compress)
$multiBefore = Get-Content -LiteralPath $multiPrimary -Raw | ConvertFrom-Json
Check ($multiBefore.groups.Count -eq 2 -and $multiBefore.groups[0].id -ceq '1' -and $multiBefore.groups[0].title -ceq '待重命名的第一组' -and $multiBefore.groups[1].id -ceq '7' -and $multiBefore.groups[1].title -ceq '保持不变的第二组' -and $multiBefore.next_group_id -ceq '8') 'Multi-group rename fixture is invalid.'
Run-Case 'rename-first-of-multiple-groups' $multiRename 'rename'
$multiAfter = Get-Content -LiteralPath $multiPrimary -Raw | ConvertFrom-Json
Check ($multiAfter.groups.Count -eq 2 -and $multiAfter.groups[0].id -ceq '1' -and $multiAfter.groups[0].title -ceq '已重命名的分组' -and $multiAfter.groups[1].id -ceq '7' -and $multiAfter.groups[1].title -ceq '保持不变的第二组' -and $multiAfter.next_group_id -ceq '8') 'Multi-group rename changed an unexpected identity, title, or allocator.'
Run-Case 'reopen-renamed-multiple-groups' $multiRename 'writable'
$multiReopen = Get-Content -LiteralPath $multiPrimary -Raw | ConvertFrom-Json
Check ($multiReopen.groups.Count -eq 2 -and $multiReopen.groups[0].id -ceq '1' -and $multiReopen.groups[0].title -ceq '已重命名的分组' -and $multiReopen.groups[1].id -ceq '7' -and $multiReopen.groups[1].title -ceq '保持不变的第二组' -and $multiReopen.next_group_id -ceq '8') 'Multi-group rename state did not survive restart.'
$uiRename = New-Case 'ui-rename-second-group'
Write-Utf8 (Join-Path $uiRename 'config.json') ($multiFixture | ConvertTo-Json -Depth 12 -Compress)
Run-Case 'ui-controls-rename-second-group' $uiRename 'ui-rename'
$uiRenamed = Get-Content -LiteralPath (Join-Path $uiRename 'config.json') -Raw | ConvertFrom-Json
Check ($uiRenamed.groups.Count -eq 2 -and $uiRenamed.groups[0].id -ceq '1' -and $uiRenamed.groups[0].title -ceq '待重命名的第一组' -and $uiRenamed.groups[1].id -ceq '7' -and $uiRenamed.groups[1].title -ceq '第二组已改名' -and $uiRenamed.next_group_id -ceq '8') 'Native controls renamed the wrong group or changed identity.'
Run-Case 'reopen-ui-renamed-group' $uiRename 'writable'
$uiReopen = Get-Content -LiteralPath (Join-Path $uiRename 'config.json') -Raw | ConvertFrom-Json
Check ($uiReopen.groups.Count -eq 2 -and $uiReopen.groups[0].title -ceq '待重命名的第一组' -and $uiReopen.groups[1].id -ceq '7' -and $uiReopen.groups[1].title -ceq '第二组已改名') 'Native-control rename was not restored.'
$color = New-Case 'storage-smoke-color'
Write-Utf8 (Join-Path $color 'config.json') ($multiFixture | ConvertTo-Json -Depth 12 -Compress)
Run-Case 'storage-smoke-first-group-color' $color 'color'
$colorAfter = Get-Content -LiteralPath (Join-Path $color 'config.json') -Raw | ConvertFrom-Json
Check ($colorAfter.groups.Count -eq 2 -and $colorAfter.next_group_id -ceq '8' -and $colorAfter.next_item_id -ceq '1') 'Color smoke changed group count or ID allocators.'
Check ($colorAfter.groups[0].id -ceq '1' -and $colorAfter.groups[0].title -ceq '待重命名的第一组' -and $colorAfter.groups[0].rect.x -eq 0 -and $colorAfter.groups[0].rect.y -eq 0 -and $colorAfter.groups[0].rect.width -eq 360 -and $colorAfter.groups[0].rect.height -eq 240 -and $colorAfter.groups[0].collapsed -eq $false -and $colorAfter.groups[0].items.Count -eq 0) 'Color smoke changed first-group fields other than color.'
Check ($colorAfter.groups[1].id -ceq '7' -and $colorAfter.groups[1].title -ceq '保持不变的第二组' -and $colorAfter.groups[1].rect.x -eq 400 -and $colorAfter.groups[1].rect.y -eq 0 -and $colorAfter.groups[1].rect.width -eq 360 -and $colorAfter.groups[1].rect.height -eq 240 -and $colorAfter.groups[1].collapsed -eq $false -and $colorAfter.groups[1].items.Count -eq 0) 'Color smoke changed the second group.'
Check ((ConvertTo-Json -InputObject $colorAfter.groups[0].color -Compress) -ceq (ConvertTo-Json -InputObject @(16,128,240) -Compress)) 'Color smoke did not set the first group RGB.'
Check ((ConvertTo-Json -InputObject $colorAfter.groups[1].color -Compress) -ceq (ConvertTo-Json -InputObject $multiFixture.groups[1].color -Compress)) 'Color smoke changed the non-target group color.'
Run-Case 'reopen-storage-smoke-color' $color 'writable'
$colorReopen = Get-Content -LiteralPath (Join-Path $color 'config.json') -Raw | ConvertFrom-Json
Check ($colorReopen.groups.Count -eq 2 -and $colorReopen.next_group_id -ceq '8' -and $colorReopen.next_item_id -ceq '1') 'Color smoke state or allocators did not survive restart.'
Check ($colorReopen.groups[0].id -ceq '1' -and $colorReopen.groups[0].title -ceq '待重命名的第一组' -and $colorReopen.groups[0].rect.x -eq 0 -and $colorReopen.groups[0].rect.y -eq 0 -and $colorReopen.groups[0].rect.width -eq 360 -and $colorReopen.groups[0].rect.height -eq 240 -and $colorReopen.groups[0].collapsed -eq $false -and $colorReopen.groups[0].items.Count -eq 0) 'First-group fields did not survive color-smoke restart.'
Check ($colorReopen.groups[1].id -ceq '7' -and $colorReopen.groups[1].title -ceq '保持不变的第二组' -and $colorReopen.groups[1].rect.x -eq 400 -and $colorReopen.groups[1].rect.y -eq 0 -and $colorReopen.groups[1].rect.width -eq 360 -and $colorReopen.groups[1].rect.height -eq 240 -and $colorReopen.groups[1].collapsed -eq $false -and $colorReopen.groups[1].items.Count -eq 0) 'Second group did not survive color-smoke restart unchanged.'
Check ((ConvertTo-Json -InputObject $colorReopen.groups[0].color -Compress) -ceq (ConvertTo-Json -InputObject @(16,128,240) -Compress)) 'First-group RGB did not survive color-smoke restart.'
Check ((ConvertTo-Json -InputObject $colorReopen.groups[1].color -Compress) -ceq (ConvertTo-Json -InputObject $multiFixture.groups[1].color -Compress)) 'Second-group color did not survive color-smoke restart unchanged.'
$uiColor = New-Case 'storage-smoke-ui-color'
Write-Utf8 (Join-Path $uiColor 'config.json') ($multiFixture | ConvertTo-Json -Depth 12 -Compress)
Run-Case 'storage-smoke-ui-second-group-color' $uiColor 'ui-color'
$uiColorAfter = Get-Content -LiteralPath (Join-Path $uiColor 'config.json') -Raw | ConvertFrom-Json
Check ($uiColorAfter.groups.Count -eq 2 -and $uiColorAfter.next_group_id -ceq '8' -and $uiColorAfter.next_item_id -ceq '1') 'UI color smoke changed group count or ID allocators.'
Check ($uiColorAfter.groups[0].id -ceq '1' -and $uiColorAfter.groups[0].title -ceq '待重命名的第一组' -and $uiColorAfter.groups[0].rect.x -eq 0 -and $uiColorAfter.groups[0].rect.y -eq 0 -and $uiColorAfter.groups[0].rect.width -eq 360 -and $uiColorAfter.groups[0].rect.height -eq 240 -and $uiColorAfter.groups[0].collapsed -eq $false -and $uiColorAfter.groups[0].items.Count -eq 0) 'UI color smoke changed the first group.'
Check ($uiColorAfter.groups[1].id -ceq '7' -and $uiColorAfter.groups[1].title -ceq '保持不变的第二组' -and $uiColorAfter.groups[1].rect.x -eq 400 -and $uiColorAfter.groups[1].rect.y -eq 0 -and $uiColorAfter.groups[1].rect.width -eq 360 -and $uiColorAfter.groups[1].rect.height -eq 240 -and $uiColorAfter.groups[1].collapsed -eq $false -and $uiColorAfter.groups[1].items.Count -eq 0) 'UI color smoke changed second-group fields other than color.'
Check ((ConvertTo-Json -InputObject $uiColorAfter.groups[0].color -Compress) -ceq (ConvertTo-Json -InputObject $multiFixture.groups[0].color -Compress)) 'UI color smoke changed the non-target group color.'
Check ((ConvertTo-Json -InputObject $uiColorAfter.groups[1].color -Compress) -ceq (ConvertTo-Json -InputObject @(232,93,117) -Compress)) 'UI color smoke did not set second-group RGB #E85D75.'
Run-Case 'reopen-storage-smoke-ui-color' $uiColor 'writable'
$uiColorReopen = Get-Content -LiteralPath (Join-Path $uiColor 'config.json') -Raw | ConvertFrom-Json
Check ($uiColorReopen.groups.Count -eq 2 -and $uiColorReopen.next_group_id -ceq '8' -and $uiColorReopen.next_item_id -ceq '1') 'UI color smoke state or allocators did not survive restart.'
Check ($uiColorReopen.groups[0].id -ceq '1' -and $uiColorReopen.groups[0].title -ceq '待重命名的第一组' -and $uiColorReopen.groups[0].rect.x -eq 0 -and $uiColorReopen.groups[0].rect.y -eq 0 -and $uiColorReopen.groups[0].rect.width -eq 360 -and $uiColorReopen.groups[0].rect.height -eq 240 -and $uiColorReopen.groups[0].collapsed -eq $false -and $uiColorReopen.groups[0].items.Count -eq 0) 'First group did not survive UI-color restart unchanged.'
Check ($uiColorReopen.groups[1].id -ceq '7' -and $uiColorReopen.groups[1].title -ceq '保持不变的第二组' -and $uiColorReopen.groups[1].rect.x -eq 400 -and $uiColorReopen.groups[1].rect.y -eq 0 -and $uiColorReopen.groups[1].rect.width -eq 360 -and $uiColorReopen.groups[1].rect.height -eq 240 -and $uiColorReopen.groups[1].collapsed -eq $false -and $uiColorReopen.groups[1].items.Count -eq 0) 'Second-group fields did not survive UI-color restart.'
Check ((ConvertTo-Json -InputObject $uiColorReopen.groups[0].color -Compress) -ceq (ConvertTo-Json -InputObject $multiFixture.groups[0].color -Compress)) 'First-group color did not survive UI-color restart unchanged.'
Check ((ConvertTo-Json -InputObject $uiColorReopen.groups[1].color -Compress) -ceq (ConvertTo-Json -InputObject @(232,93,117) -Compress)) 'Second-group RGB did not survive UI-color restart.'
$fold = New-Case 'storage-smoke-fold'
Write-Utf8 (Join-Path $fold 'config.json') ($multiFixture | ConvertTo-Json -Depth 12 -Compress)
Run-Case 'storage-smoke-first-group-fold' $fold 'fold'
$foldAfter = Get-Content -LiteralPath (Join-Path $fold 'config.json') -Raw | ConvertFrom-Json
Check ($foldAfter.groups.Count -eq 2 -and $foldAfter.next_group_id -ceq '8' -and $foldAfter.next_item_id -ceq '1') 'Fold smoke changed group count or ID allocators.'
Check ($foldAfter.groups[0].id -ceq '1' -and $foldAfter.groups[0].title -ceq '待重命名的第一组' -and $foldAfter.groups[0].rect.x -eq 0 -and $foldAfter.groups[0].rect.y -eq 0 -and $foldAfter.groups[0].rect.width -eq 360 -and $foldAfter.groups[0].rect.height -eq 240 -and $foldAfter.groups[0].collapsed -eq $true -and $foldAfter.groups[0].items.Count -eq 0) 'Fold smoke did not collapse only the first group or changed its position/dimensions.'
Check ($foldAfter.groups[1].id -ceq '7' -and $foldAfter.groups[1].title -ceq '保持不变的第二组' -and $foldAfter.groups[1].rect.x -eq 400 -and $foldAfter.groups[1].rect.y -eq 0 -and $foldAfter.groups[1].rect.width -eq 360 -and $foldAfter.groups[1].rect.height -eq 240 -and $foldAfter.groups[1].collapsed -eq $false -and $foldAfter.groups[1].items.Count -eq 0) 'Fold smoke changed the non-target group.'
Check ((ConvertTo-Json -InputObject $foldAfter.groups[0].color -Compress) -ceq (ConvertTo-Json -InputObject $multiFixture.groups[0].color -Compress)) 'Fold smoke changed first-group color.'
Check ((ConvertTo-Json -InputObject $foldAfter.groups[1].color -Compress) -ceq (ConvertTo-Json -InputObject $multiFixture.groups[1].color -Compress)) 'Fold smoke changed second-group color.'
Run-Case 'reopen-storage-smoke-fold' $fold 'writable'
$foldReopen = Get-Content -LiteralPath (Join-Path $fold 'config.json') -Raw | ConvertFrom-Json
Check ($foldReopen.groups.Count -eq 2 -and $foldReopen.next_group_id -ceq '8' -and $foldReopen.next_item_id -ceq '1') 'Fold smoke state or allocators did not survive restart.'
Check ($foldReopen.groups[0].id -ceq '1' -and $foldReopen.groups[0].title -ceq '待重命名的第一组' -and $foldReopen.groups[0].rect.x -eq 0 -and $foldReopen.groups[0].rect.y -eq 0 -and $foldReopen.groups[0].rect.width -eq 360 -and $foldReopen.groups[0].rect.height -eq 240 -and $foldReopen.groups[0].collapsed -eq $true -and $foldReopen.groups[0].items.Count -eq 0) 'First-group fold state did not survive restart.'
Check ($foldReopen.groups[1].id -ceq '7' -and $foldReopen.groups[1].title -ceq '保持不变的第二组' -and $foldReopen.groups[1].rect.x -eq 400 -and $foldReopen.groups[1].rect.y -eq 0 -and $foldReopen.groups[1].rect.width -eq 360 -and $foldReopen.groups[1].rect.height -eq 240 -and $foldReopen.groups[1].collapsed -eq $false -and $foldReopen.groups[1].items.Count -eq 0) 'Second group did not survive fold-smoke restart unchanged.'
Check ((ConvertTo-Json -InputObject $foldReopen.groups[0].color -Compress) -ceq (ConvertTo-Json -InputObject $multiFixture.groups[0].color -Compress)) 'First-group color did not survive fold-smoke restart.'
Check ((ConvertTo-Json -InputObject $foldReopen.groups[1].color -Compress) -ceq (ConvertTo-Json -InputObject $multiFixture.groups[1].color -Compress)) 'Second-group color did not survive fold-smoke restart.'
$uiFold = New-Case 'storage-smoke-ui-fold'
Write-Utf8 (Join-Path $uiFold 'config.json') ($multiFixture | ConvertTo-Json -Depth 12 -Compress)
Run-Case 'storage-smoke-ui-second-group-fold' $uiFold 'ui-fold'
$uiFoldAfter = Get-Content -LiteralPath (Join-Path $uiFold 'config.json') -Raw | ConvertFrom-Json
Check ($uiFoldAfter.groups.Count -eq 2 -and $uiFoldAfter.next_group_id -ceq '8' -and $uiFoldAfter.next_item_id -ceq '1') 'UI fold smoke changed group count or ID allocators.'
Check ($uiFoldAfter.groups[0].id -ceq '1' -and $uiFoldAfter.groups[0].title -ceq '待重命名的第一组' -and $uiFoldAfter.groups[0].rect.x -eq 0 -and $uiFoldAfter.groups[0].rect.y -eq 0 -and $uiFoldAfter.groups[0].rect.width -eq 360 -and $uiFoldAfter.groups[0].rect.height -eq 240 -and $uiFoldAfter.groups[0].collapsed -eq $false -and $uiFoldAfter.groups[0].items.Count -eq 0) 'UI fold smoke changed the first group.'
Check ($uiFoldAfter.groups[1].id -ceq '7' -and $uiFoldAfter.groups[1].title -ceq '保持不变的第二组' -and $uiFoldAfter.groups[1].rect.x -eq 400 -and $uiFoldAfter.groups[1].rect.y -eq 0 -and $uiFoldAfter.groups[1].rect.width -eq 360 -and $uiFoldAfter.groups[1].rect.height -eq 240 -and $uiFoldAfter.groups[1].collapsed -eq $true -and $uiFoldAfter.groups[1].items.Count -eq 0) 'UI fold smoke did not collapse only the second group or changed its position/dimensions.'
Check ((ConvertTo-Json -InputObject $uiFoldAfter.groups[0].color -Compress) -ceq (ConvertTo-Json -InputObject $multiFixture.groups[0].color -Compress)) 'UI fold smoke changed first-group color.'
Check ((ConvertTo-Json -InputObject $uiFoldAfter.groups[1].color -Compress) -ceq (ConvertTo-Json -InputObject $multiFixture.groups[1].color -Compress)) 'UI fold smoke changed second-group color.'
Run-Case 'reopen-storage-smoke-ui-fold' $uiFold 'writable'
$uiFoldReopen = Get-Content -LiteralPath (Join-Path $uiFold 'config.json') -Raw | ConvertFrom-Json
Check ($uiFoldReopen.groups.Count -eq 2 -and $uiFoldReopen.next_group_id -ceq '8' -and $uiFoldReopen.next_item_id -ceq '1') 'UI-fold state or allocators did not survive restart.'
Check ($uiFoldReopen.groups[0].id -ceq '1' -and $uiFoldReopen.groups[0].title -ceq '待重命名的第一组' -and $uiFoldReopen.groups[0].rect.x -eq 0 -and $uiFoldReopen.groups[0].rect.y -eq 0 -and $uiFoldReopen.groups[0].rect.width -eq 360 -and $uiFoldReopen.groups[0].rect.height -eq 240 -and $uiFoldReopen.groups[0].collapsed -eq $false -and $uiFoldReopen.groups[0].items.Count -eq 0) 'First group did not survive UI-fold restart unchanged.'
Check ($uiFoldReopen.groups[1].id -ceq '7' -and $uiFoldReopen.groups[1].title -ceq '保持不变的第二组' -and $uiFoldReopen.groups[1].rect.x -eq 400 -and $uiFoldReopen.groups[1].rect.y -eq 0 -and $uiFoldReopen.groups[1].rect.width -eq 360 -and $uiFoldReopen.groups[1].rect.height -eq 240 -and $uiFoldReopen.groups[1].collapsed -eq $true -and $uiFoldReopen.groups[1].items.Count -eq 0) 'Second-group fold state did not survive UI-fold restart.'
Check ((ConvertTo-Json -InputObject $uiFoldReopen.groups[0].color -Compress) -ceq (ConvertTo-Json -InputObject $multiFixture.groups[0].color -Compress)) 'First-group color did not survive UI-fold restart.'
Check ((ConvertTo-Json -InputObject $uiFoldReopen.groups[1].color -Compress) -ceq (ConvertTo-Json -InputObject $multiFixture.groups[1].color -Compress)) 'Second-group color did not survive UI-fold restart.'
$move = New-Case 'move-existing-group'
Write-Utf8 (Join-Path $move 'config.json') ($multiFixture | ConvertTo-Json -Depth 12 -Compress)
Run-Case 'move-first-group' $move 'move'
$moved = Get-Content -LiteralPath (Join-Path $move 'config.json') -Raw | ConvertFrom-Json
Check ($moved.groups.Count -eq 2 -and $moved.groups[0].id -ceq '1' -and $moved.groups[0].rect.x -eq -120 -and $moved.groups[0].rect.y -eq 72 -and $moved.groups[0].rect.width -eq 360 -and $moved.groups[0].rect.height -eq 240 -and $moved.groups[1].id -ceq '7' -and $moved.groups[1].rect.x -eq 400 -and $moved.groups[1].rect.y -eq 0 -and $moved.next_group_id -ceq '8') 'Move changed unexpected group fields.'
Run-Case 'reopen-moved-group' $move 'writable'
$movedReopen = Get-Content -LiteralPath (Join-Path $move 'config.json') -Raw | ConvertFrom-Json
Check ($movedReopen.groups.Count -eq 2 -and $movedReopen.groups[0].id -ceq '1' -and $movedReopen.groups[0].rect.x -eq -120 -and $movedReopen.groups[0].rect.y -eq 72 -and $movedReopen.groups[1].id -ceq '7' -and $movedReopen.groups[1].rect.x -eq 400) 'Moved group was not restored.'
$uiMove = New-Case 'ui-move-second-group'
Write-Utf8 (Join-Path $uiMove 'config.json') ($multiFixture | ConvertTo-Json -Depth 12 -Compress)
Run-Case 'ui-controls-move-second-group' $uiMove 'ui-move'
$uiMoved = Get-Content -LiteralPath (Join-Path $uiMove 'config.json') -Raw | ConvertFrom-Json
Check ($uiMoved.groups.Count -eq 2 -and $uiMoved.next_group_id -ceq '8' -and $uiMoved.next_item_id -ceq '1') 'UI move changed group count or ID allocators.'
Check ($uiMoved.groups[0].id -ceq '1' -and $uiMoved.groups[0].title -ceq '待重命名的第一组' -and $uiMoved.groups[0].rect.x -eq 0 -and $uiMoved.groups[0].rect.y -eq 0 -and $uiMoved.groups[0].rect.width -eq 360 -and $uiMoved.groups[0].rect.height -eq 240 -and $uiMoved.groups[0].collapsed -eq $false -and $uiMoved.groups[0].items.Count -eq 0) 'UI move changed the first group.'
Check ($uiMoved.groups[1].id -ceq '7' -and $uiMoved.groups[1].title -ceq '保持不变的第二组' -and $uiMoved.groups[1].rect.x -eq -240 -and $uiMoved.groups[1].rect.y -eq 96 -and $uiMoved.groups[1].rect.width -eq 360 -and $uiMoved.groups[1].rect.height -eq 240 -and $uiMoved.groups[1].collapsed -eq $false -and $uiMoved.groups[1].items.Count -eq 0) 'UI move changed the wrong group or dimensions.'
Check ((ConvertTo-Json -InputObject $uiMoved.groups[0].color -Compress) -ceq (ConvertTo-Json -InputObject $multiFixture.groups[0].color -Compress)) 'UI move changed the first group color.'
Check ((ConvertTo-Json -InputObject $uiMoved.groups[1].color -Compress) -ceq (ConvertTo-Json -InputObject $multiFixture.groups[1].color -Compress)) 'UI move changed the second group color.'
Run-Case 'reopen-ui-moved-group' $uiMove 'writable'
$uiMoveReopen = Get-Content -LiteralPath (Join-Path $uiMove 'config.json') -Raw | ConvertFrom-Json
Check ($uiMoveReopen.groups.Count -eq 2 -and $uiMoveReopen.groups[0].id -ceq '1' -and $uiMoveReopen.groups[0].title -ceq '待重命名的第一组' -and $uiMoveReopen.groups[0].rect.x -eq 0 -and $uiMoveReopen.groups[0].rect.y -eq 0 -and $uiMoveReopen.groups[0].rect.width -eq 360 -and $uiMoveReopen.groups[0].rect.height -eq 240 -and $uiMoveReopen.groups[1].id -ceq '7' -and $uiMoveReopen.groups[1].title -ceq '保持不变的第二组' -and $uiMoveReopen.groups[1].rect.x -eq -240 -and $uiMoveReopen.groups[1].rect.y -eq 96 -and $uiMoveReopen.groups[1].rect.width -eq 360 -and $uiMoveReopen.groups[1].rect.height -eq 240 -and $uiMoveReopen.next_group_id -ceq '8') 'Native-control move state did not survive restart.'
$uiDrag = New-Case 'ui-drag-second-group'
Write-Utf8 (Join-Path $uiDrag 'config.json') ($multiFixture | ConvertTo-Json -Depth 12 -Compress)
Run-Case 'ui-drag-second-group' $uiDrag 'ui-drag'
$uiDragged = Get-Content -LiteralPath (Join-Path $uiDrag 'config.json') -Raw | ConvertFrom-Json
Check ($uiDragged.groups.Count -eq 2 -and $uiDragged.next_group_id -ceq '8' -and $uiDragged.next_item_id -ceq '1') 'UI drag changed group count or ID allocators.'
Check ($uiDragged.groups[0].id -ceq '1' -and $uiDragged.groups[0].title -ceq '待重命名的第一组' -and $uiDragged.groups[0].rect.x -eq 0 -and $uiDragged.groups[0].rect.y -eq 0 -and $uiDragged.groups[0].rect.width -eq 360 -and $uiDragged.groups[0].rect.height -eq 240 -and $uiDragged.groups[0].collapsed -eq $false -and $uiDragged.groups[0].items.Count -eq 0) 'UI drag changed the first group.'
Check ($uiDragged.groups[1].id -ceq '7' -and $uiDragged.groups[1].title -ceq '保持不变的第二组' -and $uiDragged.groups[1].rect.x -eq 464 -and $uiDragged.groups[1].rect.y -eq 32 -and $uiDragged.groups[1].rect.width -eq 360 -and $uiDragged.groups[1].rect.height -eq 240 -and $uiDragged.groups[1].collapsed -eq $false -and $uiDragged.groups[1].items.Count -eq 0) 'UI drag changed the wrong group, dimensions, or destination.'
Check ((ConvertTo-Json -InputObject $uiDragged.groups[0].color -Compress) -ceq (ConvertTo-Json -InputObject $multiFixture.groups[0].color -Compress)) 'UI drag changed the first group color.'
Check ((ConvertTo-Json -InputObject $uiDragged.groups[1].color -Compress) -ceq (ConvertTo-Json -InputObject $multiFixture.groups[1].color -Compress)) 'UI drag changed the second group color.'
Run-Case 'reopen-ui-dragged-group' $uiDrag 'writable'
$uiDragReopen = Get-Content -LiteralPath (Join-Path $uiDrag 'config.json') -Raw | ConvertFrom-Json
Check ($uiDragReopen.groups.Count -eq 2 -and $uiDragReopen.next_group_id -ceq '8' -and $uiDragReopen.next_item_id -ceq '1') 'UI-drag state or allocators did not survive restart.'
Check ($uiDragReopen.groups[0].id -ceq '1' -and $uiDragReopen.groups[0].title -ceq '待重命名的第一组' -and $uiDragReopen.groups[0].rect.x -eq 0 -and $uiDragReopen.groups[0].rect.y -eq 0 -and $uiDragReopen.groups[0].rect.width -eq 360 -and $uiDragReopen.groups[0].rect.height -eq 240) 'First group did not survive UI-drag restart unchanged.'
Check ($uiDragReopen.groups[1].id -ceq '7' -and $uiDragReopen.groups[1].title -ceq '保持不变的第二组' -and $uiDragReopen.groups[1].rect.x -eq 464 -and $uiDragReopen.groups[1].rect.y -eq 32 -and $uiDragReopen.groups[1].rect.width -eq 360 -and $uiDragReopen.groups[1].rect.height -eq 240) 'Dragged group state did not survive restart.'
$resize = New-Case 'resize-first-group'
Write-Utf8 (Join-Path $resize 'config.json') ($multiFixture | ConvertTo-Json -Depth 12 -Compress)
Run-Case 'resize-first-group' $resize 'resize'
$resized = Get-Content -LiteralPath (Join-Path $resize 'config.json') -Raw | ConvertFrom-Json
Check ($resized.groups.Count -eq 2 -and $resized.groups[0].id -ceq '1' -and $resized.groups[0].rect.x -eq 0 -and $resized.groups[0].rect.y -eq 0 -and $resized.groups[0].rect.width -eq 480 -and $resized.groups[0].rect.height -eq 280 -and $resized.groups[1].id -ceq '7' -and $resized.groups[1].rect.x -eq 400 -and $resized.groups[1].rect.y -eq 0 -and $resized.groups[1].rect.width -eq 360 -and $resized.groups[1].rect.height -eq 240 -and $resized.next_group_id -ceq '8') 'Resize changed unexpected group fields.'
Run-Case 'reopen-resized-group' $resize 'writable'
$resizedReopen = Get-Content -LiteralPath (Join-Path $resize 'config.json') -Raw | ConvertFrom-Json
Check ($resizedReopen.groups.Count -eq 2 -and $resizedReopen.groups[0].rect.width -eq 480 -and $resizedReopen.groups[0].rect.height -eq 280 -and $resizedReopen.groups[1].rect.width -eq 360 -and $resizedReopen.groups[1].rect.height -eq 240) 'Resized group was not restored.'
$uiResize = New-Case 'ui-resize-second-group'
Write-Utf8 (Join-Path $uiResize 'config.json') ($multiFixture | ConvertTo-Json -Depth 12 -Compress)
Run-Case 'ui-controls-resize-second-group' $uiResize 'ui-resize'
$uiResized = Get-Content -LiteralPath (Join-Path $uiResize 'config.json') -Raw | ConvertFrom-Json
Check ($uiResized.groups.Count -eq 2 -and $uiResized.next_group_id -ceq '8' -and $uiResized.next_item_id -ceq '1') 'UI resize changed group count or ID allocators.'
Check ($uiResized.groups[0].id -ceq '1' -and $uiResized.groups[0].title -ceq '待重命名的第一组' -and $uiResized.groups[0].rect.x -eq 0 -and $uiResized.groups[0].rect.y -eq 0 -and $uiResized.groups[0].rect.width -eq 360 -and $uiResized.groups[0].rect.height -eq 240 -and $uiResized.groups[0].collapsed -eq $false -and $uiResized.groups[0].items.Count -eq 0) 'UI resize changed the first group.'
Check ($uiResized.groups[1].id -ceq '7' -and $uiResized.groups[1].title -ceq '保持不变的第二组' -and $uiResized.groups[1].rect.x -eq 400 -and $uiResized.groups[1].rect.y -eq 0 -and $uiResized.groups[1].rect.width -eq 500 -and $uiResized.groups[1].rect.height -eq 300 -and $uiResized.groups[1].collapsed -eq $false -and $uiResized.groups[1].items.Count -eq 0) 'UI resize changed unexpected second-group fields.'
Check ((ConvertTo-Json -InputObject $uiResized.groups[0].color -Compress) -ceq (ConvertTo-Json -InputObject $multiFixture.groups[0].color -Compress)) 'UI resize changed the first group color.'
Check ((ConvertTo-Json -InputObject $uiResized.groups[1].color -Compress) -ceq (ConvertTo-Json -InputObject $multiFixture.groups[1].color -Compress)) 'UI resize changed the second group color.'
Run-Case 'reopen-ui-resized-second-group' $uiResize 'writable'
$uiResizeReopen = Get-Content -LiteralPath (Join-Path $uiResize 'config.json') -Raw | ConvertFrom-Json
Check ($uiResizeReopen.groups.Count -eq 2 -and $uiResizeReopen.next_group_id -ceq '8' -and $uiResizeReopen.next_item_id -ceq '1') 'UI-resize state or allocators did not survive restart.'
Check ($uiResizeReopen.groups[0].id -ceq '1' -and $uiResizeReopen.groups[0].title -ceq '待重命名的第一组' -and $uiResizeReopen.groups[0].rect.x -eq 0 -and $uiResizeReopen.groups[0].rect.y -eq 0 -and $uiResizeReopen.groups[0].rect.width -eq 360 -and $uiResizeReopen.groups[0].rect.height -eq 240) 'First group did not survive UI-resize restart unchanged.'
Check ($uiResizeReopen.groups[1].id -ceq '7' -and $uiResizeReopen.groups[1].title -ceq '保持不变的第二组' -and $uiResizeReopen.groups[1].rect.x -eq 400 -and $uiResizeReopen.groups[1].rect.y -eq 0 -and $uiResizeReopen.groups[1].rect.width -eq 500 -and $uiResizeReopen.groups[1].rect.height -eq 300) 'Resized second-group state did not survive restart.'
Check ((ConvertTo-Json -InputObject $uiResizeReopen.groups[0].color -Compress) -ceq (ConvertTo-Json -InputObject $multiFixture.groups[0].color -Compress)) 'First group color did not survive UI-resize restart.'
Check ((ConvertTo-Json -InputObject $uiResizeReopen.groups[1].color -Compress) -ceq (ConvertTo-Json -InputObject $multiFixture.groups[1].color -Compress)) 'Second group color did not survive UI-resize restart.'
$uiResizeDrag = New-Case 'ui-resize-drag-second-group'
Write-Utf8 (Join-Path $uiResizeDrag 'config.json') ($multiFixture | ConvertTo-Json -Depth 12 -Compress)
Run-Case 'ui-resize-drag-second-group' $uiResizeDrag 'ui-resize-drag'
$uiResizedByDrag = Get-Content -LiteralPath (Join-Path $uiResizeDrag 'config.json') -Raw | ConvertFrom-Json
Check ($uiResizedByDrag.groups.Count -eq 2 -and $uiResizedByDrag.next_group_id -ceq '8' -and $uiResizedByDrag.next_item_id -ceq '1') 'UI resize drag changed group count or ID allocators.'
Check ($uiResizedByDrag.groups[0].id -ceq '1' -and $uiResizedByDrag.groups[0].title -ceq '待重命名的第一组' -and $uiResizedByDrag.groups[0].rect.x -eq 0 -and $uiResizedByDrag.groups[0].rect.y -eq 0 -and $uiResizedByDrag.groups[0].rect.width -eq 360 -and $uiResizedByDrag.groups[0].rect.height -eq 240 -and $uiResizedByDrag.groups[0].collapsed -eq $false -and $uiResizedByDrag.groups[0].items.Count -eq 0) 'UI resize drag changed the first group.'
Check ($uiResizedByDrag.groups[1].id -ceq '7' -and $uiResizedByDrag.groups[1].title -ceq '保持不变的第二组' -and $uiResizedByDrag.groups[1].rect.x -eq 400 -and $uiResizedByDrag.groups[1].rect.y -eq 0 -and $uiResizedByDrag.groups[1].rect.width -eq 424 -and $uiResizedByDrag.groups[1].rect.height -eq 272 -and $uiResizedByDrag.groups[1].collapsed -eq $false -and $uiResizedByDrag.groups[1].items.Count -eq 0) 'UI resize drag changed an unexpected second-group field or size.'
Check ((ConvertTo-Json -InputObject $uiResizedByDrag.groups[0].color -Compress) -ceq (ConvertTo-Json -InputObject $multiFixture.groups[0].color -Compress)) 'UI resize drag changed the first group color.'
Check ((ConvertTo-Json -InputObject $uiResizedByDrag.groups[1].color -Compress) -ceq (ConvertTo-Json -InputObject $multiFixture.groups[1].color -Compress)) 'UI resize drag changed the second group color.'
Run-Case 'reopen-ui-resize-drag-second-group' $uiResizeDrag 'writable'
$uiResizeDragReopen = Get-Content -LiteralPath (Join-Path $uiResizeDrag 'config.json') -Raw | ConvertFrom-Json
Check ($uiResizeDragReopen.groups.Count -eq 2 -and $uiResizeDragReopen.next_group_id -ceq '8' -and $uiResizeDragReopen.next_item_id -ceq '1') 'UI resize-drag state or allocators did not survive restart.'
Check ($uiResizeDragReopen.groups[0].id -ceq '1' -and $uiResizeDragReopen.groups[0].title -ceq '待重命名的第一组' -and $uiResizeDragReopen.groups[0].rect.x -eq 0 -and $uiResizeDragReopen.groups[0].rect.y -eq 0 -and $uiResizeDragReopen.groups[0].rect.width -eq 360 -and $uiResizeDragReopen.groups[0].rect.height -eq 240) 'First group did not survive UI resize-drag restart unchanged.'
Check ($uiResizeDragReopen.groups[1].id -ceq '7' -and $uiResizeDragReopen.groups[1].title -ceq '保持不变的第二组' -and $uiResizeDragReopen.groups[1].rect.x -eq 400 -and $uiResizeDragReopen.groups[1].rect.y -eq 0 -and $uiResizeDragReopen.groups[1].rect.width -eq 424 -and $uiResizeDragReopen.groups[1].rect.height -eq 272) 'Resize-dragged second-group state did not survive restart.'
Check ((ConvertTo-Json -InputObject $uiResizeDragReopen.groups[0].color -Compress) -ceq (ConvertTo-Json -InputObject $multiFixture.groups[0].color -Compress)) 'First group color did not survive UI resize-drag restart.'
Check ((ConvertTo-Json -InputObject $uiResizeDragReopen.groups[1].color -Compress) -ceq (ConvertTo-Json -InputObject $multiFixture.groups[1].color -Compress)) 'Second group color did not survive UI resize-drag restart.'
$delete = New-Case 'delete-first-group'
Write-Utf8 (Join-Path $delete 'config.json') ($multiFixture | ConvertTo-Json -Depth 12 -Compress)
Run-Case 'delete-first-group' $delete 'delete'
$deleted = Get-Content -LiteralPath (Join-Path $delete 'config.json') -Raw | ConvertFrom-Json
Check ($deleted.groups.Count -eq 1 -and $deleted.groups[0].id -ceq '7' -and $deleted.groups[0].title -ceq '保持不变的第二组' -and $deleted.groups[0].rect.x -eq 400 -and $deleted.groups[0].rect.y -eq 0 -and $deleted.groups[0].rect.width -eq 360 -and $deleted.groups[0].rect.height -eq 240 -and $deleted.next_group_id -ceq '8' -and $deleted.next_item_id -ceq '1') 'Delete changed the wrong group or allocators.'
Run-Case 'reopen-after-deleting-group' $delete 'writable'
$deleteReopen = Get-Content -LiteralPath (Join-Path $delete 'config.json') -Raw | ConvertFrom-Json
Check ($deleteReopen.groups.Count -eq 1 -and $deleteReopen.groups[0].id -ceq '7' -and $deleteReopen.next_group_id -ceq '8') 'Deleted group reappeared after restart.'
$uiDelete = New-Case 'ui-delete-second-group'
Write-Utf8 (Join-Path $uiDelete 'config.json') ($multiFixture | ConvertTo-Json -Depth 12 -Compress)
Run-Case 'ui-controls-delete-second-group' $uiDelete 'ui-delete'
$uiDeleted = Get-Content -LiteralPath (Join-Path $uiDelete 'config.json') -Raw | ConvertFrom-Json
Check ($uiDeleted.groups.Count -eq 1 -and $uiDeleted.next_group_id -ceq '8' -and $uiDeleted.next_item_id -ceq '1') 'UI delete changed group count or ID allocators.'
Check ($uiDeleted.groups[0].id -ceq '1' -and $uiDeleted.groups[0].title -ceq '待重命名的第一组' -and $uiDeleted.groups[0].rect.x -eq 0 -and $uiDeleted.groups[0].rect.y -eq 0 -and $uiDeleted.groups[0].rect.width -eq 360 -and $uiDeleted.groups[0].rect.height -eq 240 -and $uiDeleted.groups[0].collapsed -eq $false -and $uiDeleted.groups[0].items.Count -eq 0) 'UI delete changed the first group fields.'
Check ((ConvertTo-Json -InputObject $uiDeleted.groups[0].color -Compress) -ceq (ConvertTo-Json -InputObject $multiFixture.groups[0].color -Compress)) 'UI delete changed the first group color.'
Check (@($uiDeleted.groups | Where-Object { $_.id -ceq '7' }).Count -eq 0) 'UI delete did not remove group ID 7.'
Run-Case 'reopen-after-ui-deleting-second-group' $uiDelete 'writable'
$uiDeleteReopen = Get-Content -LiteralPath (Join-Path $uiDelete 'config.json') -Raw | ConvertFrom-Json
Check ($uiDeleteReopen.groups.Count -eq 1 -and $uiDeleteReopen.next_group_id -ceq '8' -and $uiDeleteReopen.next_item_id -ceq '1') 'UI-delete state or allocators did not survive restart.'
Check ($uiDeleteReopen.groups[0].id -ceq '1' -and $uiDeleteReopen.groups[0].title -ceq '待重命名的第一组' -and $uiDeleteReopen.groups[0].rect.x -eq 0 -and $uiDeleteReopen.groups[0].rect.y -eq 0 -and $uiDeleteReopen.groups[0].rect.width -eq 360 -and $uiDeleteReopen.groups[0].rect.height -eq 240 -and $uiDeleteReopen.groups[0].collapsed -eq $false -and $uiDeleteReopen.groups[0].items.Count -eq 0) 'First group did not survive UI-delete restart unchanged.'
Check ((ConvertTo-Json -InputObject $uiDeleteReopen.groups[0].color -Compress) -ceq (ConvertTo-Json -InputObject $multiFixture.groups[0].color -Compress)) 'First group color did not survive UI-delete restart.'
Check (@($uiDeleteReopen.groups | Where-Object { $_.id -ceq '7' }).Count -eq 0) 'Deleted group ID 7 reappeared after restart.'
$add = New-Case 'add-reference'
Write-Utf8 (Join-Path $add 'config.json') ($multiFixture | ConvertTo-Json -Depth 12 -Compress)
$addSource = Join-Path $add 'source.txt'
Write-Utf8 $addSource 'A source file must remain unchanged after adding its reference.'
$addSourceHash = (Get-FileHash -LiteralPath $addSource).Hash
Run-Case 'add-first-group-reference' $add 'add'
$added = Get-Content -LiteralPath (Join-Path $add 'config.json') -Raw | ConvertFrom-Json
Check ($added.groups.Count -eq 2 -and $added.groups[0].id -ceq '1' -and $added.groups[0].items.Count -eq 1 -and $added.groups[0].items[0].id -ceq '1' -and $added.groups[0].items[0].target.kind -ceq 'file' -and $added.groups[1].id -ceq '7' -and $added.groups[1].items.Count -eq 0 -and $added.next_item_id -ceq '2') 'Adding a reference changed group identity, placement or item allocator.'
$storedSource = -join @($added.groups[0].items[0].target.path_utf16 | ForEach-Object { [char][int]$_ })
Check ($storedSource -ceq $addSource) 'Added reference path differs from fixture source path.'
Check ((Get-FileHash -LiteralPath $addSource).Hash -ceq $addSourceHash) 'Adding a reference changed source bytes.'
Run-Case 'reopen-added-reference' $add 'writable'
$addReopen = Get-Content -LiteralPath (Join-Path $add 'config.json') -Raw | ConvertFrom-Json
Check ($addReopen.groups[0].items.Count -eq 1 -and $addReopen.groups[0].items[0].id -ceq '1' -and $addReopen.next_item_id -ceq '2') 'Added reference did not survive restart.'
Check ((Get-FileHash -LiteralPath $addSource).Hash -ceq $addSourceHash) 'Reopening changed source bytes.'
$uiAdd = New-Case 'ui-add-second-group'
Write-Utf8 (Join-Path $uiAdd 'config.json') ($multiFixture | ConvertTo-Json -Depth 12 -Compress)
$uiAddSource = Join-Path $uiAdd 'source.txt'
Write-Utf8 $uiAddSource 'UI add must preserve source bytes.'
$uiAddHash = (Get-FileHash -LiteralPath $uiAddSource).Hash
Run-Case 'ui-button-add-second-group-reference' $uiAdd 'ui-add'
$uiAdded = Get-Content -LiteralPath (Join-Path $uiAdd 'config.json') -Raw | ConvertFrom-Json
Check ($uiAdded.groups.Count -eq 2 -and $uiAdded.groups[0].id -ceq '1' -and $uiAdded.groups[0].items.Count -eq 0 -and $uiAdded.groups[1].id -ceq '7' -and $uiAdded.groups[1].items.Count -eq 1 -and $uiAdded.groups[1].items[0].id -ceq '1' -and $uiAdded.groups[1].items[0].target.kind -ceq 'file' -and $uiAdded.next_item_id -ceq '2') 'UI add changed the wrong group or allocator.'
$uiStoredSource = -join @($uiAdded.groups[1].items[0].target.path_utf16 | ForEach-Object { [char][int]$_ })
Check ($uiStoredSource -ceq $uiAddSource) 'UI add stored the wrong source path.'
Check ((Get-FileHash -LiteralPath $uiAddSource).Hash -ceq $uiAddHash) 'UI add changed source bytes.'
Run-Case 'reopen-ui-added-reference' $uiAdd 'writable'
$uiAddReopen = Get-Content -LiteralPath (Join-Path $uiAdd 'config.json') -Raw | ConvertFrom-Json
Check ($uiAddReopen.groups[0].items.Count -eq 0 -and $uiAddReopen.groups[1].items.Count -eq 1 -and $uiAddReopen.groups[1].items[0].id -ceq '1' -and $uiAddReopen.next_item_id -ceq '2') 'UI-added reference did not survive restart.'
Check ((Get-FileHash -LiteralPath $uiAddSource).Hash -ceq $uiAddHash) 'UI add restart changed source bytes.'
$uiShortcut = New-Case 'ui-add-shortcut-second-group'
Write-Utf8 (Join-Path $uiShortcut 'config.json') ($multiFixture | ConvertTo-Json -Depth 12 -Compress)
$shortcutTarget = Join-Path $uiShortcut 'source.txt'
Write-Utf8 $shortcutTarget 'Shortcut target must remain unchanged.'
$shortcutPath = Join-Path $uiShortcut 'source-link.lnk'
$shell = New-Object -ComObject WScript.Shell
$shortcut = $null
try {
    $shortcut = $shell.CreateShortcut($shortcutPath)
    $shortcut.TargetPath = $shortcutTarget
    $shortcut.Save()
} finally {
    if ($shortcut) { [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($shortcut) }
    [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($shell)
}
Check ((Test-Path -LiteralPath $shortcutPath -PathType Leaf) -and (Test-Path -LiteralPath $shortcutTarget -PathType Leaf)) 'Shortcut fixture or its target was not created.'
$shortcutHash = (Get-FileHash -LiteralPath $shortcutPath).Hash
$shortcutTargetHash = (Get-FileHash -LiteralPath $shortcutTarget).Hash
Run-Case 'ui-button-add-second-group-shortcut' $uiShortcut 'ui-shortcut'
$shortcutAdded = Get-Content -LiteralPath (Join-Path $uiShortcut 'config.json') -Raw | ConvertFrom-Json
Check ($shortcutAdded.groups.Count -eq 2 -and $shortcutAdded.groups[0].id -ceq '1' -and $shortcutAdded.groups[0].items.Count -eq 0 -and $shortcutAdded.groups[1].id -ceq '7' -and $shortcutAdded.groups[1].items.Count -eq 1 -and $shortcutAdded.groups[1].items[0].id -ceq '1' -and $shortcutAdded.groups[1].items[0].target.kind -ceq 'shortcut' -and $shortcutAdded.next_item_id -ceq '2') 'UI shortcut add changed the wrong group, kind, or allocator.'
$storedShortcut = -join @($shortcutAdded.groups[1].items[0].target.path_utf16 | ForEach-Object { [char][int]$_ })
Check ($storedShortcut -ceq $shortcutPath -and $storedShortcut -cne $shortcutTarget) 'UI shortcut add did not store the .lnk path itself.'
Check ((Get-FileHash -LiteralPath $shortcutPath).Hash -ceq $shortcutHash -and (Get-FileHash -LiteralPath $shortcutTarget).Hash -ceq $shortcutTargetHash) 'UI shortcut add changed the shortcut or target bytes.'
Run-Case 'reopen-ui-added-shortcut' $uiShortcut 'writable'
$shortcutReopen = Get-Content -LiteralPath (Join-Path $uiShortcut 'config.json') -Raw | ConvertFrom-Json
Check ($shortcutReopen.groups.Count -eq 2 -and $shortcutReopen.groups[0].items.Count -eq 0 -and $shortcutReopen.groups[1].id -ceq '7' -and $shortcutReopen.groups[1].items.Count -eq 1 -and $shortcutReopen.groups[1].items[0].id -ceq '1' -and $shortcutReopen.groups[1].items[0].target.kind -ceq 'shortcut' -and $shortcutReopen.next_item_id -ceq '2') 'UI-added shortcut did not survive restart.'
$reopenedShortcutPath = -join @($shortcutReopen.groups[1].items[0].target.path_utf16 | ForEach-Object { [char][int]$_ })
Check ($reopenedShortcutPath -ceq $shortcutPath -and $reopenedShortcutPath -cne $shortcutTarget) 'Shortcut path did not survive restart as a .lnk reference.'
Check ((Get-FileHash -LiteralPath $shortcutPath).Hash -ceq $shortcutHash -and (Get-FileHash -LiteralPath $shortcutTarget).Hash -ceq $shortcutTargetHash) 'Shortcut restart changed the shortcut or target bytes.'
$uiFolder = New-Case 'ui-add-folder-second-group'
Write-Utf8 (Join-Path $uiFolder 'config.json') ($multiFixture | ConvertTo-Json -Depth 12 -Compress)
$sourceFolder = Join-Path $uiFolder 'source-folder'
New-Item -ItemType Directory -Path $sourceFolder | Out-Null
$folderFile = Join-Path $sourceFolder 'keep.txt'
Write-Utf8 $folderFile 'Folder source contents must remain unchanged.'
$folderHash = (Get-FileHash -LiteralPath $folderFile).Hash
Run-Case 'ui-button-add-second-group-folder' $uiFolder 'ui-folder'
$folderAdded = Get-Content -LiteralPath (Join-Path $uiFolder 'config.json') -Raw | ConvertFrom-Json
Check ($folderAdded.groups.Count -eq 2 -and $folderAdded.groups[0].id -ceq '1' -and $folderAdded.groups[0].items.Count -eq 0 -and $folderAdded.groups[1].id -ceq '7' -and $folderAdded.groups[1].items.Count -eq 1 -and $folderAdded.groups[1].items[0].id -ceq '1' -and $folderAdded.groups[1].items[0].target.kind -ceq 'directory' -and $folderAdded.next_item_id -ceq '2') 'UI folder add changed the wrong group or allocator.'
$storedFolder = -join @($folderAdded.groups[1].items[0].target.path_utf16 | ForEach-Object { [char][int]$_ })
Check ($storedFolder -ceq $sourceFolder) 'UI folder add stored the wrong folder path.'
Check ((Get-FileHash -LiteralPath $folderFile).Hash -ceq $folderHash) 'UI folder add changed source file bytes.'
Check (@(Get-ChildItem -LiteralPath $sourceFolder -Force).Count -eq 1) 'UI folder add changed source folder entries.'
Run-Case 'reopen-ui-added-folder' $uiFolder 'writable'
$folderReopen = Get-Content -LiteralPath (Join-Path $uiFolder 'config.json') -Raw | ConvertFrom-Json
Check ($folderReopen.groups[0].items.Count -eq 0 -and $folderReopen.groups[1].items.Count -eq 1 -and $folderReopen.groups[1].items[0].target.kind -ceq 'directory' -and $folderReopen.next_item_id -ceq '2') 'UI-added folder did not survive restart.'
Check ((Get-FileHash -LiteralPath $folderFile).Hash -ceq $folderHash) 'UI folder restart changed source file bytes.'
$drop = New-Case 'ui-drop-link-second-group'
Write-Utf8 (Join-Path $drop 'config.json') ($multiFixture | ConvertTo-Json -Depth 12 -Compress)
$dropSource = Join-Path $drop 'source.txt'
Write-Utf8 $dropSource 'Synthetic Shell CF_HDROP must never move this file.'
$dropHash = (Get-FileHash -LiteralPath $dropSource).Hash
Run-Case 'ui-drop-reject-move-then-link-second-group' $drop 'ui-drop'
$dropped = Get-Content -LiteralPath (Join-Path $drop 'config.json') -Raw | ConvertFrom-Json
Check ($dropped.groups.Count -eq 2 -and $dropped.groups[0].id -ceq '1' -and $dropped.groups[0].items.Count -eq 0 -and $dropped.groups[1].id -ceq '7' -and $dropped.groups[1].items.Count -eq 1 -and $dropped.groups[1].items[0].id -ceq '1' -and $dropped.groups[1].items[0].target.kind -ceq 'file' -and $dropped.next_item_id -ceq '2') 'Synthetic Shell drop changed the wrong group or allocator.'
$dropStoredSource = -join @($dropped.groups[1].items[0].target.path_utf16 | ForEach-Object { [char][int]$_ })
Check ($dropStoredSource -ceq $dropSource) 'Synthetic Shell drop stored the wrong source path.'
Check ((Get-FileHash -LiteralPath $dropSource).Hash -ceq $dropHash) 'Synthetic Shell drop changed source bytes.'
Run-Case 'reopen-ui-dropped-reference' $drop 'writable'
$dropReopen = Get-Content -LiteralPath (Join-Path $drop 'config.json') -Raw | ConvertFrom-Json
Check ($dropReopen.groups[0].items.Count -eq 0 -and $dropReopen.groups[1].items.Count -eq 1 -and $dropReopen.groups[1].items[0].target.kind -ceq 'file' -and $dropReopen.next_item_id -ceq '2') 'Synthetic Shell drop did not survive restart.'
Check ((Get-FileHash -LiteralPath $dropSource).Hash -ceq $dropHash) 'Synthetic Shell drop restart changed source bytes.'
$dropFolder = New-Case 'ui-drop-folder-second-group'
Write-Utf8 (Join-Path $dropFolder 'config.json') ($multiFixture | ConvertTo-Json -Depth 12 -Compress)
$dropSourceFolder = Join-Path $dropFolder 'source-folder'
New-Item -ItemType Directory -Path $dropSourceFolder | Out-Null
$dropFolderFile = Join-Path $dropSourceFolder 'keep.txt'
Write-Utf8 $dropFolderFile 'Synthetic Shell folder drop must preserve this file.'
$dropFolderHash = (Get-FileHash -LiteralPath $dropFolderFile).Hash
Run-Case 'ui-drop-folder-reject-move-then-link' $dropFolder 'ui-drop-folder'
$dropFolderConfig = Get-Content -LiteralPath (Join-Path $dropFolder 'config.json') -Raw | ConvertFrom-Json
Check ($dropFolderConfig.groups.Count -eq 2 -and $dropFolderConfig.groups[0].items.Count -eq 0 -and $dropFolderConfig.groups[1].id -ceq '7' -and $dropFolderConfig.groups[1].items.Count -eq 1 -and $dropFolderConfig.groups[1].items[0].id -ceq '1' -and $dropFolderConfig.groups[1].items[0].target.kind -ceq 'directory' -and $dropFolderConfig.next_item_id -ceq '2') 'Synthetic Shell folder drop changed the wrong group, kind, or allocator.'
$storedDropFolder = -join @($dropFolderConfig.groups[1].items[0].target.path_utf16 | ForEach-Object { [char][int]$_ })
Check ($storedDropFolder -ceq $dropSourceFolder) 'Synthetic Shell folder drop stored the wrong path.'
Check ((Get-FileHash -LiteralPath $dropFolderFile).Hash -ceq $dropFolderHash) 'Synthetic Shell folder drop changed source bytes.'
Check (@(Get-ChildItem -LiteralPath $dropSourceFolder -Force).Count -eq 1) 'Synthetic Shell folder drop changed source entries.'
Run-Case 'reopen-ui-dropped-folder' $dropFolder 'writable'
$dropFolderReopen = Get-Content -LiteralPath (Join-Path $dropFolder 'config.json') -Raw | ConvertFrom-Json
Check ($dropFolderReopen.groups[0].items.Count -eq 0 -and $dropFolderReopen.groups[1].items.Count -eq 1 -and $dropFolderReopen.groups[1].items[0].target.kind -ceq 'directory' -and $dropFolderReopen.next_item_id -ceq '2') 'Synthetic Shell folder drop did not survive restart.'
Check ((Get-FileHash -LiteralPath $dropFolderFile).Hash -ceq $dropFolderHash) 'Synthetic Shell folder drop restart changed source bytes.'
$dropShortcut = New-Case 'ui-drop-shortcut-second-group'
Write-Utf8 (Join-Path $dropShortcut 'config.json') ($multiFixture | ConvertTo-Json -Depth 12 -Compress)
$dropShortcutTarget = Join-Path $dropShortcut 'source.txt'
Write-Utf8 $dropShortcutTarget 'Shell shortcut drop target must remain unchanged.'
$dropShortcutPath = Join-Path $dropShortcut 'source-link.lnk'
$dropShortcutShell = New-Object -ComObject WScript.Shell
$dropShortcutObject = $null
try {
    $dropShortcutObject = $dropShortcutShell.CreateShortcut($dropShortcutPath)
    $dropShortcutObject.TargetPath = $dropShortcutTarget
    $dropShortcutObject.Save()
} finally {
    if ($dropShortcutObject) { [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($dropShortcutObject) }
    [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($dropShortcutShell)
}
Check ((Test-Path -LiteralPath $dropShortcutPath -PathType Leaf) -and (Test-Path -LiteralPath $dropShortcutTarget -PathType Leaf)) 'Shortcut drop fixture or target was not created.'
$dropShortcutHash = (Get-FileHash -LiteralPath $dropShortcutPath).Hash
$dropShortcutTargetHash = (Get-FileHash -LiteralPath $dropShortcutTarget).Hash
Run-Case 'ui-drop-reject-move-then-link-shortcut-second-group' $dropShortcut 'ui-drop-shortcut'
$dropShortcutConfig = Get-Content -LiteralPath (Join-Path $dropShortcut 'config.json') -Raw | ConvertFrom-Json
Check ($dropShortcutConfig.groups.Count -eq 2 -and $dropShortcutConfig.groups[0].id -ceq '1' -and $dropShortcutConfig.groups[0].items.Count -eq 0 -and $dropShortcutConfig.groups[1].id -ceq '7' -and $dropShortcutConfig.groups[1].items.Count -eq 1 -and $dropShortcutConfig.groups[1].items[0].id -ceq '1' -and $dropShortcutConfig.groups[1].items[0].target.kind -ceq 'shortcut' -and $dropShortcutConfig.next_item_id -ceq '2') 'Shell shortcut drop changed the wrong group, kind, or allocator.'
$dropShortcutStoredPath = -join @($dropShortcutConfig.groups[1].items[0].target.path_utf16 | ForEach-Object { [char][int]$_ })
Check ($dropShortcutStoredPath -ceq $dropShortcutPath -and $dropShortcutStoredPath -cne $dropShortcutTarget) 'Shell shortcut drop did not store the .lnk path itself.'
Check ((Get-FileHash -LiteralPath $dropShortcutPath).Hash -ceq $dropShortcutHash -and (Get-FileHash -LiteralPath $dropShortcutTarget).Hash -ceq $dropShortcutTargetHash) 'Shell shortcut drop changed the shortcut or target bytes.'
Run-Case 'reopen-ui-drop-shortcut' $dropShortcut 'writable'
$dropShortcutReopen = Get-Content -LiteralPath (Join-Path $dropShortcut 'config.json') -Raw | ConvertFrom-Json
Check ($dropShortcutReopen.groups.Count -eq 2 -and $dropShortcutReopen.groups[0].items.Count -eq 0 -and $dropShortcutReopen.groups[1].id -ceq '7' -and $dropShortcutReopen.groups[1].items.Count -eq 1 -and $dropShortcutReopen.groups[1].items[0].id -ceq '1' -and $dropShortcutReopen.groups[1].items[0].target.kind -ceq 'shortcut' -and $dropShortcutReopen.next_item_id -ceq '2') 'Shell-dropped shortcut did not survive restart.'
$dropShortcutReopenPath = -join @($dropShortcutReopen.groups[1].items[0].target.path_utf16 | ForEach-Object { [char][int]$_ })
Check ($dropShortcutReopenPath -ceq $dropShortcutPath -and $dropShortcutReopenPath -cne $dropShortcutTarget) 'Shell-dropped shortcut path did not survive restart as a .lnk reference.'
Check ((Get-FileHash -LiteralPath $dropShortcutPath).Hash -ceq $dropShortcutHash -and (Get-FileHash -LiteralPath $dropShortcutTarget).Hash -ceq $dropShortcutTargetHash) 'Shortcut drop restart changed the shortcut or target bytes.'
$dropMixed = New-Case 'ui-drop-mixed'
Write-Utf8 (Join-Path $dropMixed 'config.json') ($multiFixture | ConvertTo-Json -Depth 12 -Compress)
$dropMixedSource = Join-Path $dropMixed 'source.txt'
Write-Utf8 $dropMixedSource 'Synthetic mixed Shell drop must preserve this file.'
$dropMixedSourceHash = (Get-FileHash -LiteralPath $dropMixedSource).Hash
$dropMixedSourceFolder = Join-Path $dropMixed 'source-folder'
New-Item -ItemType Directory -Path $dropMixedSourceFolder | Out-Null
$dropMixedChild = Join-Path $dropMixedSourceFolder 'child.txt'
Write-Utf8 $dropMixedChild 'Synthetic mixed Shell drop must preserve this child.'
$dropMixedChildHash = (Get-FileHash -LiteralPath $dropMixedChild).Hash
$dropMixedEntryCount = @(Get-ChildItem -LiteralPath $dropMixedSourceFolder -Force).Count
Run-Case 'ui-drop-mixed' $dropMixed 'ui-drop-mixed'
$dropMixedConfig = Get-Content -LiteralPath (Join-Path $dropMixed 'config.json') -Raw | ConvertFrom-Json
Check ($dropMixedConfig.groups.Count -eq 2 -and $dropMixedConfig.next_group_id -ceq '8' -and $dropMixedConfig.next_item_id -ceq '3') 'Mixed Shell drop changed group count or item allocator.'
Check ($dropMixedConfig.groups[0].id -ceq '1' -and $dropMixedConfig.groups[0].title -ceq $multiFixture.groups[0].title -and $dropMixedConfig.groups[0].items.Count -eq 0) 'Mixed Shell drop changed the first group.'
Check ($dropMixedConfig.groups[1].id -ceq '7' -and $dropMixedConfig.groups[1].title -ceq $multiFixture.groups[1].title -and $dropMixedConfig.groups[1].items.Count -eq 2 -and $dropMixedConfig.groups[1].items[0].id -ceq '1' -and $dropMixedConfig.groups[1].items[0].target.kind -ceq 'file' -and $dropMixedConfig.groups[1].items[1].id -ceq '2' -and $dropMixedConfig.groups[1].items[1].target.kind -ceq 'directory') 'Mixed Shell drop did not create exactly one file and one directory reference in the second group.'
$dropMixedStoredFile = -join @($dropMixedConfig.groups[1].items[0].target.path_utf16 | ForEach-Object { [char][int]$_ })
$dropMixedStoredFolder = -join @($dropMixedConfig.groups[1].items[1].target.path_utf16 | ForEach-Object { [char][int]$_ })
Check ($dropMixedStoredFile -ceq $dropMixedSource -and $dropMixedStoredFolder -ceq $dropMixedSourceFolder) 'Mixed Shell drop stored an incorrect source path.'
Check ((Get-FileHash -LiteralPath $dropMixedSource).Hash -ceq $dropMixedSourceHash -and (Get-FileHash -LiteralPath $dropMixedChild).Hash -ceq $dropMixedChildHash) 'Mixed Shell drop changed source file bytes.'
Check (@(Get-ChildItem -LiteralPath $dropMixedSourceFolder -Force).Count -eq $dropMixedEntryCount) 'Mixed Shell drop changed source folder entry count.'
Run-Case 'reopen-ui-drop-mixed' $dropMixed 'writable'
$dropMixedReopen = Get-Content -LiteralPath (Join-Path $dropMixed 'config.json') -Raw | ConvertFrom-Json
Check ($dropMixedReopen.groups.Count -eq 2 -and $dropMixedReopen.groups[0].id -ceq '1' -and $dropMixedReopen.groups[0].items.Count -eq 0 -and $dropMixedReopen.groups[1].id -ceq '7' -and $dropMixedReopen.groups[1].items.Count -eq 2 -and $dropMixedReopen.groups[1].items[0].id -ceq '1' -and $dropMixedReopen.groups[1].items[0].target.kind -ceq 'file' -and $dropMixedReopen.groups[1].items[1].id -ceq '2' -and $dropMixedReopen.groups[1].items[1].target.kind -ceq 'directory' -and $dropMixedReopen.next_item_id -ceq '3') 'Mixed Shell drop references or stable IDs did not survive writable restart.'
$dropMixedReopenFile = -join @($dropMixedReopen.groups[1].items[0].target.path_utf16 | ForEach-Object { [char][int]$_ })
$dropMixedReopenFolder = -join @($dropMixedReopen.groups[1].items[1].target.path_utf16 | ForEach-Object { [char][int]$_ })
Check ($dropMixedReopenFile -ceq $dropMixedSource -and $dropMixedReopenFolder -ceq $dropMixedSourceFolder) 'Mixed Shell drop paths did not survive writable restart.'
Check ((Get-FileHash -LiteralPath $dropMixedSource).Hash -ceq $dropMixedSourceHash -and (Get-FileHash -LiteralPath $dropMixedChild).Hash -ceq $dropMixedChildHash) 'Mixed Shell drop restart changed source file bytes.'
Check (@(Get-ChildItem -LiteralPath $dropMixedSourceFolder -Force).Count -eq $dropMixedEntryCount) 'Mixed Shell drop restart changed source folder entry count.'
$report = [ordered]@{
    test = 'P1-P2 hidden HWND + background configuration/group lifecycle'
    results = $results.ToArray()
    not_tested = @('visual appearance', 'physical keyboard input', 'mixed DPI', 'desktop integration', 'S1 performance', 'power loss')
}
$json = $report | ConvertTo-Json -Depth 6
Write-Utf8 (Join-Path $root 'results.json') $json
$json
Write-Output "Artifacts retained: $root"

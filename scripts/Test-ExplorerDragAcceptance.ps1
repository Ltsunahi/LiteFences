param(
    [Parameter(Mandatory = $true)]
    [string] $DataDir
)

$ErrorActionPreference = 'Stop'
$projectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$artifactsRoot = [IO.Path]::GetFullPath((Join-Path $projectRoot 'artifacts')).TrimEnd('\')
$checks = [System.Collections.Generic.List[object]]::new()

function Add-Check {
    param([string] $Name, [bool] $Passed, [string] $Details)
    $checks.Add([pscustomobject]@{ name = $Name; passed = $Passed; details = $Details })
}

function Decode-Utf16Path {
    param($CodeUnits)
    if ($null -eq $CodeUnits) { throw 'path_utf16 is missing.' }
    $builder = [Text.StringBuilder]::new()
    foreach ($unit in @($CodeUnits)) {
        $number = 0
        if (-not [int]::TryParse([string]$unit, [ref]$number) -or $number -lt 0 -or $number -gt 65535) {
            throw 'path_utf16 contains a value outside the UTF-16 code-unit range.'
        }
        [void]$builder.Append([char]$number)
    }
    return $builder.ToString()
}

$resolvedDataDir = $null
$dataDirValid = $false
$candidateDataDir = $null
$config = $null
try {
    if (-not [IO.Path]::IsPathRooted($DataDir) -or $DataDir -notmatch '^[A-Za-z]:[\\/]') {
        throw '-DataDir must be an explicit absolute drive path.'
    }
    $candidateDataDir = [IO.Path]::GetFullPath($DataDir).TrimEnd('\')
    $parent = [IO.Path]::GetDirectoryName($candidateDataDir).TrimEnd('\')
    $leaf = [IO.Path]::GetFileName($candidateDataDir)
    if (-not [string]::Equals($parent, $artifactsRoot, [StringComparison]::OrdinalIgnoreCase) -or
        $leaf -notmatch '^ui-sky-\d+-\d+$') {
        throw 'Resolved directory must be a direct artifacts child named ui-sky-<pid>-<timestamp>.'
    }
    if (-not (Test-Path -LiteralPath $candidateDataDir -PathType Container)) {
        throw 'The specified data directory does not exist.'
    }
    $directory = Get-Item -LiteralPath $candidateDataDir -Force
    if (($directory.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw 'The specified data directory cannot be a reparse point.'
    }
    $resolvedDataDir = $candidateDataDir
    Add-Check 'data-directory-scope' $true $resolvedDataDir
    $dataDirValid = $true
} catch {
    Add-Check 'data-directory-scope' $false $_.Exception.Message
}

$configPath = if ($dataDirValid) { Join-Path $resolvedDataDir 'config.json' } else { $null }
if ($configPath -and (Test-Path -LiteralPath $configPath -PathType Leaf)) {
    try {
        $configFile = Get-Item -LiteralPath $configPath -Force
        if (($configFile.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw 'config.json cannot be a reparse point.'
        }
        $config = Get-Content -LiteralPath $configPath -Raw -Encoding UTF8 | ConvertFrom-Json
        Add-Check 'config-readable' $true 'config.json parsed as JSON.'
    } catch {
        Add-Check 'config-readable' $false $_.Exception.Message
    }
} else {
    Add-Check 'config-readable' $false 'config.json is unavailable because the validated directory or file is missing.'
}

$expected = @(
    [pscustomobject]@{ kind = 'file'; relative = 'artifacts/ui-drag-source/file.txt' },
    [pscustomobject]@{ kind = 'directory'; relative = 'artifacts/ui-drag-source/folder' },
    [pscustomobject]@{ kind = 'shortcut'; relative = 'artifacts/ui-shortcut-source/source-link.lnk' }
)
$items = @()
$groups = @()
if ($null -ne $config) {
    $groups = @($config.groups)
    Add-Check 'one-group' ($groups.Count -eq 1) ("Observed groups: {0}." -f $groups.Count)
    if ($groups.Count -eq 1) {
        $items = @($groups[0].items)
        Add-Check 'three-references' ($items.Count -eq 3) ("Observed references: {0}." -f $items.Count)
    } else {
        Add-Check 'three-references' $false 'Cannot validate references unless exactly one group exists.'
    }
} else {
    Add-Check 'one-group' $false 'Cannot inspect groups because config.json was not parsed.'
    Add-Check 'three-references' $false 'Cannot inspect references because config.json was not parsed.'
}

$expectedByKind = @{}
foreach ($entry in $expected) { $expectedByKind[$entry.kind] = Join-Path $projectRoot ($entry.relative.Replace('/', '\')) }
$seenKinds = @{}
foreach ($item in $items) {
    $kind = [string]$item.target.kind
    try { $decodedPath = Decode-Utf16Path $item.target.path_utf16 }
    catch { $decodedPath = ''; Add-Check ("reference-path-decode-{0}" -f $kind) $false $_.Exception.Message }
    if ($decodedPath) {
        try {
            $knownKind = @('file', 'directory', 'shortcut') -ccontains $kind
            $absoluteDrivePath = $decodedPath -cmatch '^[A-Za-z]:\\'
            $matches = $knownKind -and $absoluteDrivePath -and [string]::Equals(
                $decodedPath, $expectedByKind[$kind], [StringComparison]::OrdinalIgnoreCase)
            Add-Check ("reference-{0}-path" -f $kind) $matches ("Decoded path: {0}" -f $decodedPath)
        } catch {
            Add-Check ("reference-{0}-path" -f $kind) $false $_.Exception.Message
        }
    }
    if ($seenKinds.ContainsKey($kind)) { $seenKinds[$kind]++ } else { $seenKinds[$kind] = 1 }
}
foreach ($entry in $expected) {
    $count = if ($seenKinds.ContainsKey($entry.kind)) { $seenKinds[$entry.kind] } else { 0 }
    Add-Check ("reference-{0}-kind-count" -f $entry.kind) ($count -eq 1) ("Observed kind count: {0}." -f $count)
}

$fixtures = @(
    [pscustomobject]@{ name = 'file.txt'; path = (Join-Path $projectRoot 'artifacts/ui-drag-source/file.txt'); hash = '9F5C437A591D5C9FF9F1CCD4B0D8BB5B751A88AC337FC8F42BC797F61EB3F850'; fixed = $true },
    [pscustomobject]@{ name = 'folder/child.txt'; path = (Join-Path $projectRoot 'artifacts/ui-drag-source/folder/child.txt'); hash = '20A3B1D48E575FFB57A62A8AB1EBF832B72F813C244CC1916BC3073C86CDF111'; fixed = $true },
    [pscustomobject]@{ name = 'source-link.lnk'; path = (Join-Path $projectRoot 'artifacts/ui-shortcut-source/source-link.lnk'); hash = 'E8B9BC9658DEBD8B714605412C24284F14A4ACC106095799759515EFF9CDEAA1'; fixed = $true },
    [pscustomobject]@{ name = 'shortcut target source.txt'; path = (Join-Path $projectRoot 'artifacts/ui-shortcut-source/source.txt'); hash = '6854D82492AC4D47E80D40D77A8D4E5BAEADEC1105FAA98CBF7B28586C8A70B4'; fixed = $true }
)
$hashes = @{}
if ($dataDirValid) {
    $dragSource = Join-Path $projectRoot 'artifacts/ui-drag-source'
    $dragFolder = Join-Path $dragSource 'folder'
    $shortcutSource = Join-Path $projectRoot 'artifacts/ui-shortcut-source'
    try {
        $dragEntries = @(Get-ChildItem -LiteralPath $dragSource -Force -ErrorAction Stop)
        $dragNames = @($dragEntries | ForEach-Object { $_.Name } | Sort-Object -CaseSensitive)
        $expectedDragNames = @('file.txt', 'folder') | Sort-Object -CaseSensitive
        $dragNamesMatch = ($dragNames.Count -eq $expectedDragNames.Count) -and
            (@(Compare-Object -ReferenceObject $expectedDragNames -DifferenceObject $dragNames -CaseSensitive).Count -eq 0)
        $dragTypesMatch = ($dragEntries | Where-Object { $_.Name -ceq 'file.txt' -and -not $_.PSIsContainer }).Count -eq 1 -and
            ($dragEntries | Where-Object { $_.Name -ceq 'folder' -and $_.PSIsContainer }).Count -eq 1
        Add-Check 'fixture-list-drag-source-top-level' ($dragNamesMatch -and $dragTypesMatch) ("Observed entries: {0}. Expected file.txt and folder with matching file/directory types." -f ($dragNames -join ', '))
    } catch {
        Add-Check 'fixture-list-drag-source-top-level' $false $_.Exception.Message
    }
    try {
        $childEntries = @(Get-ChildItem -LiteralPath $dragFolder -Force -ErrorAction Stop)
        $childNames = @($childEntries | ForEach-Object { $_.Name } | Sort-Object -CaseSensitive)
        $childIsExactFile = $childEntries.Count -eq 1 -and $childEntries[0].Name -ceq 'child.txt' -and -not $childEntries[0].PSIsContainer
        Add-Check 'fixture-list-drag-source-folder' $childIsExactFile ("Observed entries: {0}. Expected only file child.txt." -f ($childNames -join ', '))
    } catch {
        Add-Check 'fixture-list-drag-source-folder' $false $_.Exception.Message
    }
    try {
        $shortcutEntries = @(Get-ChildItem -LiteralPath $shortcutSource -Force -ErrorAction Stop)
        $shortcutNames = @($shortcutEntries | ForEach-Object { $_.Name })
        $hasLink = ($shortcutEntries | Where-Object { $_.Name -ceq 'source-link.lnk' -and -not $_.PSIsContainer }).Count -ge 1
        $hasTarget = ($shortcutEntries | Where-Object { $_.Name -ceq 'source.txt' -and -not $_.PSIsContainer }).Count -ge 1
        $missingShortcutNames = @()
        if (-not $hasLink) { $missingShortcutNames += 'source-link.lnk' }
        if (-not $hasTarget) { $missingShortcutNames += 'source.txt' }
        Add-Check 'fixture-list-shortcut-source' ($hasLink -and $hasTarget) ("Observed entries: {0}. Missing required files: {1}." -f (($shortcutNames | Sort-Object -CaseSensitive) -join ', '), ($missingShortcutNames -join ', '))
    } catch {
        Add-Check 'fixture-list-shortcut-source' $false $_.Exception.Message
    }

    foreach ($fixture in $fixtures) {
    $exists = Test-Path -LiteralPath $fixture.path -PathType Leaf
    Add-Check ("fixture-exists-{0}" -f $fixture.name) $exists $fixture.path
    if ($exists) {
        try {
            $actualHash = (Get-FileHash -LiteralPath $fixture.path -Algorithm SHA256).Hash.ToUpperInvariant()
            $hashes[$fixture.name] = $actualHash
            Add-Check ("fixture-sha256-{0}" -f $fixture.name) ($actualHash -eq $fixture.hash) ("actual={0}; expected={1}" -f $actualHash, $fixture.hash)
        } catch {
            $hashes[$fixture.name] = $null
            Add-Check ("fixture-sha256-{0}" -f $fixture.name) $false $_.Exception.Message
        }
    } else {
        $hashes[$fixture.name] = $null
        Add-Check ("fixture-sha256-{0}" -f $fixture.name) $false ("File missing; expected SHA256={0}" -f $fixture.hash)
    }
    }
}

$passed = @($checks | Where-Object { -not $_.passed }).Count -eq 0
$result = [pscustomobject]@{
    result = if ($passed) { 'pass' } else { 'fail' }
    passed = $passed
    dataDir = $resolvedDataDir
    hashes = $hashes
    checks = @($checks)
    limitation = 'This read-only disk check does not prove that a real Explorer drag gesture occurred.'
}
$result | ConvertTo-Json -Depth 8
if (-not $passed) { exit 1 }

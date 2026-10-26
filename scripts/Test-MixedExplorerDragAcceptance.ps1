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
$config = $null
try {
    if (-not [IO.Path]::IsPathRooted($DataDir) -or $DataDir -notmatch '^[A-Za-z]:[\\/]') {
        throw '-DataDir must be an explicit absolute drive path.'
    }
    $candidateDataDir = [IO.Path]::GetFullPath($DataDir).TrimEnd('\')
    if (-not [string]::Equals($DataDir.TrimEnd('\'), $candidateDataDir, [StringComparison]::OrdinalIgnoreCase)) {
        throw '-DataDir must already be a canonical absolute path without dot segments.'
    }
    $parent = [IO.Path]::GetDirectoryName($candidateDataDir).TrimEnd('\')
    $leaf = [IO.Path]::GetFileName($candidateDataDir)
    if (-not [string]::Equals($parent, $artifactsRoot, [StringComparison]::OrdinalIgnoreCase) -or
        $leaf -notmatch '^ui-sky-\d+-\d+$') {
        throw 'Resolved directory must be a direct artifacts child named ui-sky-<digits>-<digits>.'
    }
    if (-not (Test-Path -LiteralPath $candidateDataDir -PathType Container)) {
        throw 'The specified data directory does not exist.'
    }
    $directory = Get-Item -LiteralPath $candidateDataDir -Force
    if (($directory.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw 'The specified data directory cannot be a reparse point.'
    }
    $resolvedDataDir = $candidateDataDir
    $dataDirValid = $true
    Add-Check 'data-directory-scope' $true $resolvedDataDir
} catch {
    Add-Check 'data-directory-scope' $false $_.Exception.Message
}

# Config is deliberately not touched unless the requested directory passed scope validation.
if ($dataDirValid) {
    $configPath = Join-Path $resolvedDataDir 'config.json'
    if (Test-Path -LiteralPath $configPath -PathType Leaf) {
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
        Add-Check 'config-readable' $false 'config.json is missing.'
    }
} else {
    Add-Check 'config-readable' $false 'Config was not read because the data directory failed scope validation.'
}

$expected = @(
    [pscustomobject]@{ kind = 'file'; path = [IO.Path]::GetFullPath((Join-Path $projectRoot 'artifacts/ui-drag-source/file.txt')) },
    [pscustomobject]@{ kind = 'directory'; path = [IO.Path]::GetFullPath((Join-Path $projectRoot 'artifacts/ui-drag-source/folder')) }
)
$items = @()
if ($null -ne $config) {
    $groups = @($config.groups)
    Add-Check 'one-group' ($groups.Count -eq 1) ("Observed groups: {0}." -f $groups.Count)
    if ($groups.Count -eq 1) {
        $items = @($groups[0].items)
        Add-Check 'two-references' ($items.Count -eq 2) ("Observed references: {0}." -f $items.Count)
    } else {
        Add-Check 'two-references' $false 'Cannot validate references unless exactly one group exists.'
    }
} else {
    Add-Check 'one-group' $false 'Cannot inspect groups because config.json was not parsed.'
    Add-Check 'two-references' $false 'Cannot inspect references because config.json was not parsed.'
}

$observed = @{}
foreach ($item in $items) {
    $kind = [string]$item.target.kind
    try {
        $decodedPath = Decode-Utf16Path $item.target.path_utf16
        $expectedEntry = @($expected | Where-Object { $_.kind -ceq $kind })
        $matches = $expectedEntry.Count -eq 1 -and
            [string]::Equals($decodedPath, $expectedEntry[0].path, [StringComparison]::Ordinal)
        Add-Check ("reference-{0}-path" -f $kind) $matches ("Decoded path: {0}" -f $decodedPath)
    } catch {
        Add-Check ("reference-{0}-path" -f $kind) $false $_.Exception.Message
    }
    if ($observed.ContainsKey($kind)) { $observed[$kind]++ } else { $observed[$kind] = 1 }
}
foreach ($entry in $expected) {
    $count = if ($observed.ContainsKey($entry.kind)) { $observed[$entry.kind] } else { 0 }
    Add-Check ("reference-{0}-kind-count" -f $entry.kind) ($count -eq 1) ("Observed kind count: {0}." -f $count)
}

$sourceDir = Join-Path $projectRoot 'artifacts/ui-drag-source'
$sourceFolder = Join-Path $sourceDir 'folder'
try {
    $sourceEntries = @(Get-ChildItem -LiteralPath $sourceDir -Force -ErrorAction Stop)
    $sourceNames = @($sourceEntries | ForEach-Object { $_.Name } | Sort-Object -CaseSensitive)
    $expectedNames = @('file.txt', 'folder') | Sort-Object -CaseSensitive
    $namesMatch = $sourceEntries.Count -eq 2 -and
        @(Compare-Object -ReferenceObject $expectedNames -DifferenceObject $sourceNames -CaseSensitive).Count -eq 0
    $typesMatch = @($sourceEntries | Where-Object { $_.Name -ceq 'file.txt' -and -not $_.PSIsContainer }).Count -eq 1 -and
        @($sourceEntries | Where-Object { $_.Name -ceq 'folder' -and $_.PSIsContainer }).Count -eq 1
    Add-Check 'fixture-list-drag-source-top-level' ($namesMatch -and $typesMatch) ("Observed entries: {0}. Expected only file.txt and folder with matching file/directory types." -f ($sourceNames -join ', '))
} catch {
    Add-Check 'fixture-list-drag-source-top-level' $false $_.Exception.Message
}
try {
    $childEntries = @(Get-ChildItem -LiteralPath $sourceFolder -Force -ErrorAction Stop)
    $childIsExactFile = $childEntries.Count -eq 1 -and $childEntries[0].Name -ceq 'child.txt' -and -not $childEntries[0].PSIsContainer
    Add-Check 'fixture-list-drag-source-folder' $childIsExactFile ("Observed entries: {0}. Expected only file child.txt." -f (($childEntries | ForEach-Object { $_.Name } | Sort-Object -CaseSensitive) -join ', '))
} catch {
    Add-Check 'fixture-list-drag-source-folder' $false $_.Exception.Message
}

$fixtures = @(
    [pscustomobject]@{ name = 'file.txt'; path = (Join-Path $sourceDir 'file.txt'); hash = '9F5C437A591D5C9FF9F1CCD4B0D8BB5B751A88AC337FC8F42BC797F61EB3F850' },
    [pscustomobject]@{ name = 'folder/child.txt'; path = (Join-Path $sourceFolder 'child.txt'); hash = '20A3B1D48E575FFB57A62A8AB1EBF832B72F813C244CC1916BC3073C86CDF111' }
)
$hashes = @{}
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

$passed = @($checks | Where-Object { -not $_.passed }).Count -eq 0
$result = [pscustomobject]@{
    result = if ($passed) { 'pass' } else { 'fail' }
    passed = $passed
    dataDir = $resolvedDataDir
    hashes = $hashes
    checks = @($checks)
    limitation = 'This read-only disk check cannot prove that both artifacts were physically dragged together in one Explorer gesture.'
}
$result | ConvertTo-Json -Depth 8
if (-not $passed) { exit 1 }

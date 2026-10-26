[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'

function Get-SafeValue { param([scriptblock]$Action) try { & $Action } catch { 'unavailable' } }
function Get-ToolVersion {
    param([string]$Name, [string]$Argument)
    try {
        $command = Get-Command $Name -CommandType Application -ErrorAction Stop | Select-Object -First 1
        $text = (& $command.Source $Argument 2>&1 | Out-String).Trim()
        if ([string]::IsNullOrWhiteSpace($text)) { 'unavailable' } else { $text.Split("`r`n")[0].Trim() }
    } catch { 'unavailable' }
}
function Get-WindowsSdkVersions {
    try {
        $roots = @('HKLM:\SOFTWARE\Microsoft\Windows Kits\Installed Roots','HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows Kits\Installed Roots')
        $versions = foreach ($path in $roots) {
            if (Test-Path $path) {
                $item = Get-ItemProperty $path
                foreach ($property in $item.PSObject.Properties) {
                    if ($property.Name -like 'KitsRoot*' -and $property.Value) {
                        $lib = Join-Path ([string]$property.Value) 'Lib'
                        if (Test-Path $lib) { Get-ChildItem -LiteralPath $lib -Directory | Select-Object -ExpandProperty Name }
                    }
                }
            }
        }
        $versions = @($versions | Where-Object { $_ -match '^10\.\d+\.\d+\.\d+$' } | Sort-Object -Unique)
        if ($versions.Count) { $versions } else { 'unavailable' }
    } catch { 'unavailable' }
}
function Get-MsvcToolVersion {
    try {
        $versionFiles = @('E:\vs\VC\Auxiliary\Build\Microsoft.VCToolsVersion.default.txt')
        $vswhere = 'C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe'
        if (Test-Path -LiteralPath $vswhere) {
            $installationPath = (& $vswhere '-latest' '-products' '*' '-requires' 'Microsoft.VisualStudio.Component.VC.Tools.x86.x64' '-property' 'installationPath' 2>$null | Select-Object -First 1).Trim()
            if ($installationPath) { $versionFiles += Join-Path $installationPath 'VC\Auxiliary\Build\Microsoft.VCToolsVersion.default.txt' }
        }
        foreach ($file in $versionFiles) { if (Test-Path -LiteralPath $file) { $value = (Get-Content -LiteralPath $file -Raw).Trim(); if ($value) { return $value } } }
        'unavailable'
    } catch { 'unavailable' }
}

$savedPath = $env:Path; $savedCargoHome = $env:CARGO_HOME; $savedRustupHome = $env:RUSTUP_HOME
$savedAutoInstall = $env:RUSTUP_AUTO_INSTALL
try {
    $env:RUSTUP_AUTO_INSTALL = '0'
    $toolsRoot = Join-Path (Split-Path $PSScriptRoot -Parent) '.tools'
    $cargoRoot = Join-Path $toolsRoot 'cargo'; $rustupRoot = Join-Path $toolsRoot 'rustup'
    if (Test-Path (Join-Path $cargoRoot 'bin')) { $env:Path = (Join-Path $cargoRoot 'bin') + [IO.Path]::PathSeparator + $env:Path }
    if (Test-Path $cargoRoot) { $env:CARGO_HOME = $cargoRoot }; if (Test-Path $rustupRoot) { $env:RUSTUP_HOME = $rustupRoot }
    $os = Get-SafeValue { Get-CimInstance Win32_OperatingSystem }; $gpu = Get-SafeValue { @(Get-CimInstance Win32_VideoController -ErrorAction Stop) }
    $cpuModel = Get-SafeValue { (Get-CimInstance Win32_Processor -ErrorAction Stop | Select-Object -First 1).Name }
    $build = if ($os -and $os.BuildNumber) { [string]$os.BuildNumber } else { Get-SafeValue { [string](Get-ItemPropertyValue -LiteralPath 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion' -Name CurrentBuild -ErrorAction Stop) } }
    $ubr = Get-SafeValue { [string](Get-ItemPropertyValue -LiteralPath 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion' -Name UBR -ErrorAction Stop) }
    $fullBuild = if ($build -ne 'unavailable' -and $ubr -ne 'unavailable') { "$build.$ubr" } else { 'unavailable' }
    [ordered]@{
        os = [ordered]@{ caption = if ($os -and $os.Caption) { [string]$os.Caption } else { 'unavailable' }; build = $build; ubr = $ubr; full_build = $fullBuild; architecture = Get-SafeValue { $a = [Environment]::GetEnvironmentVariable('PROCESSOR_ARCHITECTURE'); if ($a) { $a.ToLowerInvariant() } else { 'unavailable' } } }
        cpu = [ordered]@{ model = $cpuModel; logical_processors = Get-SafeValue { [Environment]::ProcessorCount } }
        ram = [ordered]@{ total_bytes = if ($os -and $os.TotalVisibleMemorySize) { [int64]$os.TotalVisibleMemorySize * 1KB } else { 'unavailable' } }
        gpu = if ($gpu -and $gpu.Count -gt 0) { @($gpu | ForEach-Object { [ordered]@{ name = if ($_.Name) { [string]$_.Name } else { 'unavailable' }; driver_version = if ($_.DriverVersion) { [string]$_.DriverVersion } else { 'unavailable' } } }) } else { @([ordered]@{ name = 'unavailable'; driver_version = 'unavailable' }) }
        toolchain = [ordered]@{ rustc = Get-ToolVersion 'rustc.exe' '--version'; cargo = Get-ToolVersion 'cargo.exe' '--version'; msvc = Get-MsvcToolVersion; windows_sdk = Get-WindowsSdkVersions }
    } | ConvertTo-Json -Depth 8 -Compress
} finally { $env:Path = $savedPath; $env:CARGO_HOME = $savedCargoHome; $env:RUSTUP_HOME = $savedRustupHome; $env:RUSTUP_AUTO_INSTALL = $savedAutoInstall }

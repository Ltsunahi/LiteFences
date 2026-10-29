# 构建 MSI 安装包（WiX v5+）。
#
# 用法：
#   .\scripts\Build-Msi.ps1                        # 版本取 Cargo.toml，exe 取 target\release
#   .\scripts\Build-Msi.ps1 -Version 0.2.0 -Out dist\LiteFences-0.2.0-x64.msi
#
# 依赖：dotnet tool install --global wix --version 5.*（或把 wix.exe 放进 PATH）
[CmdletBinding()]
param(
    [string]$Version,
    [string]$Exe,
    [string]$Out
)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent

if (-not $Version) {
    $match = Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
    if (-not $match) { throw 'Cargo.toml 里没找到 version' }
    $Version = $match.Matches[0].Groups[1].Value
}
if (-not $Exe) { $Exe = Join-Path $root 'target\release\litefences-desktop.exe' }
if (-not (Test-Path -LiteralPath $Exe)) {
    throw "找不到 exe：$Exe`n先构建：cargo build --release --features shell --locked"
}
if (-not $Out) { $Out = Join-Path $root "dist\LiteFences-$Version-x64.msi" }

$wix = (Get-Command wix.exe -ErrorAction SilentlyContinue).Source
if (-not $wix) {
    $candidate = Join-Path $env:USERPROFILE '.dotnet\tools\wix.exe'
    if (Test-Path -LiteralPath $candidate) { $wix = $candidate }
}
if (-not $wix) {
    throw '找不到 wix。先运行：dotnet tool install --global wix --version 5.*'
}

# 打包目录：exe 改名成安装后的文件名，说明文件跟着一起进包。
$stage = Join-Path $env:TEMP ('litefences-msi-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $stage | Out-Null
try {
    Copy-Item -LiteralPath $Exe -Destination (Join-Path $stage 'LiteFencesDesktop.exe') -Force
    Copy-Item -LiteralPath (Join-Path $root 'packaging\使用说明.txt') -Destination (Join-Path $stage '使用说明.txt') -Force
    Copy-Item -LiteralPath (Join-Path $root 'LICENSE') -Destination (Join-Path $stage 'LICENSE.txt') -Force
    New-Item -ItemType Directory -Force -Path (Split-Path $Out) | Out-Null
    $licenseRtf = Join-Path $root 'packaging\LICENSE.rtf'
    & $wix build (Join-Path $root 'packaging\LiteFences.wxs') `
        -ext WixToolset.UI.wixext `
        -arch x64 `
        -d "SourceDir=$stage" `
        -d "Version=$Version" `
        -d "LicenseRtf=$licenseRtf" `
        -o $Out
    if ($LASTEXITCODE -ne 0) { throw "wix build 失败（退出码 $LASTEXITCODE）" }
    Write-Output $Out
} finally {
    Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue
}
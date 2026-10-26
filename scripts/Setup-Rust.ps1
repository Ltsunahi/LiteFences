[CmdletBinding()]
param([switch]$UseMirror)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path $PSScriptRoot -Parent
$toolRoot = Join-Path $projectRoot '.tools'
New-Item -ItemType Directory -Force -Path $toolRoot | Out-Null
$installer = Join-Path $toolRoot 'rustup-init.exe'
$partialInstaller = Join-Path $toolRoot 'rustup-init.download'
# Download to a temporary name: interrupted downloads must never be executed.
$downloadRoot = if ($UseMirror) { 'https://rsproxy.cn' } else { 'https://static.rust-lang.org' }
& "$env:SystemRoot\System32\curl.exe" --fail --silent --show-error --location --retry 2 --connect-timeout 20 --max-time 180 --output $partialInstaller "$downloadRoot/rustup/dist/x86_64-pc-windows-msvc/rustup-init.exe"
if ($LASTEXITCODE -ne 0) { throw 'Official rustup download failed; incomplete file was not executed.' }
$expectedHash = (& "$env:SystemRoot\System32\curl.exe" --fail --silent --show-error --location --connect-timeout 20 --max-time 60 'https://static.rust-lang.org/rustup/dist/x86_64-pc-windows-msvc/rustup-init.exe.sha256' | Out-String).Trim().Split(' ')[0]
if ($LASTEXITCODE -ne 0 -or $expectedHash -notmatch '^[a-fA-F0-9]{64}$') { throw 'Could not retrieve official installer SHA-256.' }
if ((Get-FileHash -LiteralPath $partialInstaller -Algorithm SHA256).Hash -ne $expectedHash) { throw 'Installer checksum mismatch; installer will not be run.' }
Move-Item -LiteralPath $partialInstaller -Destination $installer -Force
$previousCargoHome = $env:CARGO_HOME
$previousRustupHome = $env:RUSTUP_HOME
$previousDistServer = $env:RUSTUP_DIST_SERVER
$previousUpdateRoot = $env:RUSTUP_UPDATE_ROOT
try {
    $env:CARGO_HOME = Join-Path $toolRoot 'cargo'
    $env:RUSTUP_HOME = Join-Path $toolRoot 'rustup'
    if ($UseMirror) {
        $env:RUSTUP_DIST_SERVER = 'https://rsproxy.cn'
        $env:RUSTUP_UPDATE_ROOT = 'https://rsproxy.cn/rustup'
    }
    $toolchain = 'stable'
    $pin = Join-Path $projectRoot 'rust-toolchain.toml'
    if (Test-Path -LiteralPath $pin) {
        $match = [regex]::Match((Get-Content -LiteralPath $pin -Raw), 'channel\s*=\s*"([^"]+)"')
        if (-not $match.Success) { throw 'Invalid rust-toolchain.toml channel.' }
        $toolchain = $match.Groups[1].Value
    }
    & $installer -y --no-modify-path --profile minimal --default-host x86_64-pc-windows-msvc --default-toolchain $toolchain --component rustfmt --component clippy
    if ($LASTEXITCODE -ne 0) { throw "Rust setup failed ($LASTEXITCODE)." }
    & (Join-Path $env:CARGO_HOME 'bin\rustc.exe') --version
    if ($LASTEXITCODE -ne 0) { throw 'Rust compiler validation failed.' }
} finally {
    $env:CARGO_HOME = $previousCargoHome
    $env:RUSTUP_HOME = $previousRustupHome
    $env:RUSTUP_DIST_SERVER = $previousDistServer
    $env:RUSTUP_UPDATE_ROOT = $previousUpdateRoot
}

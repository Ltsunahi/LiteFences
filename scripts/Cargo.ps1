[CmdletBinding()]
param(
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$CargoArguments
)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path $PSScriptRoot -Parent
$previousCargoHome = $env:CARGO_HOME
$previousRustupHome = $env:RUSTUP_HOME
$previousPath = $env:Path
Push-Location $projectRoot
try {
    $env:CARGO_HOME = Join-Path $projectRoot '.tools\cargo'
    $env:RUSTUP_HOME = Join-Path $projectRoot '.tools\rustup'
    $cargo = Join-Path $env:CARGO_HOME 'bin\cargo.exe'
    if (-not (Test-Path -LiteralPath $cargo)) { throw 'Run scripts/Setup-Rust.ps1 first.' }
    $env:Path = (Join-Path $env:CARGO_HOME 'bin') + [IO.Path]::PathSeparator + $env:Path
    & $cargo @CargoArguments
    $cargoExit = $LASTEXITCODE
} finally {
    Pop-Location
    $env:CARGO_HOME = $previousCargoHome
    $env:RUSTUP_HOME = $previousRustupHome
    $env:Path = $previousPath
}
exit $cargoExit

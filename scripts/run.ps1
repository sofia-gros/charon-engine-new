param(
    [ValidateRange(1, 32)][int]$Workers = 4,
    [string]$Grid = '480x270',
    [switch]$Smoke,
    [switch]$Play,
    [switch]$RecordInputs,
    [switch]$Software,
    [switch]$DebugBuild
)
$ErrorActionPreference = 'Stop'
Push-Location (Split-Path $PSScriptRoot -Parent)
try {
    $stamp = Get-Date -Format 'yyyyMMdd-HHmmss-fff'
    $directory = 'artifacts/local'
    New-Item -ItemType Directory -Force $directory | Out-Null
    & rustc --version | Out-File "$directory/$stamp-toolchain.txt"
    if ($LASTEXITCODE -ne 0) { throw 'Rust toolchain is unavailable.' }
    $cargoArgs = @('run', '--locked', '-p', 'charon-sandbox')
    if (-not $DebugBuild) { $cargoArgs += '--release' }
    $cargoArgs += @('--', '--workers', "$Workers", '--grid', $Grid)
    if ($Smoke) { $cargoArgs += '--smoke' }
    else { $cargoArgs += @('--record', "$directory/$stamp-metrics.csv") }
    if ($RecordInputs) { $cargoArgs += @('--input-log', "$directory/$stamp-input.trace") }
    if ($Play) { $cargoArgs += '--play' }
    if ($Software) { $cargoArgs += '--software' }
    & cargo @cargoArgs
    if ($LASTEXITCODE -ne 0) { throw "Sandbox exited with code $LASTEXITCODE" }
}
finally { Pop-Location }

param(
    [ValidateRange(1, 32)][int]$Workers = 4,
    [string]$Grid = '480x270',
    [ValidateSet('gpu', 'cpu')][string]$FluidBackend = 'gpu',
    [switch]$NoFluidTiming,
    [switch]$Smoke,
    [switch]$Play,
    [switch]$Game,
    [switch]$World,
    [switch]$WorldGame,
    [string]$WorldDirectory = 'artifacts/local/worlds',
    [UInt64]$Seed = 42,
    [switch]$RecordInputs,
    [switch]$Software,
    [switch]$DebugBuild
)
$ErrorActionPreference = 'Stop'
Push-Location (Split-Path $PSScriptRoot -Parent)
try {
    if ($WorldGame -and ($World -or $Game -or $Play -or $Smoke)) { throw '-WorldGame is a separate mode; use it without -World, -Game, -Play or -Smoke.' }
    if ($World -and ($Game -or $Play -or $Smoke)) { throw '-World cannot be combined with -Game, -Play or -Smoke.' }
    if ($Game -and $Smoke) { throw '-Game opens a start screen; use it without -Smoke.' }
    $stamp = Get-Date -Format 'yyyyMMdd-HHmmss-fff'
    $directory = 'artifacts/local'
    New-Item -ItemType Directory -Force $directory | Out-Null
    & rustc --version | Out-File "$directory/$stamp-toolchain.txt"
    if ($LASTEXITCODE -ne 0) { throw 'Rust toolchain is unavailable.' }
    $cargoArgs = @('run', '--locked', '-p', 'charon-sandbox')
    if (-not $DebugBuild) { $cargoArgs += '--release' }
    $cargoArgs += @('--', '--workers', "$Workers", '--grid', $Grid, '--fluid-backend', $FluidBackend)
    if ($NoFluidTiming) { $cargoArgs += '--no-fluid-timing' }
    if ($Smoke) { $cargoArgs += '--smoke' }
    else { $cargoArgs += @('--record', "$directory/$stamp-metrics.csv") }
    if ($RecordInputs) { $cargoArgs += @('--input-log', "$directory/$stamp-input.trace") }
    if ($WorldGame) { $cargoArgs += @('--world-game', "$Seed", '--world-dir', $WorldDirectory) }
    elseif ($World) { $cargoArgs += @('--world', "$Seed", '--world-dir', $WorldDirectory) }
    elseif ($Game) { $cargoArgs += @('--game', "$Seed") }
    elseif ($Play) { $cargoArgs += '--play' }
    if ($Software) { $cargoArgs += '--software' }
    & cargo @cargoArgs
    if ($LASTEXITCODE -ne 0) { throw "Sandbox exited with code $LASTEXITCODE" }
}
finally { Pop-Location }

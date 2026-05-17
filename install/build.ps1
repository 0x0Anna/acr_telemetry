# Build release binaries, stage install payload, zip, and optionally compile Inno Setup.
# Run from repo root:  powershell -File install\build.ps1
# CI (after cargo build):  powershell -File install/build.ps1 -SkipCargoBuild

param(
    [switch]$SkipCargoBuild,
    [switch]$SkipInnoSetup
)

$ErrorActionPreference = 'Stop'

$InstallDir = $PSScriptRoot
$Root = (Resolve-Path (Join-Path $InstallDir '..')).Path

function Get-CargoVersion {
    $line = Select-String -Path (Join-Path $Root 'Cargo.toml') -Pattern '^\s*version\s*=\s*"(.+)"\s*$' | Select-Object -First 1
    if (-not $line) { throw 'Could not read version from Cargo.toml' }
    return $line.Matches.Groups[1].Value
}

$Version = Get-CargoVersion

$Bins = @(
    'acr_recorder',
    'acr_export',
    'acr_motec',
    'acr_telemetry_bridge',
    'acr_analysis_export',
    'acr_track_match',
    'acr_timing',
    'acr_rtss_osd'
)

if (-not $SkipCargoBuild) {
    Write-Host "==> cargo build --release (version $Version)"
    $cargoArgs = @('build', '--release', '--features', 'acr_timing_bin')
    foreach ($bin in $Bins) { $cargoArgs += @('--bin', $bin) }
    Push-Location $Root
    try {
        & cargo @cargoArgs
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    } finally { Pop-Location }
} else {
    Write-Host "==> Skipping cargo build (version $Version)"
}

$Staging = Join-Path $InstallDir 'staging'
if (Test-Path $Staging) { Remove-Item $Staging -Recurse -Force }
New-Item -ItemType Directory -Path $Staging | Out-Null
foreach ($sub in @('batch', 'docs', 'config-examples', 'reference_tracks', 'telemetry_raw')) {
    New-Item -ItemType Directory -Path (Join-Path $Staging $sub) -Force | Out-Null
}

$ReleaseDir = Join-Path $Root 'target\release'
foreach ($bin in $Bins) {
    $src = Join-Path $ReleaseDir "$bin.exe"
    if (-not (Test-Path $src)) { throw "Missing binary: $src" }
    Copy-Item $src $Staging
}

Get-ChildItem (Join-Path $InstallDir 'config') -Filter '*.toml' | ForEach-Object {
    Copy-Item $_.FullName $Staging
}

Copy-Item (Join-Path $Root 'batch\*.bat') (Join-Path $Staging 'batch')
Copy-Item (Join-Path $Root 'LICENSE') $Staging
Copy-Item (Join-Path $InstallDir 'PACKAGE_README.txt') (Join-Path $Staging 'README.txt')

$docsSrc = Join-Path $Root 'docs'
if (Test-Path $docsSrc) {
    Get-ChildItem $docsSrc -Filter '*.md' -File | ForEach-Object {
        Copy-Item $_.FullName (Join-Path $Staging 'docs')
    }
}

$configExamplesSrc = Join-Path $Root 'config-examples'
if (Test-Path $configExamplesSrc) {
    Get-ChildItem $configExamplesSrc -Filter '*.toml' -File | ForEach-Object {
        Copy-Item $_.FullName (Join-Path $Staging 'config-examples')
    }
}

$timingSrc = Join-Path $Root 'timing'
$timingDst = Join-Path $Staging 'timing'
New-Item -ItemType Directory -Path $timingDst -Force | Out-Null
if (Test-Path $timingSrc) {
    Get-ChildItem $timingSrc -File | ForEach-Object { Copy-Item $_.FullName $timingDst }
    foreach ($sub in @('timing_sectors', 'overall_markers')) {
        $subSrc = Join-Path $timingSrc $sub
        if (Test-Path $subSrc) {
            Copy-Item $subSrc (Join-Path $timingDst $sub) -Recurse -Force
        }
    }
}
Copy-Item (Join-Path $InstallDir 'assets\timing\README.txt') $timingDst -Force
New-Item -ItemType Directory -Path (Join-Path $timingDst 'runs') -Force | Out-Null

$refSrc = Join-Path $Root 'reference_tracks'
$refDst = Join-Path $Staging 'reference_tracks'
if (Test-Path $refSrc) {
    Get-ChildItem $refSrc -File | Where-Object {
        $_.Extension -match '^\.(shp|shx|dbf|cpg|qix)$'
    } | ForEach-Object { Copy-Item $_.FullName $refDst }
}
Copy-Item (Join-Path $InstallDir 'assets\reference_tracks\README.txt') $refDst -Force

$OutDir = Join-Path $Root 'target\install'
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

$ZipName = "ACR_Recorder_${Version}_windows-x64_portable.zip"
$ZipPath = Join-Path $OutDir $ZipName
if (Test-Path $ZipPath) { Remove-Item $ZipPath -Force }
Compress-Archive -Path (Join-Path $Staging '*') -DestinationPath $ZipPath
Write-Host "==> Portable zip: $ZipPath"

$Iscc = @(
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "${env:ProgramFiles}\Inno Setup 6\ISCC.exe"
) | Where-Object { Test-Path $_ } | Select-Object -First 1

if ($SkipInnoSetup) {
    Write-Host '==> Inno Setup skipped (-SkipInnoSetup).'
} elseif ($Iscc) {
    Write-Host "==> Inno Setup: $Iscc"
    Push-Location $InstallDir
    try {
        & $Iscc "/DMyAppVersion=$Version" 'ACR_Recorder.iss'
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    } finally { Pop-Location }
    Write-Host "==> Setup EXE: $(Join-Path $OutDir "ACR_Recorder_${Version}_setup.exe")"
} else {
    Write-Host '==> Inno Setup not found – skipped setup.exe.'
    Write-Host '    Staging folder: install\staging\'
}

Write-Host 'Done.'

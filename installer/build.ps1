# Builds nook.exe and packs it into dist\Nook-Setup-<version>.exe.
#
#   powershell -ExecutionPolicy Bypass -File installer\build.ps1
#
# Needs Inno Setup 6.7 or newer:
#   winget install --id JRSoftware.InnoSetup -e

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot

$manifest = Get-Content (Join-Path $root 'Cargo.toml') -Raw
if ($manifest -notmatch '(?ms)^\[workspace\.package\].*?^version\s*=\s*"([^"]+)"') {
    throw 'No version in [workspace.package] of Cargo.toml'
}
$version = $Matches[1]

$iscc = Get-Command ISCC.exe -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Source
if (-not $iscc) {
    $iscc = @(
        "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
        "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
        "$env:ProgramFiles\Inno Setup 6\ISCC.exe"
    ) | Where-Object { Test-Path $_ } | Select-Object -First 1
}
if (-not $iscc) {
    throw 'Inno Setup not found. Install it with: winget install --id JRSoftware.InnoSetup -e'
}

Write-Host "Building Nook $version"
cargo build --release --manifest-path (Join-Path $root 'Cargo.toml')
if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }

& $iscc /Q "/DAppVersion=$version" (Join-Path $PSScriptRoot 'nook.iss')
if ($LASTEXITCODE -ne 0) { throw 'ISCC failed' }

Write-Host "Wrote $(Join-Path $root "dist\Nook-Setup-$version.exe")"

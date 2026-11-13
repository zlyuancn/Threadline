# Everything `tauri build` must do before bundling.
#
# The frontend is NOT rebuilt by Tauri on its own (frontendDist just points at dist/),
# so forgetting `pnpm build` silently ships the previous UI. Making it part of
# beforeBuildCommand removes that trap.
# ASCII-only: Windows PowerShell 5.1 reads .ps1 as ANSI.
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot

# Rebuild the core first: bundle-sidecars.ps1 only copies an existing exe, so without this
# a `pnpm tauri build` after editing crates/tl-core would silently ship the previous core.
$cmd = Get-Command cargo -ErrorAction SilentlyContinue
$cargo = if ($cmd) { $cmd.Source } else { Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe' }
if (-not (Test-Path $cargo)) { throw "cargo not found: $cargo (install Rustup first)" }
Push-Location $root
try {
    & $cargo build --release -p tl-core
    if ($LASTEXITCODE -ne 0) { throw 'release core build failed' }
}
finally {
    Pop-Location
}

# Note: $LASTEXITCODE is only meaningful for native commands; a .ps1 call leaves it
# untouched, so do not test it here (that check used to fail the whole build).
& (Join-Path $PSScriptRoot 'bundle-sidecars.ps1')

Push-Location $root
try {
    & pnpm build
    if ($LASTEXITCODE -ne 0) { throw 'pnpm build failed' }
}
finally {
    Pop-Location
}

Write-Host 'prebuild: sidecars staged + frontend built'

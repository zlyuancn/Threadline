# Stage the Rust binaries as Tauri sidecars (same folder after install, which is what
# tl.exe expects when it looks for its siblings).
#
# IMPORTANT: this must run before EVERY `tauri build`. Otherwise the installer ships a
# stale tl-core.exe and fixes silently don't reach the running app. It is wired as
# `bundle.beforeBuildCommand` in tauri.conf.json so it cannot be forgotten.
#
# ASCII-only (Windows PowerShell 5.1 reads .ps1 as ANSI).
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$bin = Join-Path $root 'src-tauri\binaries'
$triple = 'x86_64-pc-windows-msvc'

# Only the release profile is shippable. Picking "the freshest of release/debug" used to
# ship a debug core whenever release happened to be stale or missing -- which is exactly
# the case for anyone who just cloned the repo and ran `pnpm tauri build`.
$profileDir = Join-Path $root 'target\release'

if (-not (Test-Path (Join-Path $profileDir 'tl-core.exe'))) {
    throw "no release tl-core.exe found -- run 'cargo build --release -p tl-core' first"
}

New-Item -ItemType Directory -Force -Path $bin | Out-Null

$pairs = @(
    @{ src = Join-Path $profileDir 'tl-core.exe'; dst = "tl-core-$triple.exe" }
)

foreach ($p in $pairs) {
    if (-not (Test-Path $p.src)) { throw "missing $($p.src) -- run 'cargo build -p tl-core' first" }
    Copy-Item $p.src (Join-Path $bin $p.dst) -Force
    $stamp = (Get-Item $p.src).LastWriteTime.ToString('yyyy-MM-dd HH:mm:ss')
    Write-Host "staged $($p.dst)  (built $stamp)"
}

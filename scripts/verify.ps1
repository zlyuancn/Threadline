# Threadline local gate: fmt + clippy + test + architecture check (msocheck).
# Any failing step fails the whole gate. Called by ./verify.sh, or run directly.
# NOTE: keep this file ASCII-only -- Windows PowerShell 5.1 reads .ps1 as ANSI and would
# mangle non-ASCII text, breaking quoting.
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$cmd = Get-Command cargo -ErrorAction SilentlyContinue
$cargo = if ($cmd) { $cmd.Source } else { Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe' }
if (-not (Test-Path $cargo)) { throw "cargo not found: $cargo (install Rustup first)" }

function Step([string]$name, [scriptblock]$action) {
    Write-Host "== $name" -ForegroundColor Cyan
    & $action
    if ($LASTEXITCODE -ne 0) { throw "$name failed (exit $LASTEXITCODE)" }
}

Push-Location $root
try {
    Step 'cargo fmt --check' { & $cargo fmt --all -- --check }
    Step 'cargo clippy -D warnings' { & $cargo clippy --workspace --all-targets -- -D warnings }
    Step 'cargo test' { & $cargo test --workspace }

    # Guard: a stale tl-agent.exe copy next to the core is how one ends up running an old UI.
    # Wake resolves the real build under src-tauri\target\debug, so this copy must not exist.
    Step 'no stale agent copy' {
        # The agent's binary name depends on how it was built (tl-agent.exe from cargo,
        # Threadline.exe after Tauri's mainBinaryName), so check both.
        foreach ($name in @('tl-agent.exe', 'Threadline.exe')) {
            $stale = Join-Path $root "target\debug\$name"
            if (Test-Path $stale) {
                throw "stale copy found: $stale -- delete it (wake finds the real build itself)"
            }
        }
    }

    # Frontend must be built BEFORE the agent: tauri-build embeds ..\dist at compile time.
    if (Test-Path (Join-Path $root 'node_modules')) {
        Step 'frontend test (vitest)' { cmd /c 'pnpm test' }
        Step 'frontend build (vue-tsc + vite)' { cmd /c 'pnpm build' }
    } else {
        # A gate that skips half its steps and still prints GREEN is worse than no gate.
        throw "node_modules missing -- run 'pnpm install' first (frontend test/build cannot be skipped)"
    }

    # src-tauri is its own workspace (Tauri's dependency graph must not leak into crates/*),
    # so the root fmt/clippy above never see it.
    #
    # fmt never touches the target directory, so it always runs -- even with the app open.
    Push-Location (Join-Path $root 'src-tauri')
    try { Step 'cargo fmt --check (agent)' { & $cargo fmt -- --check } } finally { Pop-Location }

    # clippy/test cannot build while the app is running: tauri-build deletes and recopies the
    # sidecar next to the exe, and a running tl-core.exe holds it. Skipping these silently
    # used to print "Local gate GREEN" with zero agent coverage, so fail loudly instead.
    $appRunning = @(Get-Process -Name tl-core, tl-agent, Threadline -ErrorAction SilentlyContinue)
    if ($appRunning.Count -gt 0) {
        $names = ($appRunning | ForEach-Object { "$($_.ProcessName)($($_.Id))" }) -join ', '
        throw "app is running: $names -- close it first, otherwise the agent side cannot be built or tested"
    } else {
        # tauri-build refuses to compile when externalBin is missing, and src-tauri\binaries
        # is gitignored -- stage it first, otherwise a clean clone cannot run the agent tests.
        Step 'stage sidecar' {
            $sidecar = Get-ChildItem (Join-Path $root 'src-tauri\binaries') -Filter 'tl-core-*.exe' -ErrorAction SilentlyContinue
            if (-not $sidecar) {
                & $cargo build --release -p tl-core
                if ($LASTEXITCODE -ne 0) { throw 'release core build failed' }
                & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $root 'scripts\bundle-sidecars.ps1')
            }
        }
        Push-Location (Join-Path $root 'src-tauri')
        try {
            Step 'cargo clippy -D warnings (agent)' { & $cargo clippy --all-targets -- -D warnings }
            Step 'cargo test (agent)' { & $cargo test }
        } finally { Pop-Location }
    }

    Step 'msocheck --selftest' { & node tools/msocheck/msocheck.mjs --selftest }
    Step 'msocheck (architecture)' { & node tools/msocheck/msocheck.mjs }

    Write-Host ''
    Write-Host 'Local gate GREEN.' -ForegroundColor Green
}
finally {
    Pop-Location
}

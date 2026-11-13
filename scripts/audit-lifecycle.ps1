# Lifecycle audit: walks the whole app through every state transition we can drive from
# outside (start / reuse / stop / restart / crash) and checks the invariants each time.
# Meant to be run after changes, so edge cases are found here instead of by the user.
#
# !!! DESTRUCTIVE !!!
# This script works on the REAL data directory ($env:PUBLIC\Threadline), force-kills the
# running tl-core / tl-agent (every live SSH tunnel drops), and deletes the real
# control.json. Nothing is lost permanently (the core regenerates it and tunnels
# reconnect), but do not run it while you rely on an active tunnel.
# ASCII-only (Windows PowerShell 5.1 reads .ps1 as ANSI).
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$debug = Join-Path $root 'target\debug'
$core = Join-Path $debug 'tl-core.exe'
$data = Join-Path $env:PUBLIC 'Threadline'
$controlPath = Join-Path $data 'control.json'

foreach ($p in @($core)) {
    if (-not (Test-Path $p)) { throw "missing $p (build first)" }
}

$failures = 0
function Check([string]$name, [scriptblock]$body) {
    try { & $body; Write-Host "ok   $name" }
    catch { Write-Host "FAIL $name -> $($_.Exception.Message)"; $script:failures++ }
}

function Read-Control {
    $deadline = (Get-Date).AddSeconds(15)
    while (-not (Test-Path $controlPath)) {
        if ((Get-Date) -gt $deadline) { throw 'control.json never appeared' }
        Start-Sleep -Milliseconds 100
    }
    Start-Sleep -Milliseconds 250
    return Get-Content $controlPath -Raw | ConvertFrom-Json
}

function Start-Core {
    # Wake is now part of the agent: starting the core directly is the same outcome
    # as the agent calling ensure_core. The core is long-lived, so we never wait for
    # it to exit -- success is judged by control.json and healthz.
    # ASCII-only file: Windows PowerShell 5.1 reads .ps1 as ANSI.
    Start-Process -FilePath $core -ArgumentList @('--manual', '--data-dir', $data) -WindowStyle Hidden
    Start-Sleep -Seconds 3
}

function Health([int]$port) {
    return Invoke-RestMethod -Uri "http://127.0.0.1:$port/api/v1/healthz" -Method Get
}

# 0. clean slate
Get-Process -Name tl-core, tl-agent -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 800
Remove-Item $controlPath -Force -ErrorAction SilentlyContinue

$first = $null
$second = $null

try {
    # 1. cold start
    Check 'cold start brings the core up' {
        Start-Core
    }

    Check 'cold start publishes a running contract' {
        $script:first = Read-Control
        if ($first.status -ne 'running') { throw "status=$($first.status)" }
        if (-not (Get-Process -Id $first.pid -ErrorAction SilentlyContinue)) { throw 'core pid not alive' }
    }

    Check 'healthz reports the same instance' {
        $h = Health $first.port
        if ($h.instance_id -ne $first.instance_id) { throw 'instance mismatch' }
    }

    # 2. second start must reuse, not duplicate
    Check 'second start reuses the running core' {
        Start-Core
        $again = Read-Control
        if ($again.instance_id -ne $first.instance_id) { throw 'a second core was started' }
    }

    # 3. graceful stop
    Check 'shutdown stops the core and marks the contract stopped' {
        $h = @{ Authorization = "Bearer $($first.token)" }
        Invoke-RestMethod -Uri "http://127.0.0.1:$($first.port)/api/v1/shutdown" -Method Post -Headers $h | Out-Null
        $deadline = (Get-Date).AddSeconds(10)
        while (Get-Process -Id $first.pid -ErrorAction SilentlyContinue) {
            if ((Get-Date) -gt $deadline) { throw 'core did not exit within 10s' }
            Start-Sleep -Milliseconds 100
        }
        Start-Sleep -Milliseconds 300
        $after = Get-Content $controlPath -Raw | ConvertFrom-Json
        if ($after.status -ne 'stopped') { throw "status=$($after.status)" }
    }

    # 4. restart rotates identity
    Check 'restart produces a new instance and a new token' {
        Start-Core
        $script:second = Read-Control
        if ($second.instance_id -eq $first.instance_id) { throw 'instance id did not rotate' }
        if ($second.token -eq $first.token) { throw 'token did not rotate' }
    }

    # 5. crash leaves a stale contract (this is what the tray must not trust)
    Check 'after a crash the contract is stale but healthz is gone' {
        Stop-Process -Id $second.pid -Force
        Start-Sleep -Milliseconds 800
        $stale = Get-Content $controlPath -Raw | ConvertFrom-Json
        if ($stale.status -ne 'running') {
            throw "expected a stale 'running' contract, got $($stale.status)"
        }
        $unreachable = $false
        try { Health $stale.port | Out-Null } catch { $unreachable = $true }
        if (-not $unreachable) { throw 'healthz still answers after the core was killed' }
    }
}
finally {
    if (Test-Path $controlPath) {
        try {
            $ctl = Get-Content $controlPath -Raw | ConvertFrom-Json
            if ($ctl.token) {
                Invoke-RestMethod -Uri "http://127.0.0.1:$($ctl.port)/api/v1/shutdown" -Method Post `
                    -Headers @{ Authorization = "Bearer $($ctl.token)" } | Out-Null
            }
        } catch { }
    }
    Start-Sleep -Milliseconds 600
    Get-Process -Name tl-core -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
}

Write-Host ''
if ($failures -gt 0) {
    Write-Host "LIFECYCLE AUDIT FAILED ($failures)" -ForegroundColor Red
    exit 1
}
Write-Host 'LIFECYCLE AUDIT GREEN' -ForegroundColor Green

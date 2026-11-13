# Threadline end-to-end smoke test: start the real tl-core, exercise the real HTTP API,
# then ask it to shut down and verify it exits. ASCII-only on purpose (Windows PowerShell 5.1
# reads .ps1 as ANSI and would mangle non-ASCII text).
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $root 'target\debug\tl-core.exe'
if (-not (Test-Path $exe)) { throw "binary not found: $exe (run cargo build first)" }

$data = Join-Path $env:TEMP ("threadline-smoke-" + (Get-Random))
New-Item -ItemType Directory -Force -Path $data | Out-Null
Write-Host "data dir: $data"

$failures = 0
function Check([string]$name, [scriptblock]$body) {
    try {
        & $body
        Write-Host "ok   $name"
    } catch {
        Write-Host "FAIL $name -> $($_.Exception.Message)"
        $script:failures++
    }
}

$proc = Start-Process -FilePath $exe -ArgumentList @('--manual', '--data-dir', $data) -PassThru -WindowStyle Hidden
try {
    $controlPath = Join-Path $data 'control.json'
    $deadline = (Get-Date).AddSeconds(15)
    while (-not (Test-Path $controlPath)) {
        if ($proc.HasExited) { throw "core exited early with code $($proc.ExitCode)" }
        if ((Get-Date) -gt $deadline) { throw 'control.json was not written within 15s' }
        Start-Sleep -Milliseconds 100
    }
    Start-Sleep -Milliseconds 200
    $ctl = Get-Content $controlPath -Raw | ConvertFrom-Json
    Write-Host "core listening on port $($ctl.port), instance $($ctl.instance_id), mode $($ctl.run_mode)"
    $base = "http://127.0.0.1:$($ctl.port)/api/v1"
    $headers = @{ Authorization = "Bearer $($ctl.token)" }

    Check 'healthz is reachable without token' {
        $h = Invoke-RestMethod -Uri "$base/healthz" -Method Get
        if ($h.status -ne 'ok') { throw "status=$($h.status)" }
        if ($h.instance_id -ne $ctl.instance_id) { throw 'instance_id mismatch' }
    }

    Check 'protected endpoint rejects missing token' {
        try {
            Invoke-RestMethod -Uri "$base/whoami" -Method Get | Out-Null
            throw 'expected 401'
        } catch {
            $code = $_.Exception.Response.StatusCode.value__
            if ($code -ne 401) { throw "expected 401, got $code" }
        }
    }

    Check 'whoami reports manual mode and identity' {
        $who = Invoke-RestMethod -Uri "$base/whoami" -Method Get -Headers $headers
        if ($who.run_mode -ne 'manual') { throw "run_mode=$($who.run_mode)" }
        if (-not $who.identity.StartsWith('user:')) { throw "identity=$($who.identity)" }
        if ($who.autostart.enabled -ne $false) { throw 'autostart should be off in smoke run' }
    }

    Check 'tunnel list starts empty' {
        $list = Invoke-RestMethod -Uri "$base/tunnels" -Method Get -Headers $headers
        if ($list.items.Count -ne 0) { throw "expected empty, got $($list.items.Count)" }
    }

    Check 'server create validates credential reference (409)' {
        $body = @{
            name = 'smoke'; host = '127.0.0.1'; port = 22; user = 'dev'
            credential_id = 'cred_missing'
        } | ConvertTo-Json
        try {
            Invoke-RestMethod -Uri "$base/servers" -Method Post -Headers $headers -Body $body -ContentType 'application/json' | Out-Null
            throw 'expected 409'
        } catch {
            $code = $_.Exception.Response.StatusCode.value__
            if ($code -ne 409) { throw "expected 409, got $code" }
        }
    }

    Check 'logs endpoint answers' {
        $logs = Invoke-RestMethod -Uri "$base/logs?tail=5" -Method Get -Headers $headers
        if ($null -eq $logs.lines) { throw 'missing lines field' }
    }

    Check 'shutdown returns 202 and process exits' {
        $resp = Invoke-RestMethod -Uri "$base/shutdown" -Method Post -Headers $headers
        if ($resp.accepted -ne $true) { throw 'accepted not true' }
        if (-not $proc.WaitForExit(10000)) { throw 'process did not exit within 10s' }
        if ($proc.ExitCode -ne 0) { throw "exit code $($proc.ExitCode)" }
    }

    Check 'control.json is marked stopped after shutdown' {
        $after = Get-Content $controlPath -Raw | ConvertFrom-Json
        if ($after.status -ne 'stopped') { throw "status=$($after.status)" }
    }
}
finally {
    if (-not $proc.HasExited) { $proc.Kill(); $proc.WaitForExit(5000) }
    Remove-Item -Recurse -Force $data -ErrorAction SilentlyContinue
}

Write-Host ''
if ($failures -gt 0) {
    Write-Host "SMOKE FAILED ($failures failed)" -ForegroundColor Red
    exit 1
}
Write-Host "SMOKE GREEN" -ForegroundColor Green

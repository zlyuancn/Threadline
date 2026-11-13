# Agent smoke test: start tl-core + tl-agent against a temp data dir, then verify
# that a second agent instance hands off and exits (single-instance contract).
# ASCII-only (Windows PowerShell 5.1 reads .ps1 as ANSI).
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$core = Join-Path $root 'target\debug\tl-core.exe'
$agent = Join-Path $root 'src-tauri\target\debug\tl-agent.exe'
if (-not (Test-Path $core)) { throw "missing $core" }
if (-not (Test-Path $agent)) { throw "missing $agent" }

$data = Join-Path $env:TEMP ("threadline-agent-smoke-" + (Get-Random))
New-Item -ItemType Directory -Force -Path $data | Out-Null
Write-Host "data dir: $data"

$failures = 0
function Check([string]$name, [scriptblock]$body) {
    try { & $body; Write-Host "ok   $name" }
    catch { Write-Host "FAIL $name -> $($_.Exception.Message)"; $script:failures++ }
}

$coreProc = Start-Process -FilePath $core -ArgumentList @('--manual', '--data-dir', $data) -PassThru -WindowStyle Hidden
$agentProc = $null
try {
    $deadline = (Get-Date).AddSeconds(15)
    while (-not (Test-Path (Join-Path $data 'control.json'))) {
        if ($coreProc.HasExited) { throw "core exited with $($coreProc.ExitCode)" }
        if ((Get-Date) -gt $deadline) { throw 'core did not publish control.json' }
        Start-Sleep -Milliseconds 100
    }

    $agentProc = Start-Process -FilePath $agent -ArgumentList @('--data-dir', $data) -PassThru
    Start-Sleep -Seconds 6

    Check 'agent stays alive (window + tray)' {
        if ($agentProc.HasExited) { throw "agent exited early with $($agentProc.ExitCode)" }
    }

    Check 'second instance hands off and exits 0' {
        $second = Start-Process -FilePath $agent -ArgumentList @('--data-dir', $data) -PassThru -Wait
        if ($second.ExitCode -ne 0) { throw "second instance exit code $($second.ExitCode)" }
        if ($agentProc.HasExited) { throw 'first agent died after handoff' }
    }

    Check 'core still serving after agent traffic' {
        $ctl = Get-Content (Join-Path $data 'control.json') -Raw | ConvertFrom-Json
        $h = Invoke-RestMethod -Uri "http://127.0.0.1:$($ctl.port)/api/v1/healthz" -Method Get
        if ($h.instance_id -ne $ctl.instance_id) { throw 'instance_id mismatch' }
    }
}
finally {
    if ($agentProc -and -not $agentProc.HasExited) { $agentProc.Kill(); $agentProc.WaitForExit(5000) }
    if (-not $coreProc.HasExited) {
        $ctl = $null
        try { $ctl = Get-Content (Join-Path $data 'control.json') -Raw | ConvertFrom-Json } catch { }
        if ($ctl) {
            try {
                Invoke-RestMethod -Uri "http://127.0.0.1:$($ctl.port)/api/v1/shutdown" -Method Post -Headers @{ Authorization = "Bearer $($ctl.token)" } | Out-Null
            } catch { }
        }
        if (-not $coreProc.WaitForExit(5000)) { $coreProc.Kill() }
    }
    Remove-Item -Recurse -Force $data -ErrorAction SilentlyContinue
}

Write-Host ''
if ($failures -gt 0) {
    Write-Host "AGENT SMOKE FAILED ($failures)" -ForegroundColor Red
    exit 1
}
Write-Host 'AGENT SMOKE GREEN' -ForegroundColor Green

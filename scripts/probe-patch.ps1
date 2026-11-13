# Probe: verify PATCH /servers/{id} and PATCH /tunnels/{id} really work end to end.
# The edit UI depends on these two endpoints; they are exercised here against a live core.
# ASCII-only (Windows PowerShell 5.1 reads .ps1 as ANSI).
$ErrorActionPreference = 'Stop'

$ctl = Get-Content (Join-Path $env:PUBLIC 'Threadline\control.json') -Raw | ConvertFrom-Json
$base = "http://127.0.0.1:$($ctl.port)/api/v1"
$headers = @{ Authorization = "Bearer $($ctl.token)" }
$failures = 0

function Check([string]$name, [scriptblock]$body) {
    try { & $body; Write-Host "ok   $name" }
    catch { Write-Host "FAIL $name -> $($_.Exception.Message)"; $script:failures++ }
}

$credId = $null
$serverId = $null
$tunnelId = $null

# Make the probe re-runnable: clear anything a previous failed run left behind,
# otherwise the fixed local port stays occupied and create fails with 409.
try {
    foreach ($t in (Invoke-RestMethod -Uri "$base/tunnels" -Headers $headers).items) {
        if ($t.definition.name -like 'probe-*') {
            Invoke-RestMethod -Uri "$base/tunnels/$($t.definition.id)" -Method Delete -Headers $headers | Out-Null
        }
    }
    foreach ($s in (Invoke-RestMethod -Uri "$base/servers" -Headers $headers).items) {
        if ($s.name -like 'probe-*') {
            Invoke-RestMethod -Uri "$base/servers/$($s.id)" -Method Delete -Headers $headers | Out-Null
        }
    }
    foreach ($c in (Invoke-RestMethod -Uri "$base/credentials" -Headers $headers).items) {
        if ($c.name -like 'probe*') {
            Invoke-RestMethod -Uri "$base/credentials/$($c.id)" -Method Delete -Headers $headers | Out-Null
        }
    }
}
catch {
    Write-Host "cleanup warning: $($_.Exception.Message)"
}

try {
    Check 'create credential' {
        $body = @{ name = 'probe'; kind = 'password'; username = 'root'; secret = 'pw' } | ConvertTo-Json
        $r = Invoke-RestMethod -Uri "$base/credentials" -Method Post -Headers $headers -ContentType 'application/json' -Body $body
        $script:credId = $r.id
    }

    Check 'create server (tunnel entity)' {
        $body = @{ name = 'probe-srv'; host = '127.0.0.1'; port = 22; credential_id = $credId } | ConvertTo-Json
        $r = Invoke-RestMethod -Uri "$base/servers" -Method Post -Headers $headers -ContentType 'application/json' -Body $body
        $script:serverId = $r.id
    }

    Check 'PATCH server changes name and keepalive' {
        $body = @{ name = 'probe-srv-edited'; keepalive_secs = 45 } | ConvertTo-Json
        $r = Invoke-RestMethod -Uri "$base/servers/$serverId" -Method Patch -Headers $headers -ContentType 'application/json' -Body $body
        if ($r.name -ne 'probe-srv-edited') { throw "name=$($r.name)" }
        if ($r.keepalive_secs -ne 45) { throw "keepalive=$($r.keepalive_secs)" }
    }

    Check 'PATCH server rejects empty host' {
        $body = @{ host = '' } | ConvertTo-Json
        $code = 0
        try {
            Invoke-RestMethod -Uri "$base/servers/$serverId" -Method Patch -Headers $headers -ContentType 'application/json' -Body $body | Out-Null
        }
        catch { $code = [int]$_.Exception.Response.StatusCode }
        if ($code -ne 400) { throw "expected 400, got $code" }
    }

    Check 'PATCH credential renames without touching the secret' {
        $body = @{ name = 'probe-renamed' } | ConvertTo-Json
        $r = Invoke-RestMethod -Uri "$base/credentials/$credId" -Method Patch -Headers $headers -ContentType 'application/json' -Body $body
        if ($r.name -ne 'probe-renamed') { throw "name=$($r.name)" }
    }

    Check 'PATCH credential rejects empty name' {
        $body = @{ name = '  ' } | ConvertTo-Json
        $code = 0
        try {
            Invoke-RestMethod -Uri "$base/credentials/$credId" -Method Patch -Headers $headers -ContentType 'application/json' -Body $body | Out-Null
        }
        catch { $code = [int]$_.Exception.Response.StatusCode }
        if ($code -ne 400) { throw "expected 400, got $code" }
    }

    Check 'probe reports the tcp stage for a closed port' {
        $body = @{
            name = 'probe-dead'; host = '127.0.0.1'; port = 1; credential_id = $credId
        } | ConvertTo-Json
        $srv = Invoke-RestMethod -Uri "$base/servers" -Method Post -Headers $headers -ContentType 'application/json' -Body $body
        $r = Invoke-RestMethod -Uri "$base/servers/$($srv.id)/probe" -Method Post -Headers $headers
        if ($r.stage -ne 'tcp') { throw "stage=$($r.stage) message=$($r.message)" }
        Write-Host ("     " + $r.message)
    }

    Check 'create port forward' {
        $body = @{
            name = 'probe-fwd'; server_id = $serverId
            local_port = 18099; remote_port = 8080
        } | ConvertTo-Json
        $r = Invoke-RestMethod -Uri "$base/tunnels" -Method Post -Headers $headers -ContentType 'application/json' -Body $body
        $script:tunnelId = $r.definition.id
        Write-Host ("     created tunnel id = " + $tunnelId)
        if (-not $tunnelId) { throw "create returned no definition.id: $($r | ConvertTo-Json -Compress)" }
    }

    Check 'PATCH port forward changes local port' {
        $body = @{ local_port = 18098 } | ConvertTo-Json
        $r = Invoke-RestMethod -Uri "$base/tunnels/$tunnelId" -Method Patch -Headers $headers -ContentType 'application/json' -Body $body
        if ($r.definition.local_port -ne 18098) { throw "local_port=$($r.definition.local_port)" }
        if ($r.status.state -ne 'stopped') { throw "state=$($r.status.state)" }
    }
}
finally {
    foreach ($pair in @(
            @{ uri = "$base/tunnels/$tunnelId"; label = 'port forward' },
            @{ uri = "$base/servers/$serverId"; label = 'server' },
            @{ uri = "$base/credentials/$credId"; label = 'credential' })) {
        if ($pair.uri -notmatch '/$') {
            try { Invoke-RestMethod -Uri $pair.uri -Method Delete -Headers $headers | Out-Null } catch { }
        }
    }
}

Write-Host ''
if ($failures -gt 0) {
    Write-Host "PATCH PROBE FAILED ($failures)" -ForegroundColor Red
    exit 1
}
Write-Host 'PATCH PROBE GREEN' -ForegroundColor Green

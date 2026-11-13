# Diagnostic: send the SAME raw HTTP bytes the tray uses for "stop service" and print the
# status line. Used to tell "core rejected the request" apart from "core never got it".
# ASCII-only (Windows PowerShell 5.1 reads .ps1 as ANSI).
$ErrorActionPreference = 'Stop'

$controlPath = Join-Path $env:PUBLIC 'Threadline\control.json'
$ctl = Get-Content $controlPath -Raw | ConvertFrom-Json
Write-Host "core pid=$($ctl.pid) port=$($ctl.port) status=$($ctl.status)"

$client = New-Object System.Net.Sockets.TcpClient
$client.ReceiveTimeout = 1500
$client.Connect('127.0.0.1', $ctl.port)
$stream = $client.GetStream()

$request = "POST /api/v1/shutdown HTTP/1.1`r`nHost: 127.0.0.1:$($ctl.port)`r`nAuthorization: Bearer $($ctl.token)`r`nContent-Length: 0`r`nConnection: close`r`n`r`n"
$bytes = [Text.Encoding]::ASCII.GetBytes($request)
$stream.Write($bytes, 0, $bytes.Length)
$stream.Flush()

Start-Sleep -Milliseconds 400
$buffer = New-Object byte[] 512
try {
    $read = $stream.Read($buffer, 0, 512)
    if ($read -gt 0) {
        $first = [Text.Encoding]::ASCII.GetString($buffer, 0, $read).Split("`r`n")[0]
        Write-Host "response: $first"
    }
    else {
        Write-Host 'response: (connection closed without data)'
    }
}
catch {
    Write-Host "response: (no data) $($_.Exception.Message)"
}
$client.Close()

Start-Sleep -Seconds 2
$alive = [bool](Get-Process -Id $ctl.pid -ErrorAction SilentlyContinue)
Write-Host "core alive after request: $alive"

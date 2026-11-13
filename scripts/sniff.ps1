# Read the first bytes a host:port sends, to tell an SSH banner from an HTTP error page.
# Usage: powershell -File scripts/sniff.ps1 <host> <port>
param(
    [Parameter(Mandatory = $true)][string]$TargetHost,
    [Parameter(Mandatory = $true)][int]$TargetPort
)

$client = New-Object System.Net.Sockets.TcpClient
$client.ReceiveTimeout = 2000
try {
    $client.Connect($TargetHost, $TargetPort)
}
catch {
    Write-Host "connect failed: $($_.Exception.Message)"
    exit 1
}

$stream = $client.GetStream()
$buffer = New-Object byte[] 256
try {
    $read = $stream.Read($buffer, 0, 256)
    if ($read -gt 0) {
        $text = [Text.Encoding]::ASCII.GetString($buffer, 0, $read)
        Write-Host "received $read bytes:"
        Write-Host $text
    }
    else {
        Write-Host 'connection closed without data (silent peer)'
    }
}
catch {
    Write-Host "read failed: $($_.Exception.Message)"
}
$client.Close()

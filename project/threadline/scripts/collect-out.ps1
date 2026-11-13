# Collect the shippable pair into <project>/out (next to scripts/, target/, src-tauri/).
#
# Only two binaries matter at runtime: the agent (double-click) and the core it
# spawns from its own directory. Everything else under target/ is an intermediate.
# ASCII-only: Windows PowerShell 5.1 reads .ps1 as ANSI.
$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$debug = Join-Path $root 'src-tauri\target\debug'
$out = Join-Path $root 'out'

# The agent's main binary is renamed by Tauri (mainBinaryName = Threadline); dev builds
# before that change still produce tl-agent.exe. Collect whichever exists.
$mainName = if (Test-Path (Join-Path $debug 'Threadline.exe')) { 'Threadline.exe' } else { 'tl-agent.exe' }
$pair = @($mainName, 'tl-core.exe')
foreach ($name in $pair) {
    $src = Join-Path $debug $name
    if (-not (Test-Path $src)) { throw "missing $src -- run 'pnpm tauri build --debug' first" }
}

New-Item -ItemType Directory -Force -Path $out | Out-Null
foreach ($name in $pair) {
    Copy-Item (Join-Path $debug $name) (Join-Path $out $name) -Force
    $stamp = (Get-Item (Join-Path $debug $name)).LastWriteTime.ToString('yyyy-MM-dd HH:mm')
    Write-Host ("collected {0}  (built {1})" -f $name, $stamp)
}

# Sanity: the pair must be byte-identical to what we just built.
foreach ($name in $pair) {
    $a = (Get-FileHash (Join-Path $debug $name) -Algorithm MD5).Hash
    $b = (Get-FileHash (Join-Path $out $name) -Algorithm MD5).Hash
    if ($a -ne $b) { throw "copy mismatch: $name" }
}

Write-Host ("out ready: " + $out)

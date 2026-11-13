# Build the GitHub Release body from CHANGELOG.md.
#
# Why: the release page used to show only a fixed install notice plus a
# "Full Changelog" link, so nobody could see what actually changed in a version.
# The changelog section for the tag is the single source of truth.
#
# Usage: ./scripts/release-notes.ps1 -Version 0.2.1 [-Output release-body.md]
#
# NOTE: keep this file ASCII-only -- Windows PowerShell 5.1 reads .ps1 as ANSI and
# would mangle non-ASCII text, breaking quoting. Chinese text lives in
# scripts/release-footer.md (read as UTF-8) and in CHANGELOG.md.
param(
    [Parameter(Mandatory = $true)][string]$Version,
    [string]$Output = 'release-body.md'
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$changelog = Join-Path $root 'CHANGELOG.md'
if (-not (Test-Path $changelog)) { throw "missing $changelog" }

$version = $Version.TrimStart('v', 'V')
$lines = Get-Content $changelog -Encoding UTF8

$start = -1
$end = $lines.Count
for ($i = 0; $i -lt $lines.Count; $i++) {
    if ($start -lt 0) {
        if ($lines[$i] -match "^## \[$([regex]::Escape($version))\]") { $start = $i }
        continue
    }
    # next level-2 heading ends this section
    if ($lines[$i] -match '^## ') { $end = $i; break }
}

if ($start -lt 0) { throw "CHANGELOG.md has no section for $version" }

# drop the heading itself, keep everything else
$section = ($lines[($start + 1)..($end - 1)] -join "`n").Trim()
if (-not $section) { throw "the $version section in CHANGELOG.md is empty" }
$notes = $section -replace "`r`n", "`n"

$footerPath = Join-Path $PSScriptRoot 'release-footer.md'
$footer = ''
if (Test-Path $footerPath) {
    $footer = (Get-Content $footerPath -Raw -Encoding UTF8) -replace "`r`n", "`n"
}

# The changelog section already carries its own ### headings; no extra title.
$body = "$notes`n`n$($footer.Trim())`n"

$target = if ([System.IO.Path]::IsPathRooted($Output)) { $Output } else { Join-Path $root $Output }
# No BOM: GitHub would render it as part of the first line.
[System.IO.File]::WriteAllText($target, $body, (New-Object System.Text.UTF8Encoding($false)))

$count = ($notes -split "`n").Count
Write-Host "wrote $target for $version ($count lines of notes)"

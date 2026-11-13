<#
.SYNOPSIS
    发布：把当前提交推到 GitHub 的 main 并打 tag。

.DESCRIPTION
    推完整提交历史：每个改动一个 commit，远端能看到演进过程。

    历史基线是 v0.1.0（`6868a52`）：更早的探索性提交没进远端，v0.1.0 及其
    之后的提交都在。不要再用 `git commit-tree` 构造孤儿提交——那会把远端压成
    一个 commit，看不出任何变化。

.PARAMETER DryRun
    只打印将要执行的命令，不推送、不打 tag。

.PARAMETER NoPush
    打 tag 但不推送（用于本地检查）。

.EXAMPLE
    ./scripts/release.ps1 -DryRun
    ./scripts/release.ps1
#>
param(
    [switch]$DryRun,
    [switch]$NoPush
)

$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')

$version = (Get-Content package.json -Raw -Encoding UTF8 | ConvertFrom-Json).version
if (-not $version) { throw '读不到 package.json 的 version' }
$tag = "v$version"

# 未提交的改动必须先进仓库，否则快照会漏掉它们（且事后很难发现）
$dirty = git status --porcelain
if ($dirty) {
    Write-Host '工作树不干净，先提交：' -ForegroundColor Red
    $dirty | ForEach-Object { Write-Host "  $_" }
    exit 1
}

# tag 指向已发布的提交时，说明版本号没改就重发了一次
$existing = git tag --list $tag
if ($existing -and -not $DryRun) {
    Write-Host "tag $tag 已存在，将被覆盖（内容相同则无需重发）" -ForegroundColor Yellow
}

$head = (git rev-parse --short HEAD).Trim()
$count = (git rev-list --count HEAD).Trim()

Write-Host "版本   : $version"
Write-Host "HEAD   : $head（累计 $count 个提交）"
Write-Host "远端   : 完整历史，不做压平"

if ($DryRun) {
    Write-Host ''
    Write-Host '[DryRun] 将执行：' -ForegroundColor Cyan
    Write-Host "  git tag -f -a $tag -m `"Threadline $version`""
    Write-Host "  git push github HEAD:main"
    Write-Host "  git push --force github $tag"
    exit 0
}

$short = $head
Write-Host "提交   : $short" -ForegroundColor Green

git tag -f -a $tag -m "Threadline $version" | Out-Null
if ($LASTEXITCODE -ne 0) { throw "打 tag 失败：$tag" }

if ($NoPush) {
    Write-Host "已打 $tag -> $short（未推送）" -ForegroundColor Yellow
    exit 0
}

git push github "HEAD:main"
if ($LASTEXITCODE -ne 0) { throw '推送 main 失败（远端可能有分叉，先 git fetch 与 git log 检查）' }

git push --force github $tag
if ($LASTEXITCODE -ne 0) { throw "推送 tag 失败：$tag" }

Write-Host ''
Write-Host "已发布 $tag（main -> $short），Release 流水线会由 tag 触发。" -ForegroundColor Green
Write-Host "发布页：https://github.com/zlyuancn/Threadline/releases/tag/$tag"

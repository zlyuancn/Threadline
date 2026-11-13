<#
.SYNOPSIS
    发布：把当前代码打成单提交快照，推到 GitHub 的 main 并打 tag。

.DESCRIPTION
    远端仓库只保留一个提交（`feat: Threadline vX.Y.Z`），不带开发历史——
    这是维护者定的形态。本地仓库的历史完整保留，只在发布这一刻构造快照。

    结构扁平之后不再需要 `git subtree split`：仓库根就是 GitHub 仓库根，
    所以快照可以直接用 `HEAD^{tree}` 构造。

.PARAMETER DryRun
    只打印将要执行的命令，不推送、不打 tag。

.PARAMETER NoPush
    构造快照与 tag，但不推送（用于本地检查）。

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
$message = "feat: Threadline $version"

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

$tree = (git rev-parse 'HEAD^{tree}').Trim()
$head = (git rev-parse --short HEAD).Trim()

Write-Host "版本   : $version"
Write-Host "HEAD   : $head"
Write-Host "快照树 : $tree"

if ($DryRun) {
    Write-Host ''
    Write-Host '[DryRun] 将执行：' -ForegroundColor Cyan
    Write-Host "  git commit-tree $tree -m `"$message`""
    Write-Host "  git tag -f -a $tag <snapshot> -m `"Threadline $version`""
    Write-Host "  git push --force github <snapshot>:main"
    Write-Host "  git push --force github $tag"
    exit 0
}

# 无父提交 = 单提交快照，不带任何历史
$snapshot = (git commit-tree $tree -m $message).Trim()
if (-not $snapshot) { throw 'commit-tree 失败' }
$short = $snapshot.Substring(0, 7)
Write-Host "快照   : $short" -ForegroundColor Green

git tag -f -a $tag $snapshot -m "Threadline $version" | Out-Null
if ($LASTEXITCODE -ne 0) { throw "打 tag 失败：$tag" }

if ($NoPush) {
    Write-Host "已构造 $tag -> $short（未推送）" -ForegroundColor Yellow
    exit 0
}

git push --force github "${snapshot}:main"
if ($LASTEXITCODE -ne 0) { throw '推送 main 失败' }

git push --force github $tag
if ($LASTEXITCODE -ne 0) { throw "推送 tag 失败：$tag" }

Write-Host ''
Write-Host "已发布 $tag（main -> $short），Release 流水线会由 tag 触发。" -ForegroundColor Green
Write-Host "发布页：https://github.com/zlyuancn/Threadline/releases/tag/$tag"

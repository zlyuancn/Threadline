#!/usr/bin/env bash
# Threadline 本机门禁包装：在 WSL / Git Bash 里一条命令跑完（实际逻辑在 verify.ps1，因为 cargo/pnpm 是 Windows 侧工具）。
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if command -v wslpath >/dev/null 2>&1; then
  PS_PATH="$(wslpath -w "$HERE/verify.ps1")"
else
  PS_PATH="$HERE/verify.ps1"
fi

powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$PS_PATH"

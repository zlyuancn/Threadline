#!/usr/bin/env bash
# 工作区本机门禁：依次跑各项目的门禁（当前只有 threadline），全绿才算通过。
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "== project/threadline =="
bash "$HERE/../project/threadline/scripts/verify.sh"

echo
echo "本机门禁全绿。批次收口仍以环境门禁（真实 SSH 服务器端到端转发）为准。"

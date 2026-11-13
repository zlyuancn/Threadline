# scripts 索引

按用途分组。带 ⚠️ 的需要**真实环境**（真实 SSH 服务器、正在运行的核心、有桌面的会话），跑之前先读脚本头部的注释。

## 门禁与构建

| 脚本 | 用途 |
|---|---|
| `verify.ps1` | 本机门禁：`cargo fmt --check` → `clippy -D warnings` → `cargo test` → 陈旧副本检查 → Agent 测试 → `msocheck` → 前端测试与构建。提交前必须全绿 |
| `verify.sh` | `verify.ps1` 的薄封装（Git Bash 用） |
| `prebuild.ps1` | `tauri build` 的 `beforeBuildCommand`：先 `cargo build --release -p tl-core` 重建核心，再 stage sidecar，最后 `pnpm build` |
| `bundle-sidecars.ps1` | 把 `target/release/tl-core.exe` 拷成 `src-tauri/binaries/tl-core-<triple>.exe`。**只接受 release**，缺失即报错 |
| `make-icon.ps1` | 用 GDI+ 现画 `app-icon.png`，再由 Tauri CLI 派生全套图标 |

## 冒烟

| 脚本 | 用途 |
|---|---|
| `smoke.ps1` | ⚠️ 控制面冒烟：起核心、跑 API 基本流程、退出 |
| `smoke-agent.ps1` | ⚠️ Agent 冒烟：托盘、单实例、窗口 |

## 端到端（⚠️ 需要真实 SSH 服务器与真实核心）

`e2e-*.py` 都是 `python scripts/e2e-xxx.py` 直跑的脚本，前置条件写在各自文件头。

| 脚本 | 覆盖 |
|---|---|
| `e2e-gate.py` | UAC 闸门与提权拒绝路径 |
| `e2e-http.py` | 明文 HTTP 转发 |
| `e2e-inline.py` | 本地 CA 终止（https → http） |
| `e2e-keeper.py` | keepalive 与会话存活 |
| `e2e-persist.py` | 配置落盘与重启恢复 |
| `e2e-refused.py` | 远端拒绝连接时的状态 |
| `e2e-session-error.py` | 会话错误的上报 |
| `e2e-shared-session.py` | 同隧道多转发共用会话 |
| `e2e-trust-flow.py` | 首次主机密钥确认 → 信任 → 重连 |
| `e2e-tunnel-switch.py` | 隧道切换时的转发状态 |

## 探针与诊断（⚠️ 多为一次性排查工具）

| 脚本 | 用途 |
|---|---|
| `check-*.py` | 单项行为核查（级联删除、删除守卫、端口冲突、重排序、协议格、SSE、开关隔离、速率、隧道 ID、延迟） |
| `probe-all.py` | 汇总探针，逐个跑 `check-*` 并打印结果 |
| `probe-patch.ps1` / `probe-shutdown.ps1` | 手工触发 patch / shutdown 的提权或时序探针 |
| `sniff.ps1` | 抓本机回环连接的端口与字节数 |
| `shot.ps1` | `PrintWindow` 截 Agent 窗口（锁屏也能截），输出 `%TEMP%\tl-ui.png` |
| `show-servers.py` / `state.py` | 打印 `%PUBLIC%\Threadline` 下的配置摘要（只读） |
| `audit-lifecycle.ps1` | ⚠️ **会强杀正在运行的核心与 Agent、并删除真实 `control.json`**，用来审计进程/端口/计划任务的生命周期残留 |
| `collect-out.ps1` | 把 debug 产物拷到 `out/` 便于分发（**仅开发用**，不要发布 `out/`） |
| `cleanup-probe-data.py` | 清理探针写进数据目录的测试条目 |

## 架构检查

`tools/msocheck/msocheck.mjs`：分层与依赖方向检查，`--selftest` 校验检查器自身。由 `verify.ps1` 调用。

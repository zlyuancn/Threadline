# Threadline（引线）· 项目 AI 入口

> 会话开始先读本文件；进度与待办见 `docs/TODO.md`。

```
架构：模型自组织架构（MSO）
本地副本：docs/mso/AGENTS.md —— 任何涉及架构的设计、写码、评审前先读它；不要重新联网下载
来源：https://github.com/zly-app/mso-arch（规则版本：5c432f9，2026-10-19）
```

```
服务形态：Core–Agent–Wake 三进程模式（CAW）
落地方案：tl-core 常驻持有业务状态，tl-agent 只做外壳，tl-wake 是编进 Agent 的编排库
（README 的「架构」一节有完整说明）
```

## 项目是什么

Windows 桌面 SSH **本地端口转发**工具：把远端服务器的端口映射到本机端口（`-L`），支持多条隧道并行、开机自启、快捷唤起。产品名 Threadline / 引线。

## 两进程与产物

| 进程 | 产物 | 身份 | 职责 | 硬约束 |
|---|---|---|---|---|
| Core | `tl-core.exe` | SYSTEM（boot 档）/ 用户（manual、顶班、回退） | SSH 连接与转发数据面 + 回环控制 API | 零 GUI 依赖；登录前可服务；单实例互斥 |
| Agent | `tl-agent.exe` | 登录用户 | 托盘 + 控制面板（Tauri 2 + Vue 3）+ **启动编排**（确保核心在跑） | 不持锁、不占端口、不持久化服务状态；一切动作走 HTTP API |

原先的 `tl.exe`（Wake）已并入 Agent：`tl_wake::ensure_core()` 仍是一个独立库，
由助手在启动时和「启动服务」按钮里直接调用——用户双击的本来就是助手，
没必要再经一个只活几百毫秒的中转进程。开机自启路径不变（任务计划直接拉起核心）。

Agent **只**通过 `tl-protocol` 描述的 `/api/v1` 回环 HTTP 与 Core 交互（CAW C1），不新增任何 IPC。

## 目录与层

```
crates/tl-protocol/   控制 API 与磁盘契约（DTO，两侧共用；无业务规则）
crates/tl-core/       核心
  src/main.rs         应用壳（唯一壳文件）：命令行、tracing、组合根装配
  src/interface/      ① 接口层：HTTP 适配、鉴权、参数校验（零业务规则）
  src/business/       ② 业务层：用例编排（不藏判定规则）
  src/models/         ③ 模型层：credential / server / tunnel / runtime（数据与规则所有者）
  src/foundation/     底座：原子存储、DPAPI、日志、token、退避/超时/并发预算、注册层 Win32 封装
crates/tl-wake/       启动编排（库，无独立二进制）
src-tauri/            Agent（B4 落地）
src/                  Vue 3 前端（views / api / models 三层）
tools/msocheck/       架构机检 R1-R6（分层与依赖方向 / 数据独占 / 前端收口 / 核心无头 / 壳不施放 / 环与终止保障）
docs/mso/             MSO 规则本地副本（只读基准）
docs/plans/           设计文档与实施计划
```

依赖方向恒为：壳 → 接口 → 业务 → 模型 → 底座。反向 import 由 `tools/msocheck` 在门禁里拦截。

## 铁律

1. **模型归属先行**：新增逻辑先回答「属于哪个模型」，一句话判据（拥有什么数据、提供什么操作）说不清就停下重新设计。
2. **数据独占**：模型只读写自己的状态；跨模型调用只走对方 port，禁止直翻内部字段。
3. **业务层不藏规则**：「能不能启动这条隧道」「重连几次放弃」这类判定必须在模型里。
4. **终止保障用底座**：超时/退避预算/重入防护一律用 `foundation` 的组件，禁止模型自建私有重试轮子。
5. **CAW 不变量不可破**：核心零 UI、助手无状态、唤醒必退、注册⊥运行、降级必标记。
6. **凭据只在 Core 内解密**：Agent/Wake 不读 `credentials.dat`，也不接收明文凭据回显。
7. **门禁一条命令**：`bash scripts/verify.sh`（构建 + 测试 + 格式 + lint + 机检）全绿才提交。
8. **不发散**：v1 只做 `-L` 本地转发；`-R`/`-D`/多用户/跨平台均不在范围内。

## 当前状态

B0–B6 已完成并可运行：

- `tl-core`：底座（原子存储 / DPAPI / 退避 / 并发预算 / 文件锁 / 任务计划）+ 四个模型（credential / server / tunnel / runtime）+ SSH 引擎（russh，`-L` 转发）+ 19 个控制端点的 axum 服务 + 壳层 CLI 与退出码。
- `tl-agent`：Tauri 2 + Vue 3 面板（隧道 / 服务器与凭据 / 日志 / 设置）+ 托盘 + 单实例 + 发现契约注入。
- 启动编排：`tl_wake::ensure_core()`（探测 → 按注册层分流拉起 → 等就绪），由助手调用。
- 本机门禁全绿（cargo test 136 项 + msocheck 44 项 + clippy + fmt + 前端构建）；三条冒烟脚本全绿；NSIS 安装包已产出。

剩余 B7 全部是真机验证（真实 SSH 服务器转发、UAC 自启、零登录会话、安装器实装）。

# threadline 工作区 · AI 协作入口

> 任何 AI 助手在本工作区工作时，**先读本文件，再按指引进入具体项目**。

## 工作区性质

- 本目录是 **Threadline（引线）** 的多项目工作区：`project/` 目录收纳全部项目仓库，每个都是**嵌套的独立 git 仓库**（外层 `.gitignore` 已排除 `project/`）。
- 外层仓库管理工作区文档与**全部 todo/plan 文档**（`docs/`）；项目代码的 commit/push 必须**进入对应子目录**操作，各仓库互不牵连。
- 当前只有一个项目：`project/threadline/`。工作区层面「提交/推送」= 依次检查外层 + 子仓库，有变更的分别 commit/push。

## 方法论来源（只读引用，勿改勿拷）

| 方法论 | 本地绝对路径 | 版本戳（安装时） |
|---|---|---|
| 多项目 AI 协作工作区 | `C:\my\methodology\multi-project-workspace\docs\` | `f78fd6e` |
| Core–Agent–Wake 三进程服务模式（CAW） | `C:\my\methodology\core-agent-wake\` | `9dcdcec` |
| 模型自组织架构（MSO） | `C:\my\methodology\mso-arch\` | `5c432f9` |

- 安装日期：2026-10-19。三处均为**只读引用**；升级 = 在上述路径 `git pull` 后对比版本戳，再决定是否对齐。
- MSO 采用流程要求在项目内留规则副本：`project/threadline/docs/mso/AGENTS.md`——**会话读副本，不联网下载**。
- CAW 与工作区方法论的精确规则不拷进本仓库：直接读上表路径下的 `AGENTS.zh-CN.md` / `docs/`。
- **2026-10-21 升级到 `9dcdcec`**（内容同 `10a41fd`，仅补删误提交的临时文件）：新增 R12（数据目录跨身份可写）、R13（凭据范围与运行身份一致）
  和 `PRACTICES.zh-CN.md`（进程形态 / 数据目录 / 凭据的实测结论）。本项目的落地形态据此定稿：
  两进程（Wake 并入助手）+ `%PUBLIC%\Threadline`（INTERACTIVE:Modify）+ DPAPI machine scope，
  并把"同机其他用户可读凭据"作为**单用户机器**前提记录在 TODO。

## 项目地图

| 仓库 | 路径 | 一句话定位 | AI 必读文档 |
|---|---|---|---|
| threadline | `project/threadline/` | Windows 桌面 SSH 本地端口转发工具（CAW 三进程 + Tauri 2 + Vue 3 + Rust） | `project/threadline/AGENTS.md`、`project/threadline/docs/mso/AGENTS.md`、本仓库 `docs/TODO.md` |

> 同一项目在不同上下文里名字可能不同（产品名 Threadline / 二进制 `tl`、`tl-core`、`tl-agent` / 目录 threadline）。**操作前以本表路径为准。**

### 架构速览

```
tl-agent.exe (Tauri+Vue, 托盘/面板)
   └─ 启动编排 tl_wake::ensure_core（探测 → 拉起）──┐
                                                     ├──HTTP(127.0.0.1 + token)──► tl-core.exe (无头)
                                                     │                                   │
                                                     │                          SSH(-L 本地转发)
                                                     │                                   ▼
                                                     └───────────────────────────  远端服务器端口
```

- Core 是唯一持有 SSH 连接与业务状态的进程；Agent 是 Control API 的普通客户端，不持锁、不占端口、不持久化服务状态。启动编排（原 Wake）已并入 Agent。
- 详细设计见 `docs/plans/2026-10-19-threadline-design.md`。

## 文档布局

- `docs/TODO.md`：**进行中/待办**（唯一待办清单）+ **续接点块（新会话必读）**。
- `docs/DONE.md`：已完成条目台账（收口时从 TODO 搬入，只增不改）。
- `docs/plans/`：设计文档与实施计划（`YYYY-MM-DD-<主题>.md`）。
- `docs/decisions/`：跨项目/架构级决策记录（`NNNN-<标题>.md`，只增不改）。
- `docs/TESTPLAN.md`：功能测试项清单（新增/修改功能须同步维护）。
- `docs/archive/`：完成记录、回归战报与踩坑清单。
- `scripts/`：`verify.sh`（本机门禁一条命令）。
- 项目内引用工作区文档用 `../../docs/...`。

## 场景路由

按当前任务查表路由，**按需读取**，不要在会话开始全量加载：

| 场景 | 先看哪里 |
|---|---|
| 查当前进度 / 该做什么 | `docs/TODO.md` 续接点块 |
| 改 Core / Agent / Wake 代码 | `project/threadline/AGENTS.md` → `docs/mso/AGENTS.md` → 设计文档 |
| 判断某设计是否违反 CAW 不变量 | `C:\my\methodology\core-agent-wake\AGENTS.zh-CN.md` §2/§4 |
| 判断某结构是否违反 MSO | `project/threadline/docs/mso/AGENTS.md` 硬规则节 |
| 验证没改坏 | `docs/TESTPLAN.md` + `scripts/verify.sh` |
| 回答「当时为什么这么定」 | `docs/decisions/`，其次 `docs/DONE.md` |
| 写文档 / 做独立审查 | `C:\my\methodology\multi-project-workspace\docs\05-writing.md`、`04-ai-collaboration.md` |

## 协作铁律

1. **新会话先读 `docs/TODO.md` 的续接点块**，再决定做什么；绝不凭记忆假设进度。
2. **工作区边界**：本工作区只管项目之间的协作（路由、跨项目文档、提交纪律）；项目内部治理以 `project/threadline/AGENTS.md` 为准。
3. **仓库边界**：项目代码只在子仓库内提交；外层仓库提交前 `git status` 确认无子仓库内容被暂存。
4. **提交纪律**：外层与子仓库独立提交，一次提交只做一件事；绝不把 `project/` 内容提交进外层。
5. **设计先行**：架构/新功能先出设计文档，经拍板再出实施计划；拍板（含「不做」）当天记录在案。
6. **门禁**：本机门禁 `scripts/verify.sh` 一条命令全绿才提交；批次收口以环境门禁（真实 SSH 服务器上的转发实测）为准。
7. **审查在用户拍板后执行**：设计文档送拍板前、不可逆动作（push/发布）前，由独立上下文的子智能体审查（修复-复审最多 3 轮，不嵌套，控制权在主会话）。
8. **坑位沉淀**：门禁/真机揪出的缺陷当天闭环——修复 + 回归测试 + 文档记录；发现值得沉淀的新内容时先提醒用户拍板。
9. **文件边界**：AI 产出的一切文件只写 `C:\my\threadline` 内；工具链安装（Rustup/VS Build Tools）与 pnpm 缓存属系统级例外。
10. **当前进度**：B0–B6 已完成；**真实 SSH 转发已在真机跑通**（经转发访问 `1.1.1.1:80` 拿到 301），NSIS 已实装过一次。剩余为断线/keepalive/UAC/零登录会话等真机验证，以及一个**环境阻塞项**：本机安全客户端按可执行文件路径拦截出网（详见 `docs/knowledge/guides/connection-failure-triage.md`），需把安装目录加白名单。踩坑沉淀见 `docs/knowledge/`。详情见 `docs/TODO.md` 续接点。

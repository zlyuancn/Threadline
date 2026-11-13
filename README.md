# Threadline（引线）多项目工作区

Windows 桌面 SSH 本地端口转发工具的工作区仓库。外层仓库管理工作区文档、AI 协作指引与全部 todo/plan 文档；项目本体作为**嵌套的独立 git 仓库**放在 `project/` 下，保留完整历史与独立发布。

## 目录结构

```
threadline/
├── README.md          # 本文件：工作区说明（给人看）
├── AGENTS.md          # AI 助手入口文档（AI 协作必读）
├── docs/              # 跨项目文档
│   ├── TODO.md        # 路线图 + 待办 + 续接点（以它为准）
│   ├── DONE.md        # 已完成台账
│   ├── plans/         # 设计文档与实施计划
│   ├── decisions/     # 决策记录（NNNN-<标题>.md，只增不改）
│   ├── TESTPLAN.md    # 功能测试项清单
│   └── archive/       # 完成记录与回归战报
├── scripts/           # verify.sh（本机门禁一条命令）
└── project/           # 项目仓库收纳于此（不纳入外层版本管理）
    └── threadline/    # 项目本体
```

## 项目一览

### threadline

- **定位**：Windows 上的 SSH 隧道管理工具——把远端服务器端口转发到本机端口（`-L` 本地转发），多隧道并行、开机自启、快捷唤起。
- **形态**：Rust（无头 Core）+ Tauri 2 桌面客户端（托盘 + 控制面板）+ 瞬态唤醒入口；CAW 三进程模式；MSO 三层架构。
- **关键文档**：`project/threadline/README.md`、`project/threadline/AGENTS.md`；设计文档在本仓库 `docs/plans/`。
- **发布门禁**：本机门禁 `scripts/verify.sh` 全绿 + 真实 SSH 服务器端到端转发验证。

## 项目关系

当前只有一个项目，无跨项目契约。

## 提交约定

- 外层仓库：仅文档变更，`project/` 已被 `.gitignore` 排除。
- 项目变更：进入 `project/threadline` 单独 commit/push，外层不会带上项目代码。

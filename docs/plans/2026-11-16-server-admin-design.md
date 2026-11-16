# 服务器管理 · 设计

> 状态：**已拍板**（2026-11-16，用户要求「隧道卡片上给一个按钮，点进去管理这台服务器」）
> 本次交付：**P1 概览 + 全套模块骨架**。其余模块只留位置，不假装能用。

## 目标

隧道连上之后，用户对这台机器的了解不该停在「转发通不通」。点隧道卡片上的「管理服务器」，直接进入这台机器的管理页：先看概览（系统 / CPU / 内存 / 负载 / 磁盘 / 流量 / 磁盘 IO / 容器 / 进程），未来的终端、文件、防火墙、网络等模块在同一页里从左侧导航进入。

## 原则

| 原则 | 说明 |
|---|---|
| 复用会话 | 不另开 SSH 连接：走 `TunnelRunner` 已有的那条会话（未连接时按需建立一次），与端口转发共享 |
| 只读优先 | P1 只执行**只读**采集脚本，不写远端任何东西；写操作（结束进程、删文件、防火墙改规则）逐模块单独设计 |
| 一次往返 | 概览的所有指标由**一条** `sh` 脚本一次采完（内含 0.3s 二次采样算速率），避免 N 次 exec 的延迟叠加 |
| 缺块不算失败 | 单块采集失败（没有 docker、`df` 超时、非 Linux）只让该块标 `degraded`，整页仍可用 |
| 平台边界 | 只认 Linux（`/proc` 与 `sh`）；非 Linux 时概览整体给出明确原因，不做 Windows 采集 |
| 预留是真预留 | 模块清单由**后端**给（`AdminModule[]`），前端只按清单渲染导航与占位；新增模块 = 后端加一项 + 前端加一个组件 |

## 模块分期

| 期 | 模块 | 内容 | 本次 |
|---|---|---|---|
| P1 | `overview` 概览 | 系统信息、CPU、内存/Swap、负载、磁盘、网络流量、磁盘 IO、Top 进程、容器简况 | ✅ 完整 |
| P2 | `processes` 进程管理 | 全量进程列表、排序、结束进程（需确认） | 占位 |
| P2 | `files` 文件管理 | 目录浏览、下载、上传、删除、重命名（SFTP 子系统） | 占位 |
| P2 | `terminal` 终端 | PTY + 交互式 shell（`request_pty` + 双向 channel），前端需 xterm.js | 占位 |
| P3 | `containers` 容器管理 | docker/podman 列表、启停、日志 | 概览里只给汇总，管理页占位 |
| P3 | `network` 网络管理 | 网卡、路由、监听端口、DNS | 占位 |
| P3 | `firewall` 防火墙 | ufw / firewalld / iptables 规则查看与开关 | 占位 |
| P4 | `services` 服务与自启 | systemd 单元列表、启停 | 占位 |
| P4 | `logs` 远程日志 | journalctl / 指定日志文件跟随 | 占位 |

## 接口契约

统一挂在 `/api/v1/servers/{id}/admin/` 下，后续模块不另起前缀。

```
GET /api/v1/servers/{id}/admin/overview
→ ServerOverview {
    collected_at, elapsed_ms,
    system, cpu, load, memory,
    disks[], network[], disk_io[],
    processes[], containers,
    modules: AdminModule[],   // 导航清单（ready / planned / unavailable）
    degraded: string[]        // 采集失败的块，UI 打黄条
  }
```

未来（P2+）沿用同一前缀与返回值风格：

| 端点 | 用途 |
|---|---|
| `GET /servers/{id}/admin/processes?sort=cpu` | 全量进程 |
| `POST /servers/{id}/admin/processes/{pid}/kill` | 结束进程 |
| `GET /servers/{id}/admin/files?path=` | 列目录 |
| `GET /servers/{id}/admin/files/download?path=` | 下载（流式） |
| `POST /servers/{id}/admin/files/upload` | 上传 |
| `GET /servers/{id}/admin/terminal` | WebSocket 终端（P2 再定协议） |

## 分层归属

| 位置 | 职责 |
|---|---|
| `models/tunnel/engine.rs` `exec_capture` | SSH 通道上的命令执行：开 channel → exec → 收 stdout/stderr → 退出码，带超时预算 |
| `models/server_admin/collect.rs` | 采集脚本（**只读**、POSIX sh、一次往返） |
| `models/server_admin/parse.rs` | 分节解析 + 指标计算（纯函数，单测覆盖） |
| `models/server_admin/mod.rs` | 模块清单与能力判定（有 docker 才 `ready`；非 Linux 全部 `unavailable`） |
| `business/admin.rs` | 编排：取会话 → 执行脚本 → 解析 → 组装 `ServerOverview` |
| `interface/mod.rs` | 路由 + 错误翻译 |
| `src/models/serverAdmin.ts` | 前端状态：加载、刷新、错误、上次采集时间 |
| `src/views/ServerAdminView.vue` | 管理页：导航 + 概览 + 占位页 |
| `HomeView` / `App.vue` | 卡片按钮 → 切到管理页（带 `server_id`），返回回到隧道页 |

数据独占：`server_admin` 不持有任何持久状态（无文件、无缓存），只提供「脚本 + 解析 + 清单」三件纯能力；会话归 `tunnel`，只经 `engine` 的公开操作使用。

## 采集脚本要点

| 数据 | 来源 |
|---|---|
| 主机名 / OS / 内核 | `hostname`、`/etc/os-release`、`uname -r`、`uname -m` |
| 运行时长 | `/proc/uptime` 第 1 列 |
| CPU 型号与核数 | `/proc/cpuinfo` |
| CPU 使用率 | `/proc/stat` 两次采样（间隔 0.3s）差分，分 user/system/iowait |
| 负载 | `/proc/loadavg` |
| 内存 / Swap | `/proc/meminfo`（缺 `MemAvailable` 时回退 `free+buffers+cached`） |
| 磁盘用量 | `df -kP`（POSIX），过滤 tmpfs/devtmpfs/overlay/squashfs，取前 12 条 |
| 磁盘 IO | `/proc/diskstats` 两次采样，只认 `sd*/nvme*/vd*/hd*/xvd*` |
| 网络流量 | `/proc/net/dev` 两次采样，跳过 `lo/docker/veth/br-/virbr` |
| 进程 Top | `ps -eo pid,pcpu,pmem,rss,comm --sort=-pcpu`（不可用则该块 degraded） |
| 容器 | `docker ps -a --format '...'`（无 docker 则该块 degraded 且模块标 unavailable） |

## 边界

- **不做写操作**：P1 没有「杀进程 / 删文件 / 改防火墙」的入口，避免管理页变成能把机器搞坏的地方。
- **不做前台定时轮询**：概览手动刷新 + 首次进入自动拉一次；秒级实时曲线留给 P2 的监控模块（要另设计采样推送）。
- **不做多服务器聚合**：管理页只对当前选中的那条隧道。
- **不引入新前端依赖**：概览用现有 CSS 与 Element Plus；xterm.js 等终端依赖等到 P2 再评估体积。
- **不落盘**：采集结果只在内存里活一次请求，不写 `%PUBLIC%\Threadline`。

## 验收

1. 隧道已连接 → 卡片出现「管理服务器」→ 点进管理页，8 秒内出现系统信息与各项指标。
2. 隧道未连接 → 点「管理服务器」→ 自动建立会话后正常显示；建不起来时页面给出与隧道页一致的失败原因。
3. 服务器没有 docker → 概览容器块显示「未安装」，容器模块在导航里标为不可用，不报错。
4. 服务器是 Windows / 无 `/proc` → 概览给出「仅支持 Linux」的原因，不出现空白页。
5. 左侧导航列出全部模块，未实现的显示「规划中」+ 该模块将做什么，点了不空白。
6. 刷新按钮 → 指标更新，`elapsed_ms` 显示本次采集耗时。

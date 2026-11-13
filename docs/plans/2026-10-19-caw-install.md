# Threadline 的 CAW 落地方案（安装记录）

- 方法论来源（只读）：`C:\my\methodology\core-agent-wake\`（版本戳 `41f698a`）
- 安装日期：2026-10-19
- 完整设计：`../../docs/plans/2026-10-19-threadline-design.md`（工作区 `docs/plans/`）

## 1. 三进程职责

| 角色 | 产物 | 身份 | 职责 | 绝不 |
|---|---|---|---|---|
| Core | `tl-core.exe` | SYSTEM（开机自启）/ 当前用户（双击启动、回退）/ 提权一次性（`--autostart`） | SSH 转发数据面 + `/api/v1` 控制面 + 配置与凭据 + 注册层变更 | 渲染 UI、开浏览器、写 HKCU |
| Agent | `tl-agent.exe` | 登录用户 | 托盘 + 面板（Tauri 2 + Vue 3）+ 自己的 HKCU 自启项 | 持锁、占端口、持久化服务状态、写任务计划 |
| Wake | `tl.exe` | 登录用户 | 探测 → 拉起 → 打开面板 → 退出（≤20s） | 驻留、定时重拉 |

## 2. 控制端点与鉴权

- 监听 `127.0.0.1`，端口 18921 起（占用顺延到 19020，全占 → 退出码 5）。
- 发现：`%ProgramData%\threadline\control.json`（内部 tag `status: running|stopped`），每次启动轮换 token；解析失败视为不存在。
- 实例身份校验：`/healthz` 回传 `instance_id`，与 `control.json` 比对，不匹配即判「不是我们的核心」。
- 鉴权：`Authorization: Bearer <token>`，常量时间比较；仅 `/healthz` 免鉴权。
- 退出：`POST /shutdown` 先落盘并回 `202`，随后进程退出（C4）。

## 3. 单实例：文件锁

`Global\` 命名对象对标准用户不可创建（需 `SeCreateGlobalPrivilege`），故用：

- `%ProgramData%\threadline\core.lock` + `LockFileEx`（`LOCKFILE_FAIL_IMMEDIATELY` 轮询 100ms，预算 2s）。
- ACL：`SYSTEM:F` + `Administrators:F` + `Users:RW`。
- 定序：抢锁 → 绑端口 → 原子写 `control.json`。
- 打不开 → 退出码 7；抢不到 → 退出码 3（正常，已有核心在跑）。

## 4. 开机自启 = 一个开关

| 关注点 | 做法 |
|---|---|
| 开 | 创建任务计划 `ThreadlineCore`：ONSTART，SYSTEM，RunLevel=Highest，失败后重启最多 3 次 |
| 关 | 删除任务计划 |
| 执行者 | 当前 Core 是 SYSTEM → 自己改；是用户身份 → `409 privilege_required`，由 Agent 经**一次** UAC 启动 `tl-core --autostart on\|off` |
| 与运行的关系 | **只改注册层，绝不停正在跑的核心**（I5）；切换在下一次开机生效，本次运行身份不变 |
| 结果回传 | `--autostart` 写 `autostart.result.json{ok,code,field?,message,at}`，Agent 读文件 + `GET /autostart` 复核 |
| 前置校验 | 路径全绝对、无用户环境依赖、日志去向显式；不满足**带字段名**拒绝（R5） |

`--autostart` 是独立 CLI 模式：不抢 `core.lock`、不绑端口、不写 `control.json`。

## 5. 停止服务 = 现在退出，不影响自启

- 托盘「停止服务」→ `POST /shutdown` → 停全部隧道、断 SSH、落盘、`control.json` 置 stopped、退出码 **0**。
- 不碰注册层（I5）。本次开机不再自动起（任务计划只 ONSTART 触发）；重启后照常自启。
- 关面板只退出 Agent，Core 与转发继续。

## 6. Wake 算法（每步有预算，总 20s）

1. 读 `control.json` → `GET /healthz` → 比对 `instance_id`（1.5s）。
2. 不可达且任务计划已启用 → `schtasks /run` 并等探测（3s + 5s）。
3. 仍不可达 → 以当前用户身份拉起 `tl-core --stand-in`（自启开着却起不来）或 `--manual`（自启本来关着）（2s + 5s）。
4. 启动 `tl-agent --show`（Agent 自身单实例去重）（3s）。
5. 核心仍不可达 → 打印诊断 → **退出码 4**。
6. Agent 拉起失败 → **退出码 6**。
7. 正常 → **退出码 0**。

**不做定时重拉**：会与「停止服务」语义冲突。用户态核心消失后接棒发生在下一次开机。

## 7. 退出码

`0` 成功 · `2` 参数错误 · `3` 已有核心持锁 · `4` 核心不可达且拉起失败 · `5` 控制端口全占 · `6` Agent 拉起失败 · `7` 数据目录不可用。

任务计划只配「失败后重启最多 3 次」，不配周期重复触发；优雅退出码 0 不触发重启（I8）。

## 8. 回退与降级（I9 必须可见）

| 场景 | 标记 |
|---|---|
| 自启已开但 SYSTEM 核心没起来，Wake 拉成用户身份 | `stand_in_core` |
| 无法注册任务计划，前台运行 | `fallback_run` |
| ACL 未收紧 | `credential_acl_relaxed` |
| 凭据不可读（Agent 缩减为只读 UI） | `credential_unreadable` |
| 运行期本地端口丢失 | `local_port_lost` |

## 9. 明确不采用

- **CAW R11 单进程合并**：有桌面会话与托盘，分离可成立；合并即 AP1。`tl-core`/`tl-wake` 禁任何 GUI 依赖，由 msocheck R4 拦截。

## 10. 凭据

- `%ProgramData%\threadline\credentials.dat`，machine-scope DPAPI，首跑即机器级创建。
- ACL：`SYSTEM:F` + `Administrators:F` + `install.json` 的交互用户 SID:F；取不到 → `Users:R` + `credential_acl_relaxed`。
- 目录 ACL：`SYSTEM:F | Administrators:F | Users:(OI)(CI)RX | <交互用户>:(OI)(CI)M`。
- **已知边界**：`control.json` 对 `Users:R` 开放 → 本机任意标准用户可读 token 并停服务；`core.lock` 的 `Users:RW` 允许本地进程持锁阻止核心启动。单机单用户前提下 v1 接受。

## 11. 验收

对照 `core-agent-wake/AGENTS.zh-CN.md` §4 评审清单逐条实测，测试项见工作区 `docs/TESTPLAN.md`「二、CAW 不变量」。

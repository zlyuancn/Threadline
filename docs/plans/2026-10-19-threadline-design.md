# Threadline（引线）设计文档

- 日期：2026-10-19
- 状态：**已拍板，进入实施**
- 方法论：CAW `41f698a`（三进程）、多项目工作区 `f78fd6e`、MSO `5c432f9`
- 项目目录：`C:\my\threadline\project\threadline`

---

## 1. 做什么

Windows 上的 SSH 本地端口转发工具：把远端服务器端口映射到本机端口（`-L`），多隧道并行、可开机自启、双击即用。

**目标**

1. `-L` 本地转发，浏览器直接开 `http://127.0.0.1:<本地端口>`。
2. 多条隧道并行，各自独立启停、独立状态、独立重连。
3. 开机自启做成一个**开关**（开 = 登录前就在跑；关 = 用时双击）。
4. 托盘常驻，双击 `tl.exe` 秒开面板。

**非目标**

| 不做 | 理由 |
|---|---|
| `-R` / `-D` SOCKS5 | 需求只到"把远端服务引到本机" |
| 引用 `~\.ssh` 或外部私钥路径 | 凭据由使用者填写并机器级托管 |
| 多账户各跑一套 | 单机单用户工具 |
| Linux / macOS | 只做 Windows |
| 单进程合并形态（CAW R11） | 有桌面会话与托盘，分离可成立；合并即 AP1 |
| 服务运行的周期看护 | 用户点「停止服务」就是停止，不做定时重拉（见 3.4） |

---

## 2. 三个进程

| 进程 | 产物 | 身份 | 存活期 | 职责 | 绝不做 |
|---|---|---|---|---|---|
| Core | `tl-core.exe` | SYSTEM（开机自启）/ 当前用户（双击启动、回退）/ 提权一次性（`--autostart`） | 持久 | SSH 转发数据面；`/api/v1` 控制面；配置与凭据存取；注册层变更 | 渲染 UI、开浏览器、写 HKCU |
| Agent | `tl-agent.exe` | 登录用户 | 会话存续期 | 托盘 + 面板（Tauri 2 + Vue 3）；自己的 HKCU 自启项 | 持锁、占端口、持久化服务状态、碰凭据明文 |
| Wake | `tl.exe` | 登录用户 | 秒级（≤20s） | 探测 → 补齐缺的 → 打开面板 → 退出 | 驻留、定时重拉、无限等待 |

Agent 与 Wake 都是 `127.0.0.1:<port>` + `Bearer <token>` 的普通 HTTP 客户端，只走控制 API（CAW C1），不新增管道/套接字/推送。

> Agent 的"每会话一个"由 Agent 自身保证（`Local\threadline-agent-<session>` 互斥体 + Tauri single-instance 的 `--show` 转发）。这是 Agent 与 Agent 之间的会话内机制，不承载服务状态。

---

## 3. 三个关键语义

### 3.1 开机自启 = 一个开关

| 状态 | 含义 | 注册层 | Core 身份 |
|---|---|---|---|
| 开 | 登录前就在跑 | 任务计划 `ThreadlineCore` 存在且启用（ONSTART，SYSTEM） | SYSTEM |
| 关 | 用时双击 | 任务计划不存在 | 当前用户 |

- 开关**只增删任务计划**，绝不停止或重启正在跑的 Core（CAW I5）。
- 当前 Core 是 SYSTEM 时，开关由 Core 自己改（无需提权）；当前 Core 是用户身份时，Core 无权改 SYSTEM 任务 → 返回 `409 privilege_required`，UI 出横幅 + 「以管理员权限重试」，点击后由 Agent 经**一次** UAC 启动 `tl-core --autostart on|off`（CAW R6：一次授权，不循环弹）。
- 切换后**立即生效于下一次开机**；本次运行的 Core 身份不变，UI 明确显示：「已开启/关闭开机自启，当前进程仍在运行」。

### 3.2 停止服务 = 现在退出，不影响自启

- 托盘「停止服务」→ `POST /shutdown` → Core 停全部隧道、断 SSH、落盘、写 `control.json` 为 stopped、退出码 **0**。
- **不碰注册层**（CAW I5）：开机自启开关保持原样。
- 本次开机内不会再被自动拉起（任务计划只在开机触发）；**重启后照常自启**。
- 「关闭面板」≠「停止服务」：关面板只退出 Agent，Core 与转发继续。

### 3.3 Core 是怎么起来的（`run_mode`）

| `run_mode` | 触发 | 身份 |
|---|---|---|
| `boot` | 任务计划（开机） | SYSTEM |
| `manual` | 双击 `tl.exe` 拉起 | 当前用户 |
| `stand_in` | 开机自启已开、但 SYSTEM 核心没起来，Wake 兜底拉起 | 当前用户 |
| `fallback` | 无法注册任务计划时的前台运行 | 当前用户 |

`stand_in` 是**降级标记**（期望 SYSTEM 在跑，实际是用户身份），`manual` 不是降级（自启本来就是关的）。降级集合单独上报，见 3.6。

### 3.4 Wake 算法（每条路径有上界，总预算 20s）

| 步 | 动作 | 预算 | 失败去向 |
|---|---|---|---|
| 1 | 读 `control.json`（解析失败视为不存在）→ `GET /healthz` → 比对 `instance_id` | 1.5s | 第 2 步 |
| 2 | 核心不可达 → 读注册层：任务计划**存在且启用** → `schtasks /run` 并等探测 | 3s + 5s | 第 3 步 |
| 3 | 仍不可达 → 以当前用户身份 detached 拉起 `tl-core --stand-in`（若任务计划启用）或 `--manual`（若未启用），并等探测 | 2s + 5s | 第 4 步 |
| 4 | 启动 `tl-agent --show`（不探测：Agent 自身去重，已存在则转发 `--show` 弹窗后自退） | 3s | 第 5 步 |
| 5 | 核心仍不可达：打印数据目录、端口、最近 20 行日志 | — | **退出码 4** |
| 6 | Agent 拉起失败：打印诊断 | — | **退出码 6** |
| 7 | 正常 | — | **退出码 0** |

**为什么没有"定时重拉"**：定时轮询会把用户刚点「停止服务」的核心又拉起来，与 3.2 直接冲突。用户态 Core 消失后（停止/注销），接棒发生在**下一次开机**，由任务计划把 SYSTEM 核心起来。这是刻意的取舍，不是遗漏。

### 3.5 单实例：文件锁

`Global\` 命名互斥体对标准用户不可用（创建需要 `SeCreateGlobalPrivilege`），因此用文件锁：

| 项 | 做法 |
|---|---|
| 文件 | `%ProgramData%\threadline\core.lock`（0 字节，无秘密） |
| 创建 | 安装器；缺失时由最先启动的 Core 创建 |
| ACL | `SYSTEM:F` + `Administrators:F` + `Users:RW` |
| 加锁 | `LockFileEx` + `LOCKFILE_FAIL_IMMEDIATELY`，轮询 100ms，总预算 2s |
| 释放 | 进程退出时 OS 自动释放（崩溃也不留死锁） |
| 打不开 | **退出码 7**（数据目录不可用），日志写明路径与 `GetLastError` |
| 抢不到 | 读 `control.json` + `/healthz` 校验 `instance_id` → 退出码 **3**（已有核心在跑，正常退出，不是错误） |
| 定序 | 抢锁 → 绑端口 → 原子写 `control.json`（三个 Core 不可能同时写） |

### 3.6 退出码

| 码 | 含义 |
|---|---|
| 0 | 成功（Wake 正常完成 / Core 优雅退出 / `--autostart` 执行成功） |
| 2 | 命令行参数错误 |
| 3 | 已有 Core 持锁（正常退出） |
| 4 | 核心不可达且拉起失败 |
| 5 | 控制端口 18921–19020 全被占 |
| 6 | Agent 拉起失败（核心已在跑） |
| 7 | 数据目录不可用 / `control.json` 写失败 |

任务计划只配「失败后重启最多 3 次」（崩溃自愈）；**不配周期重复触发**（否则会与 3.2 冲突）。优雅退出码 0 不触发重启（CAW I8）。

### 3.7 降级标记（CAW I9，必须可见）

`stand_in_core`（该由 SYSTEM 跑却是用户身份）/ `fallback_run`（无法注册监督者）/ `credential_acl_relaxed`（ACL 未收紧）/ `credential_unreadable`（凭据读不出）/ `local_port_lost`（运行期端口丢失，极罕见）。

---

## 4. 数据与磁盘

### 4.1 目录

```
%ProgramData%\threadline\
├── core.lock              # 单实例锁（0 字节）            SYSTEM/Admins:F, Users:RW
├── control.json           # 发现契约：端口 + token         SYSTEM/Admins:F, Users:R
├── install.json           # 安装期：交互用户 SID           Users:R
├── autostart.result.json  # --autostart 一次性进程的结果    Users:R
├── servers.json           # server 模型独占                继承目录
├── tunnels.json           # tunnel 模型独占                继承目录
├── credentials.dat        # credential 模型独占（DPAPI）   SYSTEM/Admins + 交互用户:F
└── logs\                  # 按天滚动保留 7 天             继承目录
```

目录 ACL：`SYSTEM:F | Administrators:F | Users:(OI)(CI)RX | <交互用户>:(OI)(CI)M`。绿色免安装（无 `install.json`）时退化为 `Users:M` 并打 `credential_acl_relaxed`。

### 4.2 `control.json`（发现契约，CAW C2）

```json
{
  "status": "running",
  "version": "0.1.0",
  "instance_id": "3f9c1a7e",
  "pid": 12876,
  "identity": "SYSTEM",
  "run_mode": "boot",
  "host": "127.0.0.1",
  "port": 18921,
  "token": "pXh2...base64...==",
  "started_at": "2026-10-19T09:12:03+08:00"
}
```

- 内部 tag `status: running|stopped`（不用 untagged，避免 `{"stopped":false}` 之类被误判）。
- 端口首选 `18921`，占用顺延到 `19020`，全占 → 退出码 5。
- token 每次启动轮换；`instance_id` 每次启动生成，`/healthz` 回传同值，比对不上即判「不是我们的核心」。
- 退出时原子替换为 `{"status":"stopped","version":"0.1.0"}`；解析失败一律视为「不存在」。

### 4.3 其余磁盘结构

| 文件 | 结构 | 拥有者 |
|---|---|---|
| `servers.json` | `{schema_version, items:[ServerDto]}` | `server` |
| `tunnels.json` | `{schema_version, items:[TunnelDto]}` | `tunnel` |
| `credentials.dat` | DPAPI(`{schema_version, items:[CredentialItem]}`) | `credential` |
| `install.json` | `{schema_version, interactive_user_sid, install_scope}` | 安装器写、Core 只读 |
| `autostart.result.json` | `{ok, code, field?, message, at}` | `--autostart` 一次性进程写、Agent 读 |

`CredentialItem` 只存在于 DPAPI 密文内，任何 HTTP 端点都不返回。

---

## 5. 控制 API

前缀 `/api/v1`。除 `healthz` 外全部要求 `Authorization: Bearer <token>`。

| 方法 | 路径 | 请求体 | 响应 | 主要错误 |
|---|---|---|---|---|
| GET | `/healthz` | — | `HealthResponse` | — |
| GET | `/whoami` | — | `WhoamiResponse`（含 `autostart`） | 401 |
| GET | `/servers` | — | `ServerList` | 401 |
| POST | `/servers` | `CreateServerRequest` | `ServerDto` | 400 `field` / 409（凭据不存在） |
| PATCH | `/servers/{id}` | `PatchServerRequest` | `ServerDto` | 400 / 404 / 409 |
| DELETE | `/servers/{id}` | — | 204 | 404 / 409（仍被隧道引用） |
| GET | `/servers/{id}/hostkey` | — | `HostKeyView` | 404 |
| POST | `/servers/{id}/hostkey/accept` | `AcceptHostKeyRequest` | `ServerDto` | 404 / 409 |
| GET | `/tunnels` | — | `TunnelList` | 401 |
| POST | `/tunnels` | `CreateTunnelRequest` | `TunnelStatus` | 400 / 409 |
| PATCH | `/tunnels/{id}` | `PatchTunnelRequest` | `TunnelStatus` | 400 / 404 / 409 |
| DELETE | `/tunnels/{id}` | — | 204 | 404 |
| POST | `/tunnels/{id}/start` | — | `TunnelStatus` | 404 / 409 |
| POST | `/tunnels/{id}/stop` | — | `TunnelStatus` | 404 / 409 |
| GET | `/credentials` | — | `CredentialList`（无任何密钥字段） | 401 |
| POST | `/credentials` | `CreateCredentialRequest` | `CredentialSummary` | 400 `field` |
| PATCH | `/credentials/{id}` | `PatchCredentialRequest` | `CredentialSummary` | 400 / 404 |
| DELETE | `/credentials/{id}` | — | 204 | 404 / 409 |
| GET | `/autostart` | — | `AutostartStatus` | 401 |
| POST | `/autostart` | `SetAutostartRequest{enabled}` | `AutostartStatus` | 400 / 409 `privilege_required` |
| POST | `/shutdown` | — | `ShutdownAccepted`（202，先返回再退出） | 401 |
| GET | `/logs?tail=n` | — | `LogsResponse`（默认 200，上限 2000） | 400 |

统一错误体：`{"error":{"code","message","field?"}}`；字段级错误必带 `field`（CAW R5）。

---

## 6. 鉴权与安全

- 只监听 `127.0.0.1`；无 TLS（回环 + token）；token 常量时间比较；请求体上限 1 MiB；只收 `application/json`。
- 凭据：`CryptProtectData` + `CRYPTPROTECT_LOCAL_MACHINE`，首跑即机器级创建；ACL = SYSTEM + Administrators + 交互用户 SID；取不到 SID → `Users:R` + `credential_acl_relaxed`。
- 明文只出现在面板输入框与 `POST|PATCH /credentials` 请求体；Core 收到即加密，不写日志、不进错误信息、不回显。
- **已知边界（如实记录）**：`control.json` 对 `Users:R` 开放 → 本机任意标准用户可读 token，进而读配置、读日志、`POST /shutdown` 停服务。`core.lock` 的 `Users:RW` 意味着恶意本地进程可以持锁阻止核心启动（只能靠重启/找到该进程解除）。这是"单机单用户"前提的代价，v1 接受。
- 多账户：只有安装时的交互用户被精确授权，其他账户得到 `credential_acl_relaxed` 与缩减 UI。

---

## 7. MSO 分层与模型契约

### 7.1 依赖方向

```
shell → interface → business → models → foundation
```

- `shell`：**只有 `src/main.rs`**（命令行解析、tracing、组合根装配、按参数分派 `--boot/--manual/--stand-in/--fallback/--autostart`）。
- `interface`：axum 路由、Bearer 鉴权、DTO↔内部类型、字段级校验。零业务规则。
- `business`：用例编排（`startup_autostart` / `start_tunnel` / `set_autostart` / `shutdown`）。不藏判定规则。
- `models`：数据与规则所有者。
- `foundation`：无业务语义机制（原子存储、DPAPI、日志、token、退避/超时/并发预算、`LockFileEx`、注册层 Win32 封装、HTTP 装配）。

### 7.2 模型契约

| 模型 | 一句话 | 拥有数据 | 提供操作 | 允许调用 |
|---|---|---|---|---|
| `credential` | 我拥有凭据与加解密 | `credentials.dat` | `list` / `add` / `update_secret` / `remove` / `material(id)` / `is_referenced_by_server(id)` | `foundation::secret` |
| `server` | 我拥有服务器定义与主机密钥信任 | `servers.json` + 待确认指纹缓存 | `list` / `upsert` / `remove` / `connect_params(id)` / `host_key_view(id)` / `accept_host_key(id, fp)` / `is_referenced_by_tunnel(id)` | `credential`、`foundation::store` |
| `tunnel` | 我拥有转发规则与运行态 | `tunnels.json` + 运行态 | `list` / `upsert` / `remove` / `start` / `stop` / `stop_all` / `status` / `on_session_lost` / `is_port_taken(port)` | `server`、`foundation::{retry,limiter}` |
| `runtime` | 我拥有控制信息、运行身份与自启开关 | `control.json` / `install.json`（只读）/ 注册层观测 | `whoami` / `autostart_status` / `set_autostart(enabled)` / `write_stopped` / `degradations` | `foundation::{store,registry,lock}` |

**引用完整性**：`DELETE /servers/{id}` 的"仍被隧道引用"由 `server.is_referenced_by_tunnel(id)` 判定；`DELETE /credentials/{id}` 的"仍被服务器引用"由 `credential.is_referenced_by_server(id)` 判定。规则归被引用方，business 只做"先问再删"。

**横向调用**：`tunnel → server → credential`，单向无环。两条只读反向询问（`is_referenced_by_*`）纯读、无副作用，在各模型文件里显式标注 `termination: phase`。

### 7.3 终止保障（底座统一提供，模型禁止私有实现）

| 场景 | 机制 | 参数 |
|---|---|---|
| SSH 握手 | 超时 | 10s |
| 单隧道本地并发 | 并发预算 | 64，超出立即关闭 |
| 断线重连 | 有界退避 | 1→2→4→8→16→30s 封顶，连续 10 次 → `Error` |
| keepalive 失效 | 超时 + 计数 | 30s 一次，连续 3 次无响应 → `Backoff` |
| 控制 API 单请求 | 超时 | 15s |
| 主机密钥确认 | 超时 | 300s 未确认 → `Error(trust_timeout)` |
| Wake 全流程 | 总预算 | 20s |
| 本地连接回收 | 预算回收 | 任一方向 EOF/错误即关两端 |

### 7.4 前端三层

`src/views/`（只渲染与交互，禁 axios/fetch/业务判定）→ `src/api/`（唯一 HTTP 出口）→ `src/models/`（Pinia store 与前端规则）。

---

## 8. SSH 引擎

**选型**：`russh`（纯 Rust）。需要逐隧道状态可见性与重连控制，`ssh.exe` 子进程只能靠端口探测猜状态。

**启动顺序**（顺序即规则，写在 `tunnel::start`）：① 取连接参数 → ② 取认证材料（进程内解密）→ ③ **先绑** `bind(local_host:local_port)` + `SO_EXCLUSIVEADDRUSE`（失败 → `Error(local_port_in_use)`，不重试）→ ④ **后连** `connect`（10s）+ 主机密钥校验 → ⑤ accept 循环，每连接 `channel_open_direct_tcpip` → 双向拷贝。

**断线**：**保持监听不释放**，转 `Backoff`，期间新连接立即关闭（快速失败）；重连只重建 SSH 会话，不重绑端口。重连成功回 `Running`。

**状态与资源语义**

| 状态 | 持监听 | SSH 会话 | 新连接 | 迁移 |
|---|---|---|---|---|
| `Stopped` | 否 | 无 | 无监听 | → `Connecting` |
| `Connecting` | 是 | 握手中 | 立即关闭 | → `Running` / `NeedsTrust` / `Error` |
| `NeedsTrust` | 是 | **无**（握手已丢弃） | 立即关闭 | → `Connecting`（确认后重新握手）/ → `Stopped`（拒绝）/ → `Error`（300s 超时） |
| `Running` | 是 | 已建立 | 正常转发 | → `Backoff` / → `Stopped` |
| `Backoff` | 是 | 断开 | 立即关闭 | → `Connecting` / → `Error`（10 次） |
| `Error` | 否 | 无 | 拒绝 | → `Connecting`（用户重试） |

`stop`/`stop_all` 覆盖全部状态收敛（含取消挂起握手、丢弃待确认信任）。

**主机密钥**：不做隐式 TOFU。首次连接握手后取指纹 → `NeedsTrust`，UI 显示指纹并要求确认（`POST /servers/{id}/hostkey/accept`）→ 重新握手 → `Running`；指纹不匹配 → `Error(host_key_mismatch)`，绝不自动接受。`POST /servers` 可预置指纹。

**认证**：`password`（同时尝试 `password` 与 `keyboard-interactive`）、`private_key`（内容 + 可选口令）。服务器不接受该方式 → `Error(auth_method_rejected)`。

---

## 9. 失败模式

| 失败 | 行为 | 标记 |
|---|---|---|
| 首次绑定端口被占 | `Error(local_port_in_use)` + `field=local_port` | — |
| 运行期端口丢失（不应发生） | `Error(local_port_lost)` | `local_port_lost` |
| SSH 认证失败 | `Error`，区分密钥/密码/不被允许 | — |
| 主机密钥未知 | `NeedsTrust`，UI 显示指纹 | — |
| 主机密钥不匹配 | `Error(host_key_mismatch)`，不自动接受 | — |
| 远端端口不可达 | 单连接失败，不触发隧道重连 | 日志 warn |
| 网络中断 / keepalive 超时 | `Backoff`（监听保持） | — |
| 连续 10 次重连失败 | `Error`，停止自动重连 | — |
| 凭据不可读 | 相关隧道不可启动，Agent 缩减为只读 UI | `credential_unreadable` |
| ACL 无法收紧 | 继续运行 | `credential_acl_relaxed` |
| 无法注册任务计划 | 前台 `--fallback` 运行 | `fallback_run` |
| 自启已开但 SYSTEM 核心不在 | Wake 拉起用户身份核心 | `stand_in_core` |
| 控制端口全占 | 退出码 5 | 日志 |
| 数据目录不可用 | 退出码 7 | 日志 |

---

## 10. 机检（MSO §6 可机检子集）

`tools/msocheck/msocheck.mjs`，接入 `scripts/verify.sh`（含 `--selftest`）。

| 规则 | 检查内容 |
|---|---|
| R1 依赖方向 | `src/` 只允许五个层目录 + 顶层 `main.rs`/`lib.rs`；禁反向 import；禁两层以上 `super::`；覆盖 `use crate::{a::X, b::Y}` 与 `#[path]` 重定向 |
| R2 数据独占 | 禁跨模型 `internal`（含 `use crate::models::X::*`、`{internal::Foo}`、`{b::*}` 组合形态） |
| R3 前端收口 | `src/views/**` 禁 axios（含 `import("axios")`）/ `fetch(`（含 `window.fetch` 取引用） |
| R4 核心无头 | `tl-core`/`tl-wake` 的 `Cargo.toml`、`build.rs`、根 `[workspace.dependencies]` 禁 GUI 库（含 `iced`/`slint`）；`windows` crate 禁 `Win32_UI*`/`Win32_Graphics*` 特性 |
| R5 壳不施放 | `src/main.rs` 与 `src/shell/**` 禁 `spawn*/make*/activate*/apply*`（含 `T::spawn()`、turbofish、取函数引用） |
| R6 环与终止 | 模型调用图成环时，环上至少一个模型必须**在代码中**（剥离注释后）调用 `foundation::{retry,limiter,timeout}::`，或声明 `pub const TERMINATION: &str = "phase";` |

不可机检部分（语义越权、写入口唯一、环的业务合理性）由评审清单兜底。

---

## 11. 批次计划

| 批次 | 内容 | 门禁 |
|---|---|---|
| **B0**（完成） | 工作区、项目骨架、契约、机检、门禁 | `bash scripts/verify.sh` 全绿 |
| B1 | `foundation`（store/secret/retry/limiter/lock/registry）+ `models` 纯逻辑 | 状态矩阵、退避上限、`boot`/`manual` 判定、引用完整性 |
| B2 | SSH 引擎（绑定→连接→转发→keepalive→退避→NeedsTrust） | 单测 + **[环境]** 真实 sshd |
| B3 | 控制 API 全端点 + 鉴权 + 错误体 | 每端点 2xx/400/401/404/409 冒烟 |
| B4 | Vue 面板 + Tauri 壳 | 前端构建 + msocheck + 手动走查 |
| B5 | Agent 托盘 + 灰态重挂 + 会话互斥 + 经 API 退出 | 杀 Agent 转发不断、灰态恢复 |
| B6 | Wake + 注册层（自启开关/`--autostart`）+ NSIS 安装器 | **[环境]** 零登录会话、连点单核心、开关切后重启生效 |
| B7 | 收口 | 门禁 + TESTPLAN `[环境]` 全过 + 文档收口 |

---

## 12. 必须实测的假设

| # | 断言 | 需验证 | 批次 |
|---|---|---|---|
| H1 | `LockFileEx` 跨 SYSTEM/用户身份互斥 | 双身份各建进程实测 | B1 |
| H2 | machine-scope DPAPI 双向可解 | SYSTEM 写→用户读、用户写→SYSTEM 读 | B1 |
| H3 | `schtasks /create /f` 由提权进程创建 ONSTART+SYSTEM 任务后，SYSTEM 能在零登录会话启动并绑回环端口 | 未登录状态实测 | B6 |
| H4 | 任务计划「失败后重启最多 3 次」不会把优雅退出（码 0）也重启 | 实测 | B6 |
| H5 | `schtasks /run` 对已启用任务的立即触发行为 | 实测 | B6 |
| H6 | `schtasks /delete` 对**未运行**任务是无害的（开关关闭路径） | 实测；若有副作用改 `/change /disable` | B6 |
| H7 | 安装器能取到交互用户 SID 写入 `install.json` | NSIS 提权上下文的令牌用户 | B6 |
| H8 | `%ProgramData%\threadline` 目录 ACL 允许用户身份 Core 写 JSON | 实测 | B1 |
| H9 | `russh` 握手回调能取到主机密钥指纹 | 决定 `NeedsTrust` 实现方式 | B2 |
| H10 | Tauri `single-instance` 作用域（会话内 vs 全局） | 决定是否额外需要 `Local\` 互斥体 | B5 |
| H11 | `tl-agent --show` 的成败可用退出码表达 | 实测 | B5 |
| H12 | 18921–19020 未被系统保留 | `netsh int ipv4 show excludedportrange` | B3 |
| H13 | `tl.exe` 与 PATH 上其它工具不冲突 | 安装后扫描 | B6 |

---

## 13. 已拍板的决定（不再讨论）

1. 开机自启 = 一个开关，运行期可切；切开关不停正在跑的服务。
2. 停止服务 = 现在退出，不影响自启；本次开机不再自动起，重启后恢复。
3. Wake 在核心缺失时拉起**用户身份** Core；自启已开却拉成用户身份 → 标 `stand_in_core`。
4. 不做定时重拉 / 周期重复触发（与第 2 条冲突）。
5. v1 只做 `-L`；凭据由使用者填写、machine-scope DPAPI、不读 `~\.ssh`。
6. 不采用 CAW R11 单进程合并。

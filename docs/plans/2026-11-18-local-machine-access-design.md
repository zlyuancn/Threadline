# 本机访问 · 设计

> 状态：**已拍板并落地**（2026-11-18，用户提出「把本机运行的程序用域名访问，不想经 SSH 绕一圈」）
> 一句话：**本机 = 一条已经连上的隧道**。转发怎么建、域名怎么写、hosts 怎么同步，全都不变；只把「SSH 那一跳」换成「直接连本机端口」。
>
> 落地记录见 `docs/DONE.md` 2026-11-18。用户明确要求**入口侧不加任何特殊校验**——本机转发与远端
> 转发走同一套校验，只是出口不同；因此设计里没有"目标只能回环""入口不得与目标同址"这类规则。

## 目标

本机在跑的程序（`127.0.0.1:3000` 之类）也能像远端服务一样出现在 Threadline 里，拿到 `http(s)://app.zly.m` 的域名入口，不新增任何 SSH 连接。

## 原则

| 原则 | 说明 |
|---|---|
| 入口零特殊逻辑 | 域名校验、回环 IP 池、hosts 托管、入口三格（直通 / 本地终止 / 透传）、端口唯一性、流量统计、分组与应用页、开机自启——全部沿用现有实现，本机不另立规则 |
| 只换传输这一跳 | 现有链路是「accept → (可选 TLS 终止) → `direct-tcpip`」；本机只把最后一步换成 `TcpStream::connect` |
| 本机是内置条目 | 恒为**已连接**、恒为启用；不可新建、改名、删除、断开——它没有会话可断 |
| 状态口径不变 | 本机转发与远端转发一样有 `stopped / connecting / running / error`；单次连接连不上目标只记 `last_error`，不改转发状态（本机程序重启不该让整条转发变红） |
| 仅 Windows | 本机条目只在 Windows 构建里注入（core 本身就是 Windows-only，用 `cfg(windows)` 表达这一约束，不写跨平台采集） |
| 不做服务器管理 | 「本机」没有 SSH 会话，概览 / 容器那些模块不适用；卡片上的「管理」按钮对本机不显示 |

## 契约

```
ServerDto 新增字段（默认 ssh，旧数据零迁移）
  kind: "ssh" | "local"

内置条目（不落盘，由 ServerStore 在 list() 时注入，排在最前）
  id = "local"、name = "本机"、kind = "local"、enabled = true
  host = "127.0.0.1"、port = 0、credential_id = ""、keepalive_secs = 0
  needs_credential = false、session_error = None

GET /api/v1/servers/{id}/session  →  local 恒 { connected: true }
PATCH / DELETE / 启用切换            →  local 明确拒绝（"本机是内置条目"）
```

`tunnels.json` 里的本机转发与普通转发同构：`server_id = "local"`、`remote_host` / `remote_port` 就是本机程序地址。**不新增字段、不写迁移。**

## 分层归属

| 位置 | 改动 |
|---|---|
| `tl-protocol` | `ServerKind` 枚举 + `ServerDto.kind`（serde 默认 `ssh`） |
| `models/server` | `list()` 注入本机条目（仅 `cfg(windows)`）；`exists("local") = true`；`kind_of()` 回答传输类型；对 local 的写操作一律拒绝；`decorate` 不给它打「待填凭据」 |
| `models/tunnel/engine` | `Upstream` 枚举（`Ssh(SessionHandle)` / `Direct`），`serve_listener` 接收它：`keepalive` 心跳只在 `Ssh` 时跑；每条连接 `forward_one` 按 `Upstream` 分支——`Direct` 用 `TcpStream::connect`（同一 10s 预算），之后共用现有 `pipe_dir` 与 HTTP Host 改写 |
| `business/tunnel_runner` | `start()` 按 `kind` 分支：local 跳过 `connect_params` / `ensure_session` / 启用检查；`startup_autostart` 与 keeper 跳过 local；`session_connected("local") = true`；`disconnect_session("local")` 拒绝 |
| `src/models/servers.ts`、`App.vue`、`HomeView.vue` | 本机条目置顶显示，不给连接开关 / 断开 / 速率 / 错误；转发卡片标「本机」；「管理」按钮对本机隐藏 |
| 新建转发表单 | 服务器下拉自然多一项「本机」；选中它时远端地址预填 `127.0.0.1`（**只是预填，不是校验**） |

## 边界

- **不做**端口自动发现：那是「本机管理」的另一种形态，用户明确说暂不需要。
- **不做** Windows 指标采集（CPU / 进程 / 服务）：同上。
- **不加**「目标只能是回环」「入口不得与目标相同」这类校验：用户明确说不要入口侧特殊逻辑；真撞上端口就由现有的绑定失败提示。
- **不改**转发模型字段、不改 `tunnels.json` 结构、不写数据迁移。
- **不改**分享包语义：导出跳过内置条目，挂了本机的转发照常导出（`server_id = local`，导入侧同样解析到本机）。

## 验收

1. 新建「本机」下的转发：域名 `app.zly.m` → `127.0.0.1:3000`；开启后 `http://app.zly.m` 返回本机程序内容，hosts 托管区块与代理例外照旧生效。
2. 停掉本机程序 → 浏览器连接失败、转发状态仍为 `running`，`last_error` 给出「无法连接 127.0.0.1:3000」；程序恢复后无需重开转发即可访问。
3. 本机服务的 https 入口（本地 CA 终止回源 http）行为与远端一致，Host 头改写生效。
4. 本机条目：不可改名、不可删除、不可关闭；界面不出现「管理」按钮；`servers.json` 里没有它。
5. 核心重启后，`auto_start` 的本机转发自动恢复监听。
6. 本机转发与远端转发可以同时使用同名域名（现有域名共享语义不变）。
7. 门禁 `scripts/verify.sh` 全绿（含 msocheck：新增代码仍在允许层与依赖方向内）。

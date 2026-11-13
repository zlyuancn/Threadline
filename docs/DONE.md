# DONE（已完成台账）

> 只增不改。收口时从 `TODO.md` 整条搬入，带完成日期、门禁结果与详情指针。

## 2026-11-13

### 新增：配置导入导出（`.threadline` 包）

**需求**：把一组「隧道 + 转发」分享给团队。只导结构，不含密码/私钥；两级勾选；导入前逐条预览
新建/跳过/覆盖；图标作为文件随包；主题与快捷键可选导出。设计与计划见
`docs/plans/2026-11-13-profile-import-export-design.md`、`...-plan.md`。

**改法**：
- Core：`ServerDto` 增加派生字段 `needs_credential`（`serde(skip)`，列表/创建/编辑/启停四个返回点统一重算）；
  允许空凭据的连接存在，但拒绝启用与连接，并把「还没填密码」翻成不可重试结局（不进 keeper 退避循环）；
  新增 `TunnelStore::config_conflict`（配置级端口/域名冲突，与 create/patch 同一套规则）；
  连接 id 按现有集合去重（中文名 slug 相同不再撞 id）；新增 `POST /profile/plan` 与 `POST /profile/import`。
- UI：零依赖 ZIP 读写（`src/models/zip.ts`，写 stored、读 stored+deflate，带条目数与总量上限）；
  `profile.ts` 负责勾选收集、打包、图标校验（128KB）与路径过滤；`preferences.ts` 负责主题/快捷键差异与写回；
  设置页「隧道顺序」改「隧道管理」，新增导出/导入两个弹窗；侧栏与启停提示补「待填密码」。
- 覆盖语义：只改名称/主机/端口/用户名/认证方式/保活，**不动 `credential_id` 与 `host_key_fingerprint`**。

**验证**：Rust `cargo test -p tl-core` 164 项、`cargo test -p tl-protocol` 10 项、clippy 通过；
前端 `vitest` 70 项（新增 zip 5 / profile 13 / preferences 7）、`vue-tsc --noEmit`、`vite build`、
`msocheck` 通过。TESTPLAN 增 U-16 / U-17。导出的包另用 **Windows 自带 `Expand-Archive`** 打开验证过
（解出 `profile.json` / `README.txt` / `icons/*.png`），确认是标准 zip 而非"只有自己读得通"。

**端到端（真实核心 + HTTP，2026-11-13）**：临时数据目录起 `tl-core --manual`，依次打
`profile/plan → profile/import → /servers → /servers/{id}/enabled → 再次 plan/import → 覆盖导入`。
抓出并修掉两个单测覆盖不到的缺陷：

1. `needs_credential` 原本用 `#[serde(skip)]`，它**连序列化一起跳过**——`/servers` 响应里根本没有
   这个字段，界面永远看不到「待填密码」。改为 `#[serde(default)]`：落盘冗余一份，读回由 `decorate`
   按 `credential_id` 重算覆盖（`crates/tl-protocol` 的测试改为断言字段**必须出现**在 JSON 里）。
2. 覆盖导入只增加 `created_forwards`、不加 `created_servers`，界面会显示「写入 0 条连接」，看起来像失败。
   统一为「写入 = 新建 + 覆盖」计数，同时去掉导入面板里无意义的「导入后保持停用」开关
   （包里没有密码，启用必然 409，开关只会制造"关掉就该自动连"的误解）。

实测确认（超出单测范围的行为）：拒绝启用返回 `409 credential_missing` 且带中文原因；重复导入落到
`failed` 而不是写出脏数据；覆盖导入**不改** `credential_id`、也**不用**文件里的 `SHA256:evil` 顶掉
已信任的 `SHA256:known`；转发的 `local_port` 与 `auto_start` 按文件更新。

### 修复：同一条转发在首页卡片与应用磁贴上的默认图标不一致

**用户报**：`mypc远程桌面`（RDP，本机 3389）在首页「端口转发」卡片上是 TCP 的「两条线」图标，
在「应用」页磁贴上却是显示器图标。

**根因**：协议图标写了两份。`ForwardTile` 有完整的 `Record<LocalScheme, string>`（http/https/ssh/rdp/tcp），
`ForwardCard` 只有 `web ? 地球 : TCP` 两种兜底——RDP/SSH 一律落成 TCP 图形。

**改法**：
- `src/models/forwardIcon.ts` 导出唯一的 `PROTOCOL_GLYPHS` / `protocolGlyph(scheme)`，卡片与磁贴同源取图。
- `ForwardCard` 用 `schemeOf(view)` 决定图标并加 `is-<scheme>` 配色类；`styles.css` 补齐 ssh 青 / rdp 橙 /
  tcp 中性（web 仍是薄荷），与磁贴同一套语义。
- 单测 +3：五种协议齐全、rdp/ssh 不与 tcp 共用图形、http 与 https 同图。

**验证**：`vitest` 45 项通过（`forwardIcon` 6 项）、`vue-tsc --noEmit`、`vite build`、`msocheck` 通过；
Rust 侧无改动故未重跑。

## 2026-10-20

### 修复：「启动服务」后隧道显示未连接，其实是**查询失败被当成了没连上**

**用户补充的复现条件**：问题出现在「停止服务 → 启动服务」之后。

**根因（比"缺提示"更根本）**：
- `servers.refresh()` 里写的是 `sessionViews[i]?.connected ?? false`——
  会话查询**失败**（核心刚重启、token 已轮换）被折叠成 `false`，界面于是断言"未连接"。
  实际隧道可能已经连上了，只是这一次没问到。
- HTTP 客户端对 401 没有任何反应：核心每次启动都会轮换 token，之后所有请求继续 401，
  界面只能靠 F5 恢复。

**改法**：
- 会话状态改成**三态**（`connected` + `known`）：查到才更新，查不到保留上一次；
  从未查到过则显示「状态未知」，而不是"未连接"。
  合并逻辑抽到 `src/models/sessionState.ts`，vitest 覆盖 4 条（含"失败不能写成 false"）。
- HTTP 客户端遇 401：重新注入 bootstrap（token）后**重试一次**，不再整页卡死。
- 隧道行新增「状态未知」标签（悬停说明"核心可能正在启动或不可达"）。

**验证**：前端 16 项测试通过（新增 4 项）；`pnpm build` 通过；门禁全绿。

- 门禁：cargo test 147 项 + agent 4 项 + frontend 16 项 + msocheck 44 项 + clippy + fmt + 构建全绿。

## 2026-10-20

### 修复：隧道「未连接」时没有任何原因

**用户报**：隧道行只显示「未连接」，开关是开的，但看不出为什么没连上（端口行反而有原因）。

**根因**：`session_error` 这个概念根本不存在。端口行的原因是 `last_error`（转发层），
隧道会话建立失败只在 `ensure_session` 里被翻成 `Err` 返回给调用方，**从未落到任何可读的地方**。

**改法**：
- `ServerDto` 增加 `session_error`（`skip_serializing_if = "Option::is_none"`，运行时字段不落盘）。
- `ServerStore` 增加内存表 `session_errors`：`set_session_error` / `clear_session_error`，
  `list()` 时填进 DTO —— 与隧道 `last_error` 同一套思路。
- `TunnelRunner::ensure_session`：失败时记 `engine::describe(&outcome)`；
  连上后**清除**，避免挂着一个过期的失败理由。
- 界面：隧道行「未连接」下面就地显示原因，悬停看全文。

**验证**（`scripts/e2e-session-error.py`，指向必然连不上的 `127.0.0.1:1`）：
```
session_error: 端口 1 没有回应 SSH 握手（对方不是 SSH 服务？），详情：由于目标计算机积极拒绝，无法连接。 (os error 10061)
SESSION ERROR: GREEN
```

- 测试：新增 1 条（错误随列表带出、连上后清除）。
- 门禁：cargo test 147 项 + agent 4 项 + frontend 12 项 + msocheck 44 项 + clippy + fmt + 构建全绿。

## 2026-10-20

### 重构：三进程 → 两进程（Wake 并入 Agent）

**动机**：`tl.exe`（Wake）不独占任何资源，全部工作就是"探一下核心、必要时起个进程"，
属于可以内联的启动编排。多一个二进制的成本是实际的：用户要记住双击哪个、打包多一份、文档多一节。

**做法**：`crates/tl-wake` 保留但**不再产出二进制**，只提供 `ensure_core(data_dir)`；
助手在两个时机调用它——启动时（后台线程，不阻塞窗口）与「启动服务」按钮。

**为什么不合并另外两个方向**：
- Agent 并入 Core：SYSTEM 自启会话里起不了界面，且界面崩溃会带走全部转发。
- Core 并入 Agent：转发挂在带界面的进程里，"零登录会话仍要服务"直接消失。

保留"无头核心 + 有界面客户端"这条边界，比减少进程数更重要。

**连带改动**：
- `externalBin` 只留 `binaries/tl-core`；`bundle-sidecars.ps1` 不再暂存 `tl.exe`。
- 删除 `scripts/smoke-wake.ps1`；`audit-lifecycle.ps1` 改为直接起核心，
  断言契约与 healthz（7 项仍全绿）。
- 移除 `wake_candidates` 及其测试；文档（AGENTS/README）改为两进程表述，并新增
  `docs/plans/2026-10-20-two-process-design.md` 记录取舍。

**验证**：核心未运行时只启动助手 → 8 秒内 `tl-core.exe` 出现，
`agent.log` 记录 `startup: core ensured` 与 `core alive -> true`；
生命周期巡检 `LIFECYCLE AUDIT GREEN`。

**踩坑记录**：给 `audit-lifecycle.ps1` 加中文注释后脚本**静默失效**——
Windows PowerShell 5.1 按 ANSI 读 .ps1，中文注释的尾字节把下一行"吃掉"了
（`Start-Process` 整句被并进注释），表现为"命令执行了但什么都没发生"。
已改回纯 ASCII 注释，并把这条写进 `docs/knowledge/guides/windows-platform-traps.md`。

- 门禁：cargo test 146 项 + agent 4 项 + frontend 12 项 + msocheck 44 项 + clippy + fmt + 构建全绿。

## 2026-10-20

### 修复：核心启动后，已启用的隧道没有自动恢复连接

**用户遇到**：核心重启后，隧道开关是「启用」，但状态「未连接」，端口报错——需要手动点一下才好。

**根因**：启动恢复（`startup_autostart`）对每条启用的隧道**只尝试一次**连接，
失败就算了。而启动那一刻恰好撞上本机安全软件对新二进制的拦截（HTTP 403），
于是一次失败 → 永久停在"启用但未连接"。语义上也不对：
开关表示"我希望它连着"，就该一直尝试。

**改法：会话守护（keeper）**
- 每条启用的隧道挂一个守护任务：会话不在就重连，连上后每 5 秒巡检一次；
  失败按退避重试（上限 30s）。
- **只对网络类失败重试**：指纹待确认 / 指纹不一致 / 认证被拒属于"人没处理"，
  重试一万次也不会变，反而会把服务器惹到封 IP —— 直接停下等人。
  （`wants_retry()` 有单测锁定这条边界。）
- 关掉开关时 `stop_keeper` 主动取消，避免守护把连接又拉回来。
- 确认指纹之后调 `after_host_key_accepted`：把守护和"启用"的端口重新拉起，
  否则用户刚处理完的那次失败已经把守护停掉，界面仍显示未连接。

**验证**（`scripts/e2e-keeper.py`，真实服务器）：
```
--- 模拟核心重启 ---     重启后自动连上: True
--- 模拟会话被断开 ---   断开后 connected: False -> 守护自动重连: True
KEEPER: GREEN
```

- 门禁：cargo test 146 项 + agent 6 项 + frontend 12 项 + msocheck 44 项 + clippy + fmt + 构建全绿。

## 2026-10-20

### 改进：握手失败时区分「对方不是 SSH」与「本机安全软件拦截」

**现象**：面板报「端口 33303 上不是 SSH 服务（收到：HTTP/1.1 403 Forbidden）。请确认主机与端口是否正确」，
但同一时刻用 Python 直连同一目标拿到的是 `SSH-2.0-OpenSSH_10.0`。

**定位结论（多次复现）**：
- 同一时刻、同一目标，`python.exe` 正常、`tl-core.exe` 收到 HTTP 403 → **按进程拦截**，与端口无关。
- **每次重新构建**出新的 `tl-core.exe`，拦截就会重新出现；过一会儿（安全软件重新评估后）同一条探测又变成 `ok`。
  这解释了为什么"时好时坏"：不是网络抖动，而是可执行文件的**哈希变了要重新放行**。

**改法**（不试图绕过安全软件，只把话说清楚）：
- `diagnose_connect_error` 嗅到横幅以 `HTTP/` 开头时，不再说"请确认主机与端口"，
  改为：「这不是 SSH 服务的响应。多半是本机安全软件拦截了这个程序，请把 `<当前 exe 的完整路径>` 加入白名单后重试。」
- 其它两种仍然分别报"不是 SSH 服务"与"没有回应握手"。

**验证**：重建后探测从 403 变为 `ok`（连接、指纹、登录全部通过）；恢复后的 `deva` 与 `HA` 均正常。

- 门禁：cargo test 145 项 + agent 6 项 + frontend 12 项 + msocheck 44 项 + clippy + fmt + 构建全绿。

## 2026-10-20

### 修复：新建的隧道重启后消失（版本号写 1、作废阈值判 2）

**用户遇到**：刚建好的隧道，重启后没了。

**根因（我自己引入的）**：v2 为了"旧数据不迁移"加了启动时留档逻辑
`if schema_version < 2 { 改名备份 }`，但两个 Store 写入时用的常量仍是
`SCHEMA_VERSION = 1`。于是**每次启动都把上一轮刚写的配置当旧数据作废一次**，
表现为"重启即丢配置"。更糟的是备份文件名固定，第二次留档会把第一次的覆盖掉。

**改法**：
- `tl_protocol::SCHEMA_VERSION = 2` **单一定义**；两个 Store 与启动时的作废判断都用它，
  从结构上消除"写 A、判 B"的漂移。
- 备份文件名带时间戳，重复留档不再互相覆盖。
- 新增测试 `written_file_declares_the_current_schema_version`：断言写盘版本等于共享常量且 ≥2。
- 从留档恢复用户配置：`scripts/restore-config.py` 把 `*.bak` 重新盖回（版本号改成 2），
  其中 `HA` 转发原来指向已不存在的 `srv_deva_5796`，重指到现存隧道。

**验证**（`scripts/e2e-persist.py`）：
```
before restart: tunnels=['deva'] forwards=['HA']
after  restart: tunnels=['deva'] forwards=['HA']
PERSIST: GREEN
```

- 门禁：cargo test 145 项 + agent 6 项 + frontend 12 项 + msocheck 44 项 + clippy + fmt + 构建全绿。

## 2026-10-20

### 修复：合并页面时把「确认主机密钥」的入口弄丢了

**用户遇到**：开隧道开关只弹一个 `主机密钥待确认：SHA256:…` 的错误，没有任何地方能确认指纹，
首次连接就此死锁。

**根因**：v2 把两张表合并成一页时，隧道行的「主机密钥」列连同「信任/拒绝/核对本机」一起被删掉了，
但后端仍然坚持"未知指纹必须先确认"——功能还在，入口没了。

**改法**：
- 隧道行状态列改为**三态**：待确认主机密钥（显示指纹 + 信任并记录 / 拒绝 / 核对本机）＞ 已连接 ＞ 未连接。
- 「信任并记录」之后**自动打开隧道**（用户本来就是要连它），并提示恢复了几个端口。
- 「核对本机」用 `~/.ssh/known_hosts` 做参照物，结果分三档：一致（绿）/ 不一致（红，勿信任）/ 无记录或哈希格式（灰）。
- 开关失败且原因是待确认时，不再甩库报错，改为一次提示「指纹已显示在状态列，点信任并记录」。
- 错误码：`待确认` 与 `拒绝了该认证方式` 归为 **409 conflict**（原来落到 500）。

**验证**（`scripts/e2e-trust-flow.py`，真实服务器）：
```
未信任时开隧道 -> 409 conflict「主机密钥待确认：SHA256:…」
GET /hostkey   -> pending_fingerprint 可见
accept         -> 200
再开           -> 走到认证阶段（密码是故意填错的），trusted_fingerprint 已落盘
```

- 门禁：cargo test 144 项 + agent 6 项 + frontend 12 项 + msocheck 44 项 + clippy + fmt + 构建全绿。

## 2026-10-20

### 重构（v2 界面与模型）：取消独立「凭据」、隧道与端口混合展示、深色紧凑主题

用户要求：① 去掉凭据单独管理，创建隧道时一起创建；② 隧道和端口转发混合显示；③ 界面太丑。
确认过的取舍：**旧数据不迁移，直接作废**（留档为 `*.v1.bak`）。

**模型**：
- `CreateServerRequest` 增加 `username` / `kind` / `secret` / `passphrase`；
  `ServerDto` 增加 `username`（列表与表单直接用），`credential_id` 退化为内部引用。
- `Services::create_server` 在创建隧道时**顺带创建登录材料**（DPAPI 加密，名称带内部前缀），
  创建失败会回滚，不留孤儿记录；`patch_server` 在改用户名或换密钥时重建材料并切换，
  旧材料随之清理。
- 界面上不再有「凭据」页/接口调用；`credentials` 仅作为内部存储存在。
- 旧版 `servers.json` / `tunnels.json` 在启动时按 `schema_version < 2` 改名留档，从空白开始。

**界面**：
- 单一页面「端口转发」：树形表格，隧道为父行、其下端口为子行；隧道行带「加端口」。
- 新建隧道表单直接含用户名、认证方式、密码/私钥（私钥可一键读本机），不再有"先建凭据"这一步。
- 深色 + 紧凑主题：深底 (#0d1015)、低饱和强调色、更小行高与字号，
  Element Plus 走 `html.dark` + CSS 变量覆盖（`--el-component-size-small`、表格 padding 等）。

**验证**（`scripts/e2e-inline.py`，真实服务器）：
```
POST /servers { username, kind, secret }（不再有 credential_id） -> 201，返回 username=root
POST /servers/{id}/probe -> stage=auth「服务器拒绝了该认证方式」
   ← 证明登录材料确实由隧道自建并被使用（能连上、走到认证这一步）
旧数据留档：servers.json.v1.bak / tunnels.json.v1.bak
```

- 门禁：cargo test 144 项 + agent 6 项 + frontend 12 项 + msocheck 44 项 + clippy + fmt + 构建全绿。

## 2026-10-20

### 修复：开关开着、状态已停止，界面不给原因

**用户质问**：「状态和开关都不一样，你怎么验证通过的？」

开关=意图、状态=现实，这个两分本身是刻意的；**问题在于现实与意图不符时，界面没有就地解释**：
原因当时只写在「最近错误」列（表格最后，需要横向滚动），而该列内容还可能是**过期错误**
（隧道关掉后，旧错误仍留着），看起来就更像自相矛盾。

- 状态列就地显示原因，并且**优先报当前阻塞点**：
  - 隧道没启用 → 「隧道「deva」未启用」
  - 隧道正常才回落到上次错误（避免拿过期错误当现在的原因）
  - 隧道信息未加载时不瞎猜
- 判定逻辑抽到 `src/models/forwardStatus.ts`，用 vitest 覆盖 5 条
  （运行中不说话 / 未启用优先 / 回落到错误 / 不编造 / 缺数据不崩）。
- 隧道列不再重复标记，原因只有一处，避免两处信息打架又对不上。

**顺带承认的验证盲区**：之前的 `e2e-*.py` 验证的都是**后端契约**
（409、`last_error`、连接数 0），**没有覆盖「界面看着是否自洽」**，
所以"验证通过"与"用户看到的矛盾"可以同时成立。这类 UI 判定现在有了单测。

- 门禁：cargo test 144 项 + agent 6 项 + frontend 12 项 + msocheck 44 项 + clippy + fmt + 构建全绿。

## 2026-10-20

### 修复：隧道未启用，端口转发却照跑（开关变成了空话）

**用户发现**：所有隧道都是「未启用」，端口转发却显示「运行中」而且真的能访问。

**根因**：启动端口转发时**无条件**调用 `ensure_session`，也就是"要跑就顺手建会话"，
完全没有看隧道层的意图。于是隧道开关停在 `false`，会话和监听却已经建好——
开关和现实各说各话。

**改法**（意图必须是硬约束，而不是展示字段）：
- `ConnectParams` 带上 `enabled`；`TunnelRunner::start` 在建会话**之前**检查，
  未启用则直接拒绝：状态保持 `Stopped`，`last_error = 隧道未启用：请先在「隧道」页启用这条隧道`，
  返回 409 `conflict`——**不会偷偷建会话**。
- `set_last_error` 改为对"从未启动过"的条目也生效（`or_insert_with(Runtime::new)`），
  否则这种拒绝在界面上毫无痕迹（用户点了启动什么也看不到）。
- `ServerStore::patch` 不再处理 `enabled`（纯配置路径），改由 `set_enabled` +
  业务层编排；`PATCH /servers/{id}` 里若带 `enabled`，接口会转交
  `set_server_enabled`，保证"改开关 = 连带建立/断开会话"。
- 界面：端口转发表在「隧道」列显示「隧道未启用」标记，省得用户以为是自己填错了。

**验证**（`scripts/e2e-gate.py`，真实服务器）：
```
隧道未启用时启动端口 -> 409 conflict，state=stopped，
                        last_error=隧道未启用：请先在「隧道」页启用这条隧道
                        本核心到服务器 ESTABLISHED = 0（没有偷偷建会话）
打开隧道后            -> state=running，ESTABLISHED = 1
GATE: GREEN
```

- 测试：新增 1 条（未启动过的条目也要能看到拒绝原因）。
- 门禁：cargo test 144 项 + agent 6 项 + frontend 7 项 + msocheck 44 项 + clippy + fmt + 构建全绿。

## 2026-10-20

### 改进：隧道也有「启用」开关（两级意图：隧道=保持会话，端口=保持监听）

**用户提出**：既然隧道能复用，隧道本身就该有开关表示是否启用；端口那层仍然各自有开关。

**语义（与端口转发同一套）**：
- 隧道「启用」= 用户希望这条 SSH 会话保持连接；**持久化**，连不上不会把开关弹回去。
- 端口「启用」= 用户希望这个本地端口保持监听；同样持久化。
- 关闭隧道 → 会话断开、挂在上面的端口全部停止，但**各端口的启用意图保留**；
  重新打开隧道 → 会话恢复，并自动拉起该隧道下所有"启用"的端口。
- 隧道未启用时，挂在上面的转发不会自动启动，`last_error` 写明「隧道未启用：这条转发不会自动启动」。

**改动**：
- 契约：`ServerDto.enabled`（`#[serde(default)]`，老文件读回 false，升级不会变成"全部自动连"）、
  `CreateServerRequest.enabled`、`PatchServerRequest.enabled`；
  新增 `POST /servers/{id}/enabled`（返回 `session_connected` + `affected_forwards`）。
- 编排：`Services::set_server_enabled` = 落盘意图 → 开：建会话 + 恢复启用的端口；关：断会话（连带停端口）。
- 启动恢复：`startup_autostart` 先连启用的隧道，再拉起启用的端口；隧道没启用的端口跳过并记原因。
- 界面：隧道表新增「启用」开关；「会话」列退化为纯状态（已连接/未连接），
  原先那个单独的「断开」按钮删掉——一个控件一种含义，避免"开关说开着、按钮说断开"。

**验证**（`scripts/e2e-tunnel-switch.py`，真实服务器）：
```
打开隧道 -> {session_connected: true, affected_forwards: 3}，3 条转发 running，连接数 1
关闭隧道 -> {session_connected: false, affected_forwards: 3}，全部 stopped，连接数 0
          各转发 auto_start 仍为 true（意图保留），隧道 enabled=false
TUNNEL SWITCH: GREEN
```

- 测试：新增 2 条（启用落盘/patch、老文件缺 `enabled` 反序列化为 false）。
- 门禁：cargo test 143 项 + agent 6 项 + frontend 7 项 + msocheck 44 项 + clippy + fmt + 构建全绿。

## 2026-10-20

### 重构：一条隧道只建一条 SSH 会话，多条端口转发共用

**用户提出**：多条转发应共享同一条 SSH 隧道；关掉隧道，挂在上面的端口全部关；
隧道连着的时候，随时可以加端口。

**原来**：`engine::serve` 把"连 SSH"和"监听本地端口"绑在一起，**每条转发各自登录一次**。
N 条转发 = N 条 SSH 连接，既浪费，也容易触发服务器/云防火墙的连接数限制。

**改法**：
- `engine` 拆两层：`open_session`（建会话/握手/指纹/认证）与 `serve_listener`
  （在已有会话上 accept → `direct-tcpip`），新增 `SessionHandle` 别名。
- `TunnelRunner` 增加 `sessions: server_id → Session{handle, cancel}`；
  `ensure_session` 按需建立、后续复用。建会话用 `session_lock` 串行化——
  两条转发同时启动时若不加锁会各建一条，留下孤儿会话（实测踩到过）。
- `disconnect_session(server_id)`：停该隧道下所有转发 → 取消监听 → 主动 `disconnect`。
  监听循环在重连返回后也检查取消标记，避免"断开后又偷偷建一条"。
- 新增 API：`GET /servers/{id}/session`（`connected` + `attached_forwards`）、
  `POST /servers/{id}/session/disconnect`（返回顺带停掉的转发数）。
- 界面：隧道表新增「会话」列（已连接/未连接）+「断开」，提示连带停掉的转发数。
- **顺带修**：握手失败改用与探测同一套诊断（嗅横幅），不再把 rusc 的
  `invalid SSH version string` 直接抛给用户，而是"端口 X 上不是 SSH 服务（收到：……）"。

**验证**（`scripts/e2e-shared-session.py`，真实服务器）：
```
两条转发启动后，本核心到服务器的 ESTABLISHED 连接数 = 1
session: {connected: true, attached_forwards: 3}
disconnect -> {stopped_forwards: 3}，两条转发均 stopped，连接数回到 0
SHARED SESSION: GREEN
```
（脚本最初把用户自己开着的 `ssh.exe` 也统计进去，已改为只统计 `tl-core` 进程的连接。）

- 门禁：cargo test 141 项 + agent 6 项 + frontend 7 项 + msocheck 44 项 + clippy + fmt + 构建全绿。

## 2026-10-20

### 重构：「启用」开关只表示用户意图；转发失败必须报错

- **语义拆分**（用户提出的问题）：之前一个开关同时承载"用户想要它开着"和"当前是否连上"，
  导致连不上时用户不知道开关到底代表什么。
  - 开关 = **意图**，持久化（`auto_start`）；打开即连接、关闭即断开；
  - **启动失败不回滚意图**——连不上是网络/目标的问题，不该篡改"我想要它开着"；
  - 状态列才是现实（运行中/重连中/错误），核心重启时按意图自动恢复（已验证：重启后 `homeassistant` 自动回到 running）。
- **失败必须可见**：
  - `let _ = forward_one(...)` 把单次转发错误吞掉了 → 现在通过 `on_error` 回调写进
    `last_error`（**不改状态**：会话还在，只是那个目标连不上），界面「最近错误」列直接显示。
  - 打开通道原来没有超时：目标把包丢掉时会**永久挂起**，用户只感觉"连上了没反应"。
    加 `CHANNEL_OPEN_TIMEOUT = 10s`，超时报「服务器在 10 秒内没有确认通道（目标不可达或被防火墙丢弃）」。
- 验证（对真实服务器）：
  - 目标端口明确拒绝 → `last_error = 无法连接 127.0.0.1:1（服务器侧）：Failed to open channel (ConnectFailed)`，状态仍为 running；
  - 目标被丢包（192.168.28.98:16100）→ 10 秒后按超时报错，而不是无限挂起。
- 门禁：cargo test 154 项 + frontend 7 + msocheck 44 + clippy + fmt + 构建全绿。

### 修复：信任指纹后点「启动」无效（两个真 bug）+ 首次跑通真实转发

**用户现象**：点启动后状态仍是旧的；信任指纹后再点启动报 `非法状态迁移：NeedsTrust + Start`。

- **bug 1（状态机）**：`NeedsTrust` 只允许 `TrustAccepted/TrustRejected/TrustTimeout/Stop` 离开，
  **不允许 `Start`**。而用户的真实流程是「首连 → 待确认 → 信任并记录 → 再点启动」，
  最后一步必然 500。补 `(NeedsTrust, Start) → Connecting`，并加测试锁定这条路径。
- **bug 2（任务登记泄漏）**：`TunnelRunner::start` 开头「已在 tasks 里就直接返回」，
  但任务自己结束（待确认 / 认证失败 / 重试耗尽）时**从不摘除登记**。
  于是第一次启动卡在 `NeedsTrust` 之后，再点启动全部变成空操作——"点了没反应"就是它。
  改为在 spawn 的任务尾部自行 `tasks.remove(id)`。
- **首次跑通真实转发**（此前全部是"未验证"）：
  - 建转发 → 启动 → `running`；从本机连该端口并发出 `GET / HTTP/1.0`，
    经 SSH 通道到服务器出网访问 `1.1.1.1:80`，收到 **381 字节 `HTTP/1.1 301 Moved Permanently`**。
  - 这验证了 `bind → accept → channel_open_direct_tcpip → copy_bidirectional` 整条链路。

### 诊断：`HTTP/1.1 403 Forbidden` 不是配置错误，是**本机安全策略按路径拦截**

- 现象：面板"测试连接"报 `端口 33303 上不是 SSH 服务（收到：HTTP/1.1 403 Forbidden）`，
  但同一台机器上 `ssh -p 33303` 能正常登录。
- 实测矩阵（同一时刻、同一目标 `1.14.74.129:33303`）：

  | 客户端 | 结果 |
  |---|---|
  | `ssh.exe`（用户终端） | 登录成功 |
  | PowerShell / Python / Node 直连 | `SSH-2.0-OpenSSH_10.0` |
  | `tl-core.exe` 从 `target\debug\` | HTTP 403（网关拒绝页，含 `code:190001`） |
  | 同一份 `tl-core.exe` 从 `%TEMP%` | `SSH-2.0-OpenSSH_10.0`，探测 `stage=ok` |
  | 安装版从 `%LOCALAPPDATA%\Threadline\` | HTTP 403 |

- 结论：**可执行文件的启动位置决定是否被拦截**，与代码、与目标服务器无关。
  这是本机安全客户端（环境里有 iOA / SmartVPN 等）按路径放行/拦截的结果。
- 处置建议：把安装目录加入安全软件白名单；开发期从 `%TEMP%` 启动可复现"未被拦截"的行为。
- 待办：`channel open` 失败（远端目标不可达）目前只表现为连接被重置，应在状态里给出原因。

### 修复：端口转发地址过长时溢出到行外

- **现象**：`本地 → 远端` 里的长地址在 `<code>` 里换行，但行高没跟着长，第二行画到了行框外面（与下一行重叠）。
- **改法**：不再依赖自动换行。该列改为**两行固定布局**（`本地 …` / `远端 …`），每行 `white-space: nowrap` + 省略号，
  悬停 `title` 看完整值。行高因此是确定的，不会被内容撑破。
- 顺带把隧道页的「地址」列也改成同样的单行省略（`用户@主机:端口` 同样可能很长）。
- 门禁：cargo test 145 项 + agent 6 项 + frontend 7 项 + msocheck 44 项 + clippy + fmt + 构建全绿。

### 改进：非 SSH 端口给出可读原因；端口转发开关改为「启用/关闭」

- **用户反馈的 `invalid SSH version string`**：那不是说配置有错，而是**那个端口上跑的不是 SSH**。
  原来的报错直接抛 russh 的库信息，没人看得懂。现在分两种情况给结论：
  - 对方主动发了非 SSH 横幅（SMTP 之类）→ 「端口 X 上不是 SSH 服务（收到：220 smtp...）」
  - 对方**静默**（HTTP 这类"客户端先开口"的协议）→ 「端口 X 没有回应 SSH 握手（对方不是 SSH 服务？），详情：...」
  - 实现：先单独开一条连接嗅横幅（读完即关，不影响随后的 russh 握手），静默与否决定错误措辞。
- **测试连接后直接把指纹变成「待确认」**：探测走到握手阶段就 `observe_host_key`（只观测、不信任），
  列表里立刻能"信任并记录"，不必先启动一条转发。
- **端口转发开关改名**：`随服务启动` → 列头「启用」、开关内文字「启用/关闭」，
  对话框提示改为「启用后服务启动时自动连上；关闭则只能手动启动」。
- 验证：新增 2 条单测（外来横幅 / 静默），并写了 `scripts/probe-nonssh.py` —— 起一个真 HTTP 服务器让探测去打，
  输出 `端口 57649 没有回应 SSH 握手（对方不是 SSH 服务？）`，脚本判绿。
- 门禁：cargo test 145 项 + agent 6 项 + frontend 7 项 + msocheck 44 项 + clippy + fmt + 构建全绿。

### 改进：隧道支持「测试连接」；端口转发的自启项改名

- **「开机自启」改名「随服务启动」**：端口转发行上的这个开关从来不是"开机自启"，
  而是"核心启动时自动拉起这条转发"。真正的开机自启在设置页（任务计划），两者层级不同。
  列头与对话框标签一起改，避免继续误导。
- **新增 `POST /servers/{id}/probe`**：真的连一次，回答"这条配置到底通不通"，分阶段返回：
  - `tcp` 地址/端口错、没开服务、防火墙
  - `ssh` TCP 通但不是 SSH 服务 / 握手失败
  - `fingerprint` 主机密钥与记录不一致（探测也不允许悄悄连上去）
  - `auth` 认证被拒（用户名/密钥/密码错）
  - `ok` 全通（未信任指纹时会提示"保存指纹后才能正式启用"）
  - 实现：`engine::probe` 做 TCP → 握手 → 指纹比对 → 认证，**不绑端口、不转发**；
    `Services::probe_server` 组装参数后调用；界面在隧道行加「测试连接」按钮并展示阶段
- 验证：`probe-patch.ps1` 新增一条 —— 对已关闭端口 `127.0.0.1:1` 探测，返回 `tcp` 阶段并带真实 OS 错误。
- 门禁：cargo test 143 项 + agent 6 项 + frontend 7 项 + msocheck 44 项 + clippy + fmt + 构建全绿。

### 改进：隧道 / 端口转发 / 凭据 都能编辑了（并修掉两个后端缺陷）

- **界面**：三类实体都加了「编辑」入口。对话框预填现值、标题区分新建/编辑；
  凭据编辑时锁定类型与登录用户（属身份，改了会影响已有隧道），密钥留空表示不修改。
- **后端补强**（写探针时才发现）：
  1. `PatchCredentialRequest` 原先只有 `secret`，无法只改名 → 改为 `{ name?, secret?, passphrase? }`；
     `secret` 省略即不动密钥材料，空 patch 与空名都报错。
  2. **错误码分类错**：`不能为空`、`必须` 被判成 **409**（资源冲突），等于把"用户填错参数"报成"引用冲突" → 改判 **400**。
  3. **响应形状不一致**：`POST /tunnels` 返回扁平 `TunnelStatus`，`GET /tunnels` 却返回 `{definition, status}`。
     新增 `TunnelModel::view()`，创建/修改/启停统一返回 `TunnelView`。
- **新增 `scripts/probe-patch.ps1`**：对活核心跑 9 条断言（三类实体改名/改端口、400 校验、可重复运行的自清理），
  当前全绿——"接口存在"不再等于"接口能用"。
- 门禁：cargo test 141 项 + agent 6 项 + frontend 7 项 + msocheck 44 项 + clippy + fmt + 构建全绿。

### 修复：界面还是旧版 —— 启动的是冒烟脚本留下的陈旧 agent 副本

- **现象**：界面仍是「隧道 / 服务器与凭据」的旧文案、离线时仍显示「停止服务」、横幅是旧的报错模板。
- **根因**：`smoke-wake.ps1` 为了满足"三者同目录"，把 agent **拷贝**了一份到 `target\debug\tl-agent.exe`。
  那份副本再也不会更新，而用户启动的正是它——与之前 sidecar 陈旧是同一类错误。
- **修复**：
  - Wake 自己解析真实构建（安装态找同目录，开发态找 `<项目根>/src-tauri/target/debug/tl-agent.exe`），**不再需要拷贝**。
  - 冒烟脚本删掉拷贝逻辑；陈旧副本已删除。
  - 门禁新增硬检查：`target\debug\tl-agent.exe` 存在即失败，杜绝再次出现。
- 验证：`tl.exe` 启动后进程路径为 `src-tauri\target\debug\tl-agent.exe`，`agent.log` 记录
  `core alive -> true (menu: start=false, stop=true, bootstrap_injected=true)`。

### 修复：托盘「退出助手」不关核心；状态变化时不刷新 token；新增生命周期巡检

- **退出留孤儿**：`退出助手` 原先只 `app.exit(0)`，核心继续在后台跑，且再没有托盘可以停它。
  现在先 `POST /shutdown`（失败也照样退出，但会记日志）→ 等 400ms 让核心落盘 `stopped` → 退出。
- **token 轮换后 401**：核心每次启动都换 token，但前端只在「没有 token」时才重新注入。
  核心重启后旧 token 仍在窗口里，表现为「起来了但一直连不上」。现在**状态切换时**
  （探活发现 alive 变化）就重新注入，`agent.log` 里能看到 `bootstrap_injected=true/false`。
- **新增 `scripts/audit-lifecycle.ps1`**：把外部可驱动的状态转换全部跑一遍并断言不变量——
  冷启动、健康校验、二次唤醒复用、优雅停止、重启换身份、崩溃后契约陈旧但探活失败。
  当前 7 项全绿。定位方式从"用户发现"改成"改完自己跑"。

### 改进：主机密钥确认给出可用参照物（本机 known_hosts 比对）

- **问题**：让用户确认 `SHA256:xxx` 却不给参照物，等于走过场——普通用户没有第二次计算的途径。
- **改法**：新增 Tauri 命令 `known_hosts_fingerprint(host, port)`，读 `~/.ssh/known_hosts`，
  解析该主机（含 `[host]:port` 非默认端口写法）的 key，按 OpenSSH 规则算 `SHA256:base64` 指纹。
  「待确认」状态下点「核对本机记录」得到三态结论：
  - ✓ 与本机记录一致（可以直接信任）
  - ✗ 与本机记录不一致（红色警告，请勿信任）
  - 本机没有记录 / 记录是 `|1|` 哈希格式（无法比对）
- 界面同时给出两条取信途径：本机 known_hosts（终端 ssh 过一次即有）、服务器上 `ssh-keygen -lf`。
- **立场**：仍然不做隐式 TOFU，但把「确认」从空口要求变成可操作的比对。
- 门禁：cargo test 141 + frontend 7 + msocheck 44 + clippy + fmt + 构建全绿。

### 修复：主机密钥列把「未连接」误显示成「未信任」

- **问题**：新建的隧道从没连过，界面照样显示「未信任」并给出「信任并记录 / 拒绝」两个按钮。点下去必然失败——核心那边根本没有待确认的指纹（`accept_host_key` 会报「没有待确认的主机密钥」）。这是死按钮。
- **改法**：按三态显示，数据来自 `GET /servers/{id}/hostkey`（含内存态的 `pending_fingerprint`）：
  - **已信任** —— 显示指纹前 24 字符
  - **待确认** —— 显示待确认指纹 + 「信任并记录 / 拒绝」（此时才有意义）
  - **未连接** —— 纯提示「启动一条端口转发后才会观测到指纹」，不给按钮
- 指纹在首次握手后被观测到（进 `NeedsTrust`），确认后才落盘；不一致则直接进错误态、绝不自动接受。
- 门禁：cargo test 141 + frontend 7 + msocheck 44 + clippy + fmt + 构建全绿。

### 修复：点「刷新」仍闪黑窗 —— 跑的是旧 sidecar

- **现象**：`/whoami` 会闪一个控制台窗口（面板刷新按钮和三秒轮询都会碰到）。
- **定位过程**：
  - 延迟测量：`/healthz` 69ms、`/whoami` **243ms**、`/autostart` **234ms**、`/tunnels` 1.8ms —— 证明 `whoami` 确实在起 `schtasks.exe`。
  - 文件时间戳比对：`target\debug\tl-core.exe` = 10:00，而 `src-tauri\binaries\*.exe` 与运行中的 `src-tauri\target\debug\tl-core.exe` = **09:46**。
- **根因**：`CREATE_NO_WINDOW` 的修复在源码里，但**打包用的 sidecar 是旧副本**。我只重建了 `target\debug`，没有重新暂存到 `src-tauri\binaries`，Tauri 打包时照抄旧文件，于是跑的一直是没有该标志的旧核心。
- **修复**：
  - `scripts/bundle-sidecars.ps1` 重写：自动挑最新的 profile 目录，并打印每个 sidecar 的构建时间戳（以后一眼能看出是不是旧的）。
  - `tauri.conf.json` 增加 `bundle.beforeBuildCommand`，**每次打包前强制重新暂存**，这类"改了代码但发的是旧二进制"的问题不会再发生。
- **验证**：重建并重启后做 A/B 对照（各 6 秒、30ms 采样 conhost）：空闲 27 个、连续 `whoami` 期间 26 个 —— 没有新增控制台窗口。
- **顺带修复**：门禁里的 `cargo test (agent)` 在应用运行时必然失败（`tauri-build` 要覆盖被运行中 `tl-core.exe` 占用的 sidecar）。现在检测到应用在跑时明确跳过并提示，而不是报红。
- 门禁：cargo test 141 + frontend 7 + msocheck 44 + clippy + fmt + 构建全绿（agent 测试因应用运行中按设计跳过）。

### 改进：凭据支持自定义名称

- `CredentialSummary` / `CredentialItem` / `CreateCredentialRequest` 增加 `name`；
  旧文件缺少该字段时 `#[serde(default)]` 兜底，不需要迁移代码。
- 服务端在 `name` 为空时用 `username` 填充——列表里出现空白项没有意义。
- 界面：凭据表新增「名称」列（空则回退用户名）；新建对话框新增「名称」输入，占位提示「生产机 root」；
  隧道的凭据下拉框显示 `名称（用户名 · 类型）`，同名用户不再混淆。
- 按 TDD 补 3 条测试：名称持久化并出现在摘要、空名回退用户名、老条目反序列化。
- 门禁：cargo test 141 项 + agent 2 + frontend 7 + msocheck 44 + clippy + fmt + 构建全绿。

### 改进：凭据可一键导入本机私钥；登录用户改为「凭据唯一提供」

- **一键导入**：凭据对话框在「私钥」类型下加「填入本机私钥」按钮 → Tauri 命令 `read_local_private_key`
  依次找 `~/.ssh/id_ed25519`、`id_rsa`、`id_ecdsa`，把内容填进文本框并显示来源路径。
  - 立场澄清：这**不是**回到"运行时引用 `~/.ssh` 路径"（那在 SYSTEM 身份下读不到），
    只是把用户本来要手动粘贴的内容帮他填上，随后仍由核心 DPAPI 加密托管。
- **用户名默认 `root`**：新建凭据时预填，可改。
- **去掉隧道上的「登录用户」**：`ServerDto` / `CreateServerRequest` / `PatchServerRequest` 移除 `user` 字段，
  运行时登录用户取自凭据的 `username`（`to_login(material)`），从此只有一处真相。
  - 旧 `servers.json` 里的 `user` 字段会被 serde 忽略，不需要迁移代码。
  - 隧道列表的地址列现在显示「凭据用户名@host:port」。
- 门禁：cargo test 139 + agent 2 + frontend 7 + msocheck 44 + clippy + fmt + 构建全绿。

### 改进：开机自启开关自动提权（不再要求用户手动开管理员终端）

- **问题**：核心以用户身份运行时，改 per-machine 任务计划必然返回 `409 privilege_required`，界面只能提示「请以管理员身份重试」——用户得自己去开管理员终端，体验断掉。
- **改法**：
  1. 核心照旧拒绝（它确实无权，也不该偷偷提权）。
  2. 助手收到 `privilege_required` 后，弹一次确认 → `ShellExecuteW(..., "runas", ...)` 拉起 `tl-core --autostart on|off`（一次性进程：不抢锁、不绑端口）。
  3. 一次性进程把结果写 `autostart.result.json`，助手轮询该文件（上界 120s，覆盖用户慢慢点 UAC 的情况），再 `GET /autostart` 复核。
  4. **不假装成功**：提权执行成功但核心读不到 SYSTEM 任务状态时，如实提示「以重启后的实际行为为准」。
- 备注：`ShellExecuteW` 需要 `Win32_UI_Shell` + `Win32_UI_WindowsAndMessaging` 两个特性（后者因为签名引用 `SHOW_WINDOW_CMD`）。
- 门禁：cargo test 139 + agent 2 + frontend 7 + msocheck 44 + clippy + fmt + 构建全绿。

### 调整：界面术语改为「端口转发 / 隧道 / 凭据」

- 原「隧道 / 服务器 / 凭据」改为「**端口转发**（一条 `-L` 规则）/ **隧道**（一条 SSH 连接）/ **凭据**」。
- 菜单拆成五项：端口转发 · 隧道 · 凭据 · 日志 · 设置（原先「服务器与凭据」合并成一页）。
- 新增 `views/PortForwardsView.vue`（原 `TunnelsView`）、`views/TunnelsView.vue`（原 `ServersView`）、
  `views/CredentialsView.vue`；`ServersView.vue` 删除。
- 内部标识符（`server` / `tunnel`、`/servers`、`/tunnels`、DTO 名）**保持不变**，理由与映射见
  `decisions/0002-terminology.md`。
- README 同步新术语，并修掉其中已废弃的「boot/manual 档位切换」描述（v4 起只有自启开关）。
- 门禁：cargo test 139 + agent 2 + frontend 7 + msocheck 44 + clippy + fmt + 构建全绿。

### 改进：异步按钮统一「转圈 + 防重复 + 超时」

- 新增 `src/models/asyncAction.ts`：`withTimeout` + `useAsyncAction`（`pending` / `error` / `run`），`run` 在进行中再次调用直接返回 `false`，不会发出第二次请求。
- 按 TDD 先写 7 条测试（重复点击只执行一次、失败后恢复可点、超时文案、成功清空错误、超时透传原错误等），`vitest` 接入门禁。
- 接入位置：
  - 顶部「启动/停止服务」：转圈 + 禁用，文案变「正在启动… / 正在停止…」，上界 **30s**
  - 顶部「刷新」：转圈 + 禁用，上界 15s
  - 隧道行内「启动/停止/删除」：按行转圈，同一时刻只允许一个行操作，上界 20s
  - 隧道「创建」、服务器/凭据「删除」「信任/拒绝」：转圈 + 禁用，上界 15–20s
  - 设置页「开机自启」开关：加载中禁用
- HTTP 层也加了兜底：每个请求带 `AbortSignal.timeout(15s)`，核心卡死时不会把界面吊住。
- 门禁新增 `frontend test (vitest)` 一步。
- 门禁：cargo test 139 项 + agent 2 项 + frontend 7 项 + msocheck 44 项 + clippy + fmt + 构建全绿。

### 修正：停止服务后按钮仍是「停止服务」

- **问题**：点「停止服务」后核心已退出，按钮标签不变（仍是"停止服务"），且没有任何路径能把服务重新拉起来——只能去双击 `tl.exe`。
- **改法**：
  - 按钮随连接态翻转：在线 → 「停止服务」（`POST /shutdown`）；离线 → 「启动服务」。
  - 「启动服务」走 Tauri 命令 `start_core`，由它拉起 `tl.exe`（唤醒入口），把"如何确保核心在跑"这件事仍然留给 Wake，助手不复制这套逻辑。
  - 离线横幅从"无法连接本机核心"改成"核心未运行 · 点右上角「启动服务」即可拉起"。
  - 托盘菜单项改为单一开关「启动 / 停止服务」，点击时按实际状态决定方向。
- **连带修复**：核心每次启动都会**轮换 token**，拉起新核心后前端仍持旧 token 会 401。新增 Tauri 命令 `refresh_bootstrap`：启动循环与每次离线轮询前重新读取 `control.json` 注入，因此「外部双击 tl.exe 拉起核心」也能自动恢复。
- 门禁新增 `cargo test (agent)` 一步（`src-tauri` 是独立 workspace，原先不在门禁覆盖内）。
- 门禁：cargo test 139 项 + agent 2 项 + msocheck 44 项 + clippy + fmt + 前端构建全绿。

### 修正：心跳轮询用错了接口

- **问题**：面板每 5 秒拉一次 `/whoami`。但 `/whoami` 是**身份信息**（版本、运行方式、降级标记、自启状态），只在启动/切开关时变；而它内部要查任务计划（起进程）。把"身份"当"心跳"是设计偷懒，也是闪窗的放大器。
- **改法**（CAW C5「核心不推、助手主动探测」仍然遵守，只是探测对象换对）：
  - 心跳只轮询 `/tunnels`（纯内存状态，零进程），5 秒一次；失败即判核心离线，退避到 **15 秒**。
  - `/whoami` 只在**启动、手动刷新、掉线恢复**时取一次。
  - 掉线恢复时会补一次全量（核心可能换了实例，身份信息已变）。
- 验证：采样 20 秒，`schtasks` 进程出现 **0 次**（修复前约 4 次/20 秒）。
- 门禁：cargo test 139 项 + msocheck 44 项 + clippy + fmt + 前端构建全绿。

### 修复：每隔几秒闪一次黑框

- **现象**：面板开着时黑框不停地闪。
- **根因**：面板每 5 秒轮询 `/whoami`，而 `/whoami` 每次都会执行 `schtasks.exe` 查任务计划。核心以 `DETACHED_PROCESS` 运行（自身没有控制台），子进程 `schtasks.exe` 是控制台程序且未指定 `CREATE_NO_WINDOW` → Windows 每次都给它新开一个控制台窗口，闪一下。
- **修复**：`tl-core` 的 `registry::schtasks` 与 `tl-wake` 的 `schtasks` 统一加 `CREATE_NO_WINDOW`（`0x0800_0000`）；`spawn_detached` 已有 `DETACHED_PROCESS`，全仓库再无其它会开控制台的进程。
- 验证：`/whoami` 仍正常返回 `autostart.enabled=false / privilege_required=true`，三进程链路 `tl.exe` 退出码 0。
- 门禁：cargo test 139 项 + msocheck 44 项 + clippy + fmt + 前端构建全绿。

### 修复：双击后出现控制台黑窗

- **现象**：面板旁边挂着一个 `tl-agent.exe` 的命令行黑窗。
- **根因**：`#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]` —— 只有 release 才切 GUI 子系统，而日常双击的正是 debug 构建。
- **修复**：`tl-agent` 改为**无条件** `windows_subsystem = "windows"`；`tl.exe`（Wake）同样处理，双击不再闪黑框。
  - Wake 仍可从终端调用并输出到 stderr（GUI 子系统只表示"不主动创建控制台"，继承的句柄照常可用）。
- 验证：PE OptionalHeader.Subsystem —— `tl.exe` = 2、`tl-agent.exe` = 2（`tl-core.exe` 保持 3，它由 `DETACHED_PROCESS` 拉起，不创建控制台）。

### 修复：陈旧 `control.json` 让 Wake 误判「核心不可达」（退出码 4）

- **现象**：杀掉旧核心后重跑 `tl.exe`，新核心明明起来了，Wake 却报不可达、退出码 4、面板没被拉起。
- **根因**：Wake 先 `read_control` 拿到**上一个核心**留下的契约（同端口、旧 `instance_id`），然后拿这份**一次性快照**去探测；新核心写回自己的契约后快照仍是旧的 → `instance_id` 永远不匹配 → 判 `NotOurs` 直到预算耗尽。
- **修复**：`wait_until_healthy` 改成接受数据目录、**每轮重读** `control.json`；删掉 `wait_for_control`（它正是产生快照的来源）。
- 按 TDD：新增三条测试——陈旧契约会被重读后判定可达、无核心时超时、端口活着但不是我们时如实报 `NotOurs`。
- 验证：保留陈旧 `control.json` 直接跑 `tl.exe` → 退出码 **0**，`tl-core` 与 `tl-agent` 均起来。
- 门禁：cargo test **139 项** + msocheck 44 项 + clippy + fmt + 前端构建全绿。

### 修复：面板报「无法连接本机核心（TypeError: Failed to fetch）」

- **现象**：核心与助手都在跑、`control.json` 正常、`healthz` 直连返回 200，但面板一直显示「核心不可达」。
- **根因**：核心监听 `127.0.0.1:18921`，而 WebView 页面源是 `http://tauri.localhost` —— **跨源**。axum 没发 `Access-Control-Allow-Origin`，浏览器在 JS 层直接拒绝，抛 `TypeError: Failed to fetch`（连响应体都拿不到，看起来像"核心没启动"）。
- **修复**：`interface` 最外层加 CORS 层，允许 `http://tauri.localhost` / `https://tauri.localhost` / `tauri://localhost` / `127.0.0.1:5173` / `localhost:5173`，方法含 `OPTIONS`，头含 `Authorization`、`Content-Type`。
  - 顺序关键：CORS **必须在鉴权中间件外层**，否则预检（不带 token）会被拦成 401，浏览器仍报 CORS 失败。
  - 安全边界未放宽：仍只监听回环、仍要求 Bearer。
- 前端错误信息改为带真实请求地址，并区分「未注入发现契约」与「跨源被拦/不可达」，下次能一眼定位。
- 按 TDD：先写两条测试（预检需 200 + 允许头、普通响应需带允许头）确认失败，再实现。
- 验证：`OPTIONS /api/v1/whoami`（带 `Access-Control-Request-Headers: authorization`）返回 200、`allow-origin=http://tauri.localhost`、`allow-headers=authorization,content-type`；重启后 Agent 面板恢复。
- 门禁：cargo test **138 项** + msocheck 44 项 + clippy + fmt + 前端构建全绿。

## 2026-10-19

### 方法论落地与设计拍板

- 读入三份方法论并按规则定稿项目形态：CAW `41f698a`、多项目工作区 `f78fd6e`、MSO `5c432f9`。
- 产品名 **Threadline / 引线**；工作区 `C:\my\threadline`；项目 `project\threadline`；二进制 `tl` / `tl-core` / `tl-agent`。
- 拍板：CAW 三进程；开机自启做成**一个开关**（运行期可切、不停正在跑的服务）；停止服务 = 现在退出且不影响自启；Wake 在核心缺失时拉起用户身份 Core；v1 仅 `-L`；凭据由使用者填写 + machine-scope DPAPI；不采用 CAW R11 合并形态。
- 详情：`plans/2026-10-19-threadline-design.md`、`plans/2026-10-19-caw-install.md`。

### 方法论回灌

设计阶段暴露出 7 处 CAW 规则歧义与 3 处 MSO 缺口，已修正并推送上游：

- CAW `41f698a`：R7 档位=注册层配置、R5 交接执行者、R10 监督者形态与用户停止语义、I8 限定事件式监督、I4 单实例原语、AP9 补读侧 + AP9/AP10 注册物所有权、平台映射与清单。
- MSO `5c432f9`：核心纪律 1 环级终止保障、§6 机检自检与代码级声明。
- 记录：`decisions/0001-caw-ambiguities-and-readings.md`。

### 工具链

- Rust 1.99.1（rustup）+ VS 2022 Build Tools（C++ 工作负载 + Windows SDK，MSVC 14.44）。

### B0 工作区与项目骨架

- 外层工作区：`AGENTS.md`、`README.md`、`docs/{TODO,DONE,TESTPLAN}.md`、`decisions/`、`scripts/verify.sh`。
- 项目骨架：Cargo workspace（`tl-protocol` / `tl-core` / `tl-wake`）、MSO 规则副本 `docs/mso/AGENTS.md`。
- 契约 `tl-protocol`：发现契约 `control.json`、19 个端点 DTO、6 个磁盘文件类型，14 项 golden 单测。
- 架构机检 `tools/msocheck/msocheck.mjs`：R1-R6 + `--selftest` 43 项。
- 门禁 `scripts/verify.ps1`（fmt + clippy + test + 机检自测 + 机检）；`bash scripts/verify.sh` 全绿。

### B1 底座与四个模型

- `foundation`：
  - `store` 原子 JSON 读写（临时文件 + fsync + rename）
  - `token` CSPRNG → Base64URL
  - `retry` 有界退避（1→2→4→8→16→30 封顶，默认 10 次）
  - `limiter` 并发预算（超限立即拒绝，不排队）
  - `lock` 单实例文件锁（跨身份可用，句柄关闭即释放）
  - `secret` DPAPI machine-scope 绑定（`seal`/`open`，两身份可解）
  - `registry` 任务计划封装（字段级前置校验 + 创建/删除/启用/禁用/查询）
- `models`：
  - `credential`：加密落盘、摘要无密钥、`material` 进程内取用、引用阻断
  - `server`：引用校验、主机密钥**显式信任**（不隐式 TOFU）、连接参数
  - `tunnel`：六态状态机（相位约束）+ 定义 CRUD + 端口占用登记 + 事件入口
  - `runtime`：`control.json` 发现契约、自启开关（含提权判定）、降级集合
- 门禁：`bash scripts/verify.sh` 全绿（**cargo test 82 项** + msocheck selftest 43 项 + clippy + fmt）。
- 详情：`plans/2026-10-19-threadline-design.md` §7。

### B2 SSH 引擎

- `models/tunnel/engine.rs`：
  - 连接：`russh` 0.64（**ring** 后端，避开 aws-lc-sys 需要的 NASM）；keepalive 30s × 3 由 config 驱动，另有 30s 轮询 `is_closed()` 主动判死
  - 主机密钥：握手回调采集指纹；**未知 → `NeedsTrust`（未转发任何字节就断开）**；不一致 → `FingerprintMismatch`（绝不自动接受）
  - 认证：密码（`authenticate_password`）/ 私钥 + 可选口令（`PrivateKeyWithHashAlg`），失败原因区分「格式无法解析 / 口令错误 / 服务器拒绝」
  - `-L` 转发：`bind_local` 先抢端口 → accept → `channel_open_direct_tcpip` → `copy_bidirectional`；并发上限 64（超限立即关闭）
  - **监听不释放**：`serve` 把 `TcpListener` 归还调用方，退避期间端口仍在本进程手上；`reject_one` 负责退避期新连接快速失败
  - 结局 → 状态机事件映射（`to_event`），失败阶段区分建连（`HandshakeFailed`，可退避）与转发（`NetworkLost`）
- 状态机补 `FingerprintMismatch` 事件（→ `Error`，等人工处理）。
- 门禁：**cargo test 89 项**（新增 7 项：Debug 脱敏、端口冲突、非法主机、退避期拒绝、映射正确性、映射后事件合法性）+ msocheck 43 项 + clippy + fmt。
- 未做：真实 sshd 验证（无服务器可用），已登记为待环境验证项。
- 详情：`plans/2026-10-19-threadline-design.md` §8。

### B3 控制 API、业务编排与壳层装配

- `business`：
  - `TunnelRunner`：启动（模型推进 → 取值 → 绑定 → 交给引擎）、停止、批量停止、开机自启；**引擎只跑单次，退避循环在这里**（退避预算用底座 `Backoff`，耗尽进 `Error`）
  - 退避期保持监听 + `engine::reject_one` 快速拒绝新连接
  - `Services`：`on_startup` / `shutdown`（先停数据面再写 stopped）/ `whoami` / `autostart_status` / `set_autostart`
- `interface`：19 个端点 + Bearer 中间件（常量时间比较）+ 统一错误体（字段级错误带 `field`）+ 日志尾部读取
- 壳层 `main.rs`：CLI（`--boot/--manual/--stand-in/--fallback`、`--data-dir`、`--autostart on|off`）、单实例文件锁、控制端口顺延（18921–19020）、写出发现契约、优雅退出、退出码 2/3/5/7
- **`--autostart` 一次性模式**：不抢锁、不绑端口、不写 `control.json`，结果写 `autostart.result.json` 供 Agent 读取
- 门禁：**cargo test 113 项** + msocheck 44 项（新增一条：框架 `tokio::spawn` 不算违规）+ clippy + fmt
- **端到端冒烟**（`scripts/smoke.ps1`，真进程 + 真 HTTP）：healthz 免鉴权可达、受保护端点无 token 返回 401、`whoami` 报 `manual` 与 `user:` 身份、隧道列表为空、引用不存在的凭据返回 409、`/logs` 可用、`/shutdown` 返回 202 且进程 10s 内退出码 0、退出后 `control.json` 变为 `stopped` —— **全绿**
- 详情：`plans/2026-10-19-threadline-design.md` §5。

### B4a Vue 控制面板

- 三层结构：`src/views/`（只渲染）、`src/api/`（**唯一 HTTP 出口**，`client.ts` 封装 fetch + 类型化 `ApiFailure`）、`src/models/`（Pinia store，界面规则集中在这里）
- 契约镜像：`src/api/types.ts` 手写对齐 `tl-protocol` 的 DTO
- 页面：隧道（列表/启停/新建/开机自启/错误原因）、服务器与凭据（凭据录入支持私钥粘贴 + 可选口令；主机密钥显式信任/拒绝）、日志（3 秒轮询 + 尾部截断提示）、设置（自启开关 + 核心信息 + 降级清单）
- 顶部状态条：核心在线/不可达、运行方式、降级标记、停止服务
- 门禁加入前端构建：**`pnpm build`（vue-tsc 类型检查 + vite 打包）通过**，`dist/` 产物 1.05 MB（Element Plus 全量引入，v1 接受）
- 踩到的坑：`verify.ps1` 里一句中文注释把 Windows PowerShell 5.1 的 ANSI 解析搞崩（自己定的"ps1 必须纯 ASCII"规矩）—— 已改回英文注释
- 详情：`plans/2026-10-19-threadline-design.md` §7.4。

### B4b Tauri 壳（Agent）

- `src-tauri`：Tauri 2.12 + `tray-icon` + `tauri-plugin-single-instance`；`[workspace]` 空声明让它独立于 `crates/*` 的依赖解析
- 托盘：打开面板 / 停止服务 / 退出助手；左键点击弹窗；**关窗 = 收进托盘**（退出服务 ≠ 退出助手）
- **停止服务走 API**：托盘菜单直接 `POST /api/v1/shutdown`（手写最小 HTTP 请求，不引额外依赖）—— 这是 CAW C4「低权限助手用 token 停提权核心」的落地
- **单实例**：第二次启动把 `--show` 转发给已运行实例并弹窗，自己退出（冒烟实测退出码 0）
- **发现契约注入**：从 `control.json` 读端口 + token，`window.__THREADLINE__` 注入 WebView；有界重试（最多 10s，绝不无限等）
- 图标：`scripts/make-icon.ps1` 用 System.Drawing 画「引线 + 两端点」（1024×1024），`pnpm tauri icon` 生成全尺寸（含 ico）
- 编译：dev 构建 3m02s（首次），增量 16s
- **Agent 冒烟**（`scripts/smoke-agent.ps1`）：真 core + 真 agent 启动、agent 存活、第二实例转发后退出 0、core 仍在服务 —— **全绿**
- 详情：`plans/2026-10-19-threadline-design.md` §2。

### B5 唤醒入口 `tl.exe`

- 按 TDD 做：先写 13 项测试 + `todo!()` 空实现，确认全部 panic（RED），再实现（GREEN）。
- 决策与执行分离：
  - `plan(probe, task)` 纯函数：`Healthy` → 不打扰；`Absent/NotOurs` + 任务启用 → `TriggerTask`；否则 `SpawnManual`
  - `plan_after_trigger_failed(task)`：自启开着却起不来 → `SpawnStandIn`（降级），否则 `SpawnManual`
  - `NotOurs` 单独成态：**端口被别的程序占着不算"核心在跑"**（`instance_id` 比对不上）
- 手写最小 HTTP（只发一个 `GET /healthz`）与 `schtasks /query /xml` 解析，不引异步栈
- 拉起用 `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP`，Wake 绝不等子进程
- **TDD 过程中抓到的真 bug**：拉起核心后立刻读 `control.json`（那时还没写），导致第一次 wake 误报"核心不可达"退出码 4。补 `wait_for_control`（先等发现契约出现，再探测健康），并补两条测试（文件出现即返回 / 无文件超时）锁定行为。
- **Wake 冒烟**（`scripts/smoke-wake.ps1`）：第一次 `tl.exe` 退出 0 并把核心拉起、`healthz` 通、**第二次 `tl.exe` 复用同一核心**（`instance_id` 不变）—— 全绿。
- 踩到的坑：PowerShell `Start-Process -Wait` 会等待**整个进程树**（含被拉起的常驻核心），误以为 Wake 卡死；冒烟脚本改用 `WaitForExit(ms)` 只等自身。
- 门禁：**cargo test 136 项**（新增 23 项）+ msocheck 44 项 + clippy + fmt + 前端构建。
- 详情：`plans/2026-10-19-threadline-design.md` §3.4。

### B6 打包与安装器

- **sidecar 打包**：`tauri.conf.json` 加 `bundle.externalBin = ["binaries/tl-core", "binaries/tl"]`；`scripts/bundle-sidecars.ps1` 把 `target/debug/{tl-core,tl}.exe` 暂存成 Tauri 要求的 `*-x86_64-pc-windows-msvc.exe` 命名。安装后三者同目录，正是 `tl.exe` 查找兄弟进程的前提。
- **安装包产出**：`pnpm tauri build --debug --bundles nsis` 成功，得到 `Threadline_0.1.0_x64-setup.exe`（5.31 MiB）；生成的 `installer.nsi` 里确实包含两个 sidecar（4 处引用）。
- **Agent 程序名兼容**：安装态 Tauri 会把主程序名改成 `Threadline.exe`，Wake 原先只找 `tl-agent.exe` → 会"核心起来了但面板打不开"。补 `sibling_exe_any`，按 `tl-agent.exe` / `Threadline.exe` / `threadline.exe` 依次查找。
- 踩到的坑：Tauri CLI 在非交互 shell 里找不到 `cargo`（rustup 装到 `%USERPROFILE%\.cargo\bin` 但没进 PATH），打包前必须显式 prepend PATH。
- 回归：Wake 冒烟在改名兼容改动后仍全绿。
- 详情：`plans/2026-10-19-threadline-design.md` §3.1 与 `docs/TESTPLAN.md` U-04/U-05。

## 2026-10-20

### 界面重做：侧栏 + 大标题 + Hero 结论卡 + 端口卡片网格

**起因**：用户判定"完全不美观"，且明确指出前两轮只是在配色上打转，要的是换设计、换交互。

**做法**：
- **信息架构**：顶栏分段导航 → 侧栏（品牌 / 导航带计数 / 底部服务开关 / 状态脚注）；
  主区从"表格"改成"页面级大标题 + Hero 结论卡 + 端口卡片网格"。
- **Hero**：一句话结论（已就绪 / 尚未连接 / 等待确认密钥）+ 本机 ⇄ 服务器示意图 +
  三格指标（会话状态 / 运行中的转发 / 监听范围）。
- **端口卡片**：状态胶囊 + 自动开关、`本机 → 远端` 映射条、URL 行（可复制）、失败原因（限高三行）、
  操作行（打开本地页面 / 编辑 / 删除）。
- **设计系统**：墨绿玻璃质感（`#071316` 底 + 顶部青色光晕 + 半透明卡片 + 柔和外阴影），
  薄荷→青渐变只用于主操作与已连接；按钮、开关、卡片全部手写，不再套 Element 默认观感。

**新增能力**：
- `open_external` 命令（`rundll32 url.dll,FileProtocolHandler`，仅放行 http/https）→ 「打开本地页面」；
- 复制转发地址（`navigator.clipboard`）。

**顺带修掉的两个真问题**：
- 已启用但没连上时，主按钮显示「断开连接」（语义相反）→ 改为「重新连接」
  （先 `disconnectSession` 再 `setServerEnabled(true)`）。
- `beforeBuildCommand` 不含前端构建 → `pnpm tauri build` 会静默打包**上一版界面**。
  改为 `scripts/prebuild.ps1`（sidecar + `pnpm build`）。

**验证**：抓图逐屏确认（侧栏 / 页头 / Hero / 示意图 / 指标 / 错误条 / 端口卡片 / 卡片底部操作行）；
`localUrl` 新增 5 条单测（含通配地址 → `127.0.0.1`）。

- 门禁：cargo test 147 项 + agent 4 项 + frontend 21 项 + msocheck 44 项 + clippy + fmt + 构建全绿。

## 2026-10-21

### 界面二次调整：侧栏承载隧道、Hero 精简、转发区分协议、快捷键呼出

**用户反馈**：停止服务放侧栏底部容易误触；隧道应放左侧、设置类入口放下方；
Hero 区无意义信息太多；新建转发应区分 http/https 与 tcp（tcp 不需要"打开本地页面"）。

**改动**：
- **侧栏**：品牌 → `SSH 连接` + 计数 → 「+ 新建连接」虚线按钮 → 隧道卡片（头像 / 名称 /
  状态点 / 转发数）→ 底部「N 条隧道正在运行」→ **快速呼出 Ctrl+Alt+D** → 日志 / 设置 → 版本脚注。
  「停止服务」大卡片移除，改为设置页的「核心服务」面板（二次确认），侧栏离线时只提供「启动服务」。
- **主区**：Hero（示意图 + 三格指标）整体删除，换成 SSH SERVER 卡片：
  `SSH SERVER` 小标 + 连接状态 + 大号 `user@host:port` + 一句话说明 + 标签（保活 / 转发数 / 指纹前缀）
  + 编辑 / 断开 / 删除。信息量与"能不能用"直接相关。
- **转发规则加本地协议** `scheme`（tcp / http / https，默认 tcp）：只有 http/https 显示
  URL 行与「打开本地页面」；TCP 显示"纯 TCP 转发，用客户端连接 127.0.0.1:port"。
  老配置没有该字段，反序列化按 tcp 处理。
- **全局快捷键**：`Ctrl+Alt+D` 呼出/收起面板。用 `RegisterHotKey` + `GetMessageW`
  直接走 user32，不引入 `tauri-plugin-global-shortcut`（少一个依赖与权限声明）。

**验证**：
- 端到端：`POST /tunnels` 带 `scheme=http` → 列表读回 `http`；界面 HTTP 卡片出现 URL 与「打开本地页面」，
  TCP 卡片只有 TCP 提示（`scripts/check-scheme.py`）。
- 快捷键：连按两次 `Ctrl+Alt+D`，窗口可见性 True → False。
- 单测：`localUrl` 6 项（新增 https）；核心新增 3 项（旧配置默认 tcp、非法 scheme 拒绝、patch 生效）。

- 门禁：cargo test 149 项 + agent 4 项 + frontend 22 项 + msocheck 44 项 + clippy + fmt + 构建全绿。

## 2026-10-21

### 快捷键可配置 + 会话恢复后自动补启转发 + 卡片渐变与流光边框

**四个反馈**：快捷键要能改且能报冲突；隧道连上了端口却没自动连；端口卡片太扁平；
已连接的卡片要边框流光。

**1) 快捷键可配置（含冲突检测）**
- 后端 `hotkey` 模块改成"专职线程持有注册"：`RegisterHotKey` + `GetMessageW`，
  改绑通过 `PostThreadMessage` 投给该线程，由它自己注销/注册（跨线程注销是错的）。
- 失败（`ERROR_HOTKEY_ALREADY_REGISTERED = 1409`）会把旧注册**回滚**并返回中文原因；
  非法组合（没有修饰键、键名不认识、F25 之类）在解析阶段就拒掉。
- 持久化到 `<data-dir>\agent.json` 的 `hotkey` 字段；启动时按它注册。
- 设置页新增「全局快捷键」面板：点「修改快捷键」进入录制，按下组合即保存，Esc 取消；
  侧栏的键帽跟着变。
- 单测 4 项：解析（大小写/顺序无关）、无修饰键拒绝、错键名拒绝、F12 可用。

**2) 隧道连上后端口不自动连**
- 根因：`keeper_loop` 重连成功只把 SSH 会话拉回来，**没有恢复**挂在它下面的转发；
  会话掉线期间停在 Error/Stopped 的转发不会自己回来。
- 修法：会话恢复分支里调用 `resume_auto_start_forwards`，只补启
  "auto_start 且当前不在 Running/Connecting"的转发。
- 选择逻辑抽成纯函数 `forwards_to_resume` 并单测（5 种状态组合 + 别的隧道不受影响）。

**3) 端口卡片不再扁平**：卡片改用径向 + 线性双层渐变，加内高光与外阴影；
映射条也从纯色块改成渐变。

**4) 已连接卡片加流光**：`::before` 画 `conic-gradient` 描边，用 `mask` 只留 1px 边框，
配合 `@property --tl-angle` 让光沿边框旋转；隧道「已连接」与转发「运行中」才挂 `.is-live`，
`prefers-reduced-motion` 下退化为静止渐变。

**验证**：抓图确认隧道与端口都以 `is-live` 渲染；间隔 1.2s 连拍两张，文件哈希不同（动画在跑）；
设置页快捷键面板显示后端返回的当前组合。

- 门禁：cargo test 150 + agent 6 + frontend 22 + msocheck 44 + clippy + fmt + 构建全绿。

## 2026-10-21

### 配置目录权限：普通用户只读 → 提供一次性提权修复

**背景**：确认"不放弃登录前启动"后，数据目录必须留在 `%ProgramData%\threadline`
（SYSTEM 在用户登录前要读配置），因此不能改成 `%APPDATA%`。但该目录由管理员/SYSTEM 创建，
交互用户只剩 `ReadAndExecute`——普通权限运行助手时改配置会写失败。

**改动**：
- 核心新增一次性模式 `--fix-acl`：以提权身份跑
  `icacls <data-dir> /grant <USERDOMAIN\USERNAME>:(OI)(CI)M /T`，结果写 `acl.result.json`。
  `/T` 是为了把已存在文件的显式 ACL 一并修正（新建文件才自动继承）。
- 助手新增命令 `data_dir_status`（建删临时文件做可写探针）与 `fix_data_dir_acl`
  （走既有 UAC 通道：`ShellExecuteW runas` 拉起 `tl-core --fix-acl`，轮询结果文件）。
- 界面：数据目录不可写时在顶部给出横幅 + 「修复权限」按钮，而不是等用户点保存才报错。
- 单测：`--fix-acl` 参数解析与默认关闭。

**验证**：
- `tl-core --fix-acl` 执行后 ACL 变为 `TENCENT\zlyuanzhang : Modify`（实际为当前用户）；
  `runas /trustlevel:0x20000` 受限令牌写探针从"失败"变为"成功"。
- 重启界面后不出现只读横幅。

- 门禁：cargo test 151 + agent 6 + frontend 22 + msocheck 44 + clippy + fmt + 构建全绿。

## 2026-10-21

### 数据目录迁到 `%PUBLIC%\Threadline`：跨身份可写，不再需要 ACL 手术

**问题**：`%ProgramData%\threadline` 由谁创建就归谁——管理员建则交互用户只读，
SYSTEM 建则用户只有 `Write`（缺 `Delete`，原子替换失败）。而要保留"登录前启动"，
数据就必须是 SYSTEM 与登录用户共写。

**实测三种创建者的继承结果**（详见 `docs/knowledge/guides/windows-platform-traps.md` 第 14 条）：

| 目录 | 创建者 | 用户得到 | 保存配置 |
|---|---|---|---|
| ProgramData | 管理员 | ReadAndExecute | ❌ |
| ProgramData | SYSTEM | Read+Write（无 Delete） | ❌ |
| ProgramData | 用户 | FullControl（CREATOR OWNER） | 仅自己建的文件 |
| **Public** | 任意 | **INTERACTIVE: Modify** | ✅ |

**改动**：
- 核心与助手的默认数据目录改为 `%PUBLIC%\Threadline`（`--data-dir` 仍可覆盖）。
- 首次启动做一次性迁移：把老 `%ProgramData%\threadline` 下的
  `servers.json` / `tunnels.json` / `credentials.dat` / `agent.json` 复制到新位置，
  **只补缺失文件、不覆盖**，旧目录原样保留备查。
- 助手读取快捷键时也回退读老位置（它可能先于核心启动，迁移还没跑）。
- 任务计划命令行不含 `--data-dir`，自动跟随新的默认值。
- `scripts/` 下 18 个 e2e/诊断脚本与 `audit-lifecycle.ps1` 的路径同步更新。

**验证**：
- 重启后 `C:\Users\Public\Threadline` 出现 4 个迁移文件 + 新 `control.json`，
  目录与文件的 ACL 均为 `INTERACTIVE: Modify (inherit)`。
- 用计划任务以 SYSTEM 写入一个文件，再用受限令牌（`runas /trustlevel:0x20000`）
  `move /Y` 覆盖它 —— 内容成功变为用户写入的值。
- 界面正常加载迁移后的隧道与转发。

- 门禁：cargo test 152 + agent 6 + frontend 22 + msocheck 44 + clippy + fmt + 构建全绿。

## 2026-10-21

### 会话掉线后的「僵尸转发」：界面显示运行中，背后已经没有 SSH 会话

**现象**：隧道行显示「未连接」，其下的转发却显示「运行中 · 暂无连接」。两行状态互相矛盾。

**根因**：转发的状态由 `listener_loop` 维护，而它只在 `serve_listener` 返回后才处理结局。
`serve_listener` 绝大多数时间挂在 `accept()` 上——**没人连进来就永远不会返回**。
于是 SSH 会话悄悄断掉（keepalive 失败 / 对端关闭）时：

- `session_connected()` 因为 handle 已关闭而返回 false → 隧道行显示「未连接」
- 转发任务还持有监听、状态仍是 `Running` → 卡片显示「运行中」
- 本地端口仍然被占着，新连接会先被接受、再失败

**修法**（`business/tunnel_runner.rs`）：
- 新增 `paused` 表：记录"会话断开时被暂停的转发"。
- `keeper_loop` 发现会话不可用 → 先 `pause_forwards_for(server)`：取消监听任务、
  释放本地端口、状态回落 `Stopped`，并写明原因「SSH 会话已断开：转发已暂停，重连后自动恢复」。
- 重连成功 → `resume_paused_forwards`：按**掉线前真在跑的那批**恢复（不看 `auto_start`，
  因为它们不是用户关掉的），再补 `resume_auto_start_forwards` 兜住其它自启项。
- 用户显式断开（`disconnect_session`）会清空该名单，避免"我主动断了却被自动拉回来"。

**验证**：
- 显式断开：`stopped_forwards: 1`，转发状态 `stopped` + 原因；约 5 秒后守护重连，
  转发自动回到 `running`（说明恢复链路本身是通的）。
- 意外掉线路径依赖真实网络抖动，无法在本机可靠复现，已登记为 TODO（拔网线/飞行模式验证）。

- 门禁：cargo test 152 + agent 6 + frontend 22 + msocheck 44 + clippy + fmt + 构建全绿。

## 2026-10-21

### 级联启动做成明确契约：核心起来 → 连启用的隧道 → 连这些隧道下启用的端口

**用户给的目标语义**（此前只是"碰巧成立"，各处口径不一）：

1. 核心连上后，自动连接**启用**的隧道
2. 隧道连上后，自动连接**启用**（`auto_start`）的端口

**审计出的三处缺口**：

| 入口 | 之前 | 现在 |
|---|---|---|
| 新建转发（`POST /tunnels`） | 只写模型，不启动；要用户再拨一次开关 | 带 `auto_start` 就立刻连上 |
| 修改转发（`PATCH /tunnels/{id}`） | 只改模型：旧监听按老配置继续跑，状态却回 `Stopped` | 先停 → 改 → 需要就按新配置重启；改失败则把旧配置接回去 |
| 启动编排（`startup_autostart`） | 不管会话是否就绪，直接硬起端口（必然失败并留下 Error） | 只起"隧道已连上"的那些；其余写「等待隧道连接后自动启动」，由守护连上后统一恢复 |

**改动**：
- `Services::create_tunnel` / `patch_tunnel` 取代接口层直接调模型，级联规则收在业务层。
- `TunnelRunner::is_running` 暴露"监听任务是否在跑"这一事实，供修改时判断要不要重启。
- `startup_autostart` 增加会话就绪判断，明确"端口跟着已连上的隧道走"。

**验证**（`scripts/check-cascade.py`，端到端）：
```
create(auto_start)      -> running   ✅
patch(remote_port)      -> running   ✅   （此前会停在 stopped）
patch(auto_start=false) -> stopped   ✅
CASCADE: GREEN
```
另：核心重启后 `deva` 已连接、`HA` 自动回到 running（启动级联生效）。

- 门禁：cargo test 152 + agent 6 + frontend 22 + msocheck 44 + clippy + fmt + 构建全绿。

## 2026-10-21

### 转发卡片：备注、远端在前、协议改名、搜索、竖排 tab、"所有转发"页

**用户提的六点**，逐条落地：

1. **备注**：新建/编辑转发时在「名称」下面填备注；卡片在名称后面以 `· 备注` 显示。
   契约层加 `note`（默认空串，老配置兼容），超过 120 字拒绝（备注只用于展示与搜索，
   不给人贴日志的地方）。
2. **远端在前**：卡片映射条改成 `远端 … → 本机 …`，表单里也把「远端」移到「本地」之前，
   读起来是"服务器上的服务映射成本机端口"。
3. **「本地协议」→「协议」**：字段含义没变，只是叫法对齐用户语言。
4. **搜索**：端口转发标题右侧、`+ 新建转发` 左边加搜索框；匹配名称、备注、IP、端口。
5. **左侧导航**：日志/设置从两个并排按钮改成**竖排 tab**（左侧强调条 + 文字，无按钮边框）。
6. **新增「所有转发」tab**（在日志之前，且是默认页）：跨隧道铺平展示全部转发，按隧道分组，
   顶部搜索可匹配名称/备注/IP/端口/**隧道名与主机**；卡片点「编辑」会切到该隧道详情页
   （表单只有一处，避免两份弹窗）。

**顺带的结构整理**：
- 抽 `components/ForwardCard.vue`：首页与「所有转发」共用同一张卡片，样式与行为不会漂移。
- 抽 `models/forwardView.ts`：`isWeb / urlOf / pillOf / dotOf / connLabel / matchesQuery`
  变成纯函数，vitest 覆盖 5 条（含"通配地址 → 127.0.0.1"与四类搜索命中）。

**踩坑**：用正则批量删函数时把整个 `.vue` 吃掉了一半（`function x(...) {` 的匹配跨过了
多层大括号），靠 `git checkout` 恢复。之后改成**括号配对**扫描：先配平参数列表的 `(`，
再配平函数体的 `{`，可行且不再误伤。

- 门禁：cargo test 153 + agent 6 + frontend 27 + msocheck 44 + clippy + fmt + 构建全绿。

## 2026-10-21

### 侧栏底部那组元素要压到左下角（回归修复）

用户截图指出「运行状态 / 快速呼出 / 导航 tab / 版本」紧贴着隧道卡片，而不是停在左下角。

**根因**：`.rail` 是 flex 列，靠中间的 `<span class="spacer">` 撑开剩余空间。
但前几轮改样式时，`.spacer { flex: 1 }` 这条规则被误删了——元素顺序没错，撑开的力没了。
**修法**：加回 `.rail .spacer { flex: 1 1 auto; min-height: 16px }`，并注明它存在的理由。

**教训**：按"区域替换"改 CSS 时，被替换区间的边界规则（这里是 `.spacer`）最容易连带丢掉；
改完应该用渲染结果复核，而不是只看 diff 是否合理。

## 2026-10-21

### 「新建连接」只跳到 deva，不弹对话框

**现象**：在侧栏点「新建连接」，界面切到隧道详情页并停在那条隧道上，新建弹窗没有出现。

**根因是时序，不是事件绑定**：按钮做两件事——切 tab、发一个自增信号；主区 `watch` 这个信号开弹窗。
但默认页是「所有转发」，点击时 `HomeView` **还没挂载**：`watch` 在挂载后才建立，初始值已等于新值，
变化事件永远不会到来，信号就丢了。切 tab 生效了，所以表现成"跳到隧道页"。

**修法**：把"事件"改成"待办"。
- store 记 `createRequests` / `createHandled` 两个计数，`hasPendingCreate()` 判断是否还有没兑现的请求。
- 主区 `onMounted` 先补一次 `openIfRequested()`，之后 `watch` 接后续请求，处理完 `markCreateHandled()`。
- 单测 3 条：待办产生/消费、以及"连点两次只开一次"的边界（第二次仍算待办）。

**验证**：临时按真实时序（先切 tab、3 秒后发请求）注入一次，截图确认弹出「新建隧道」；
验证后移除临时代码。

## 2026-10-21

### 删除隧道时被"服务器仍被隧道引用：tun_HA_5796"挡下

**现象**：点删除，弹出内部术语 + 内部 id，用户既看不懂也不知道下一步做什么。

**两个问题**：
1. 文案泄漏了代码里的命名（server=隧道、tunnel=转发）和 `tun_xxx` 这样的 id。
2. 交互是死路：隧道下有转发就被拦，用户得先去别处一条条删。

**改法**：
- **界面级级联**：删除隧道前先看它下面有几条转发，确认框直接写清
  「删除隧道「deva」，会同时删除它下面的 1 条端口转发（HA）。继续？」；
  确认后按"先删转发、再删隧道"的顺序执行，最后刷新列表。
- **后端文案**：拦截信息改为
  「这条隧道下还有 1 条端口转发（strata），请先删除它们」——带数量与名称，
  不再出现内部 id（`check-delete-guard.py` 验证返回体）。

**没验到的**：确认框本身（`window.confirm`）无法用截图/注入点击验证，只能靠代码走查；
后端拦截与级联顺序均已用接口验证。

## 2026-10-21

### http/https 转发可以自定义「打开本地页面」的路径

**需求**：点「打开本地页面」常常要落到具体页面（如 `/lovelace/0`），而不是永远停在根路径。

**改动**：
- 契约新增 `open_path`（默认空串，老配置兼容），仅对 http/https 有意义。
- **写入时归一到"以 / 开头"**：填 `lovelace/0` 与 `/lovelace/0` 都存成后者——
  存的时候统一，拼接处就不用各自补斜杠（那种补法迟早漏一处）。超过 200 字拒绝。
- 表单在「协议」下面新增「打开路径」，前缀直接显示
  `http://127.0.0.1:<本地端口>`，只填路径部分；选 TCP 时该字段隐藏。
- 卡片 URL 行与「打开本地页面」都走同一个 `urlOf`，带上路径。

**验证**：
- 核心新增 1 条单测（归一化、空值、超长拒绝）；前端 `urlOf` 新增 1 条（带路径、缺前导斜杠、空与 "/"）。
- 端到端：给现有 http 转发 PATCH `open_path=lovelace/0` → 读回 `/lovelace/0`，
  界面 URL 行显示 `http://127.0.0.1:18080/lovelace/0`（截图确认，同页另一条未配置的仍是根路径）。

## 2026-10-21

### 本地端口冲突时"静默失败还提示端口已添加"

**现象**：新建转发填了一个已被占用的本地端口，弹窗关闭、提示"端口已添加"，但列表里没有。

**根因在 store 的成功/失败判定**：

```
create()  →  guard(api.create)      // 失败，lastError = "端口已被占用"
          →  refresh()              // 成功，lastError = ''   ← 把失败信息抹了
```

界面随后读 `lastError`，看到空串，就按成功处理。而 `refresh()` 里那句
`lastError = ''` 本意是"列表拉取成功说明连接是通的"，却顺手把上一个动作的错误也清了。

**改法**：
- `refresh()` 不再清 `lastError`（拉取成功不代表上一个写操作成功）。
- `guard()` 在**动作开始时**清空，保证"上一次的错误"不会被当成这一次的结果。
- servers store 同样处理（它更早，`guard` 里成功后还调了一次 refresh）。

**顺带把文案说清楚**：后端从 `本地端口已被其它隧道占用：18080`
改成 `本地端口 18080 已被转发「strata」占用，换一个端口`——点名是哪条规则占着，
用户不用自己一条条找；"隧道/转发"的用词也与界面一致。

**验证**：
- 接口：重复端口 POST → 409 + 上述文案（`scripts/check-port-conflict.py`）。
- 前端：新增 2 条 store 单测——"新建失败后刷新不能抹掉错误"、"下一次动作会清掉上一次错误"。

## 2026-10-21

### 删除运行中的端口转发"静默失败"

**现象**：转发在跑时点删除，没有任何反应（既没删掉，也没提示）。

**根因**：`canRemove` 把 `running` / `connecting` 判为不可删，卡片据此把按钮 `disabled` 了。
按钮变灰 + 无提示 = 看起来像坏了。而这个限制本身已经过时——后端 `remove_tunnel`
早就先 `stop_tunnel` 再移除，运行中的转发是能删的。

**改法**：
- `canRemove()` 不再按状态拦截（保留函数位置，写清为什么不再拦）。
- 卡片删除按钮只受 `busyKey` 约束。
- 删除前若处于 `running` / `connecting`，弹确认框：
  「「strata」正在运行，删除会先停止这条转发。继续？」（隧道详情页与「所有转发」页都加）。

**验证**（`scripts/check-delete-running.py`）：
```
state before delete: running
delete status: 204
still listed: False
reuse same port: 201   ← 端口确实释放了
```

## 2026-10-21

### 收尾：静默失败全量排查 + 沉淀

今天连修两个"界面不说话"的 bug（端口冲突后仍提示成功、运行中的转发删除按钮变灰无反应），
根因不同但同类，于是做了一次全量排查并把可复用的判定写成文档。

**排查结果**（`grep ':disabled='` 全项目）：剩余 14 处禁用条件全部是
「操作进行中」（`busyKey` / `saving` / `serviceBusy` / `refreshPending`）或
「环境不支持」（`!isTauri()`），没有第二处"因业务状态禁用且不给解释"。

**沉淀**：新增 `docs/knowledge/guides/ui-failure-modes.md`
- 三种静默形态：① 按钮被禁用且无解释 ② 错误信息被后续成功调用抹掉 ③ 无依据的成功提示
- 每条都给判定规则（例如"状态类禁用应改为允许 + 确认框，或至少给可见原因"）
- 附一份改完界面后必须过的清单（5 条）
- 说明本项目的验证手段：逻辑层单测 / 接口层脚本 / 临时注入截图（用完即删）

**收尾时又抓到一条"会随环境变红"的测试**：
`standalone_autostart_status_is_readable` 断言 `!status.enabled`，读的是这台机器的真实任务计划。
开发机装上自启任务后，全量测试直接红，但代码没问题。改成只断言"查询不报错 +
用户身份需提权"，并写明**不要断言机器状态的具体值**。

→ 教训归到同一类：**断言要落在自己的逻辑上，不要落在环境上**。

## 2026-10-21

### 把本项目的形态/存储经验回写进方法论 CAW（`41f698a` → `10a41fd`）

**动机**：两进程、数据目录、私钥存储这三块都是踩过坑才定下来的，留在项目内部只能帮到本项目。

**回写内容**（`git@git.zly.m:methodology/core-agent-wake.git`，`10a41fd`）：
- 新增 `PRACTICES.md` / `PRACTICES.zh-CN.md`：进程形态决策表（三/两/单进程，含"Wake 并入助手"
  的成立条件与四条安全规则）、数据目录按**创建者**区分的 ACL 实测表、凭据加密范围与代价，
  每条都附验证命令。
- `AGENTS`：新增 **R12**（数据目录必须对"所有可能运行核心的身份"可写，`Write` 而无 `Delete`
  视为不可写，原子写/可写探针/只补缺失的迁移）、**R13**（凭据范围与运行身份一致；机器级加密
  必须配收紧 ACL 或声明单用户前提；不读 `~/.ssh`），并补两条验收清单与术语。
- `INSTALL`：Step 1 增加三个决策问题（运行时机 / 单机还是共用 / 进程形态），
  **收到答案后必须先给一次推荐**（附推荐表）；Step 2 要求把选择与"为什么不是另外两种"写进设计文档；
  Step 3 增加"数据与凭据"落地步骤。
- `README`：顶部链接 PRACTICES，并加"两进程变体"一段（消失的是进程，不是职责）。

**项目内的对齐**：本项目据此定稿为两进程 + `%PUBLIC%\Threadline` + machine-scope DPAPI，
"同机其他用户可读凭据"作为单用户机器前提登记在 TODO；版本戳同步为 `10a41fd`。

## 2026-10-21

### 开机自启时，登录后也要把助手拉起来

**问题**：`开机自启` 只登记了 SYSTEM 任务（`tl-core --boot`），登录前拉起核心；
登录后没有任何东西把**助手**带起来——托盘不见，用户以为没生效。

**为什么不能由核心顺手拉起**：`HKCU` 是**每用户**的。以 SYSTEM 运行的核心去写 HKCU，
写的是 SYSTEM 自己的 hive（CAW AP9/AP10 明确禁止），表面成功、实际不生效。
所以这条必须由**跑在用户会话里的助手**自己维护。

**改法**：
- 助手新增 `sync_logon_entry(enabled)`：写/删
  `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 的 `Threadline` 值，
  命令行为 `"<助手 exe 绝对路径>"`（不带参数 → 登录后只出托盘）。
- **启动时自动同步一次**（`watch` 运行时状态里的 `autostart.enabled`）：
  开关开着就补上、关了就删掉；用户手工删过注册表项也会自愈。
- 设置页开关成功后立即同步，提示改为「…；登录后助手会自动启动」。
- 新增一次性命令 `tl-agent.exe --sync-logon on|off`：不启界面、不碰核心，
  用于自动化验证与"注册表项被外部删掉后一键修回"。

**验证**：
- 单测：`logon_entry` 的写/读/删往返 + 重复删除幂等（用独立值名，不碰正式项）。
- 端到端：`--sync-logon on` 后 `reg query ... /v Threadline` 能看到
  `"C:\\my\\threadline\\project\\threadline\\src-tauri\\target\\debug\\tl-agent.exe"`；
  `--sync-logon off` 后该值消失（测试写入的项已清理）。

**注意**：本次没有重启助手（避免顺带拉起核心），所以正式的启动项会在你下次手动启动时写入。

## 2026-10-21

### 登录自启只出托盘，不弹窗

**问题**：窗口配置没有 `visible: false`，且 `--show` 从未被解析——任何方式启动都会弹窗。
登录自启本来就不该打断用户。

**改法**：
- 窗口默认 **`visible: false`**（webview 照常加载，只是不显示）。
- 启动时区分两种意图：带 `--tray` → 只留托盘；**无参数（双击图标）→ 显示窗口**。
- 登录启动项命令改为 `"<助手 exe>" --tray`；已存在的旧项（无 `--tray`）在下次启动时
  因字符串不一致被自动改写（同步逻辑比对的是完整命令行）。
- 单测断言命令必须带 `--tray` 且路径带引号。

**验证**：
- 单测 8 项通过（含新增的 `logon_command_starts_in_the_tray_only`）。
- `--sync-logon on` 后注册表实读为
  `"...\tl-agent.exe" --tray`；`off` 后消失。

**待你确认**：这次的窗口可见性只在代码与配置层验证过——按你的要求没有启动核心，
所以没有真机跑一次"登录后只出托盘"。下次启动时可以这样看：登录后托盘有图标、无窗口。

## 2026-10-27

### 隧道排序弹窗 + 实时网速 + 端口自动跟随/随机

**1) 隧道排序**（设置页 → 「调整顺序」弹窗）
- 拖动行改序，每行另有两个按钮**一键置顶 / 置底**（长列表里拖到顶太难受）。
- 后端新增 `PUT /servers/order`（`ReorderServersRequest { ids }`）：顺序就是 `servers.json`
  的数组顺序，**没提交的 id 追加在末尾并保持原相对顺序**，不会因为部分提交把隧道弄丢。
- 单测：部分提交 / 完整倒序 / 未知 id 忽略三种情况。

**2) 实时网速**（端口 / 隧道 / 全部隧道三级）
- 协议 `TunnelStatus` 增加 `bytes_up` / `bytes_down`（单调递增累计值）。
- 引擎把 `copy_bidirectional` 换成 `split` + 单方向搬运循环，边搬边累加——
  `copy_bidirectional` 只在连接结束时给出总数，界面上永远是 0。
- 计数存在 `TunnelStore`（`traffic_of` / `reset_traffic`），转发重启时归零，避免把上一段算进新连接。
- 界面：`models/traffic.ts` 每次刷新采样、按间隔算速率（<400ms 的采样跳过，防抖）；
  端口卡片显示 `↑ ↓`、隧道卡片显示合计、侧栏底部显示全局合计。全 0 时整段不渲染。

**3) 端口联动与随机**
- 远端端口改动时，若本地端口**没被手动改过**，自动跟随（用 `localPortTouched` 记住意图，
  而不是比较两个数字是否相等——那样手动填成同一个值会被误判）。
- 本地端口旁加「随机」按钮：在 20000~59999 里挑一个未被其它转发占用的端口。

**验证**：
- 排序：`PUT /servers/order` 倒序提交后列表按新顺序返回，随后还原（`scripts/check-reorder.py`，GREEN）。
- 流量：通过转发真实打一次请求，`+up=365 +down=2978`（`scripts/check-traffic.py`）；
  `pipe` 的计数逻辑另有内存双工单测（写入 11 字节必须数到 11）。
- 界面截图：卡片 `↑ 1.23 KB/s ↓ 31.3 B/s`、侧栏底部合计速率，底栏改成两行避免挤压。

- 门禁：cargo test 157 + agent 8 + frontend 33 + msocheck 44 + clippy + fmt + 构建全绿。

### 修复：隧道顺序弹窗"拖不动"

**根因**：在 `dragover` 里直接 `splice` 换位——被拖的那个 `<li>` 会被从 DOM 上摘下来重新插入，
Chromium（含 WebView2）判定"拖拽源消失"，整次拖拽当场中断。表现就是按下去毫无反应。

顺带发现第二处：弹窗若在隧道列表加载完之前打开，会是一个**空列表**（`openOrder` 只在点击那一刻快照）。

**改法**：
- 拖拽整体改成**指针事件**（`pointerdown` → `pointermove` 算落点 → `pointerup` 才换位），
  不再依赖浏览器对 `draggable` 的实现；拖动时源行半透明、目标行高亮。
- 加一个 watch：弹窗打开着而列表到位了，就补一次填充。
- 行内 `user-select: none`，避免拖拽变成选中文字。

**验证**：截图确认弹窗里两行隧道（名称 / user@host / 置顶置底按钮）正常渲染；
拖拽的指针逻辑无法用注入点击验证，改由代码走查 + 手动确认。

### 修复：拖动时出现"禁止"光标；侧栏隧道补速率

**禁止光标的根因**：上一版修 HTML5 拖拽时，CSS 里留了 `-webkit-user-drag: element`。
它会让浏览器**再起一次原生拖拽**，与我们自己的指针拖拽打架；原生拖拽找不到合法放置目标，
光标就变成"禁止"。改成 `-webkit-user-drag: none`，并在行上加 `@dragstart.prevent` 兜底。

**侧栏速率**：隧道行在 `user@host` 下加一行 `↑ … ↓ …`，用 `tunnelSpeed(server.id)`，
全 0 时不渲染（避免每条都挂一行 0）。

**验证**：持续请求 napcat 转发制造流量后截图——侧栏 `↑ 1.06 KB/s ↓ 1.76 KB/s`、
卡片与底部合计同步显示。

**顺带发现的环境问题**（不是代码缺陷）：`mypc远程桌面` 那条转发报
`本地端口绑定失败 (os error 10048)`——6551 端口被 VS Code 占用。现在可以用「随机」按钮换端口。

### 速率双色 + 拖动实时换位（带 FLIP 动画）

**速率配色**：上行（↑）薄荷绿、下行（↓）天蓝。之前上行下行同色，同一个数字换个箭头最容易看错。
卡片 / 隧道卡片 / 侧栏 / 底部合计四处统一用 `.rate-up` / `.rate-down`。

**拖动实时换位**：上一版只在 `pointerup` 换位，拖的过程中列表纹丝不动，看起来像没反应。
现在 `pointermove` 越过相邻行中线就立刻换位——能这么做正是因为拖拽已改由指针事件驱动，
不存在"原生拖拽源被移除就中断"的问题。

**FLIP 动画**：数组换位是瞬时的，行会"跳"。换位前按 id 记录每行 `offsetTop`，
换完补一个反向 `translateY` 再 transition 回 0，看起来就是在平滑移动（160ms）。
用 `offsetTop` 而不是 `getBoundingClientRect`：后者含 transform，动画期间会把位移算进测量。

**验证**：截图确认四处速率均已按上/下行分色；拖动动画无法用注入事件验证，由代码走查 + 手动确认。

### 修复：拖动到某个高度时位置高频跳动

**根因是反馈回路**：每一帧都按"当前 DOM 里各行的中线"算落点，而换位又改变这些中线。
指针正好压在某个中线上时，这一帧算出「往下」，换位后同一高度又算出「往上」，
于是一帧一个来回，表现为原地抖动。

**改法**：拖动开始时把**每个槽位的中线 Y 采一次**存起来（`slotCenters`），之后只按这份
固定坐标把指针 Y 单调映射到槽位下标——槽位不随内容移动，映射没有回路，自然不抖。
拖动结束清空。

代价：槽位高度按开始拖动时的布局算，行高不一致时会有几像素误差；
本弹窗每行都是"名称 + user@host"，高度一致，可以接受。

### 修复：刚按下没移动，第一行就跳了一格

**根因**：落点按"指针越过了几个中线"算。多数人点在行的偏下位置，指针**天生就在该行中线以下**，
于是第一次 `pointermove` 就把落点算成下一格——手指还没动，顺序先变了。

**改法两步**：
1. 按下时记下 `pressOffset = 指针 Y − 该行中线`，之后用 `指针 Y − pressOffset`（即**被拖行的中线**）
   参与判断。刚按下时它正好等于自己所在槽位，落点稳定。
2. 判断基准从中线改成**相邻中线的中点（边界）**。以中线为准时"换过去"和"换回来"的阈值不对称，
   往回拖要退将近一整行才生效，手感发黏。

### 修复：保存隧道顺序报「核心不可达或跨源拦截 · Failed to fetch」

**根因**：新接口用了 `PUT /servers/order`，而 CORS 的 `allow_methods` 里只有
GET/POST/PATCH/DELETE/OPTIONS。浏览器对 PUT 会先发 OPTIONS 预检，预检里没有 PUT
就判定跨源失败 → 前端只拿到 `TypeError: Failed to fetch`。

**为什么脚本测不出来**：`check-reorder.py` 直连核心、不带 Origin，根本不走 CORS。
我上一轮就是这么"验证通过"的——这是这次最该记住的教训：**跨源接口不能只用脚本直连验收**。

**改法**：`allow_methods` 加 `Method::PUT`；并给 `check-reorder.py` 增加一次真实的
OPTIONS 预检断言（带 `Origin` 与 `Access-Control-Request-Method: PUT`，
检查响应里的 `access-control-allow-methods` 含 PUT）。

**验证**：
```
preflight allow-methods: GET,POST,PUT,PATCH,DELETE,OPTIONS
PREFLIGHT: GREEN
REORDER: GREEN（倒序提交后按新顺序返回，随后还原）
```

### 主题切换（四套配色）

设置页新增「外观」面板，四套深色主题：**墨玉**（原配色）/ **深海** / **紫夜** / **炭火**。

**做法**：
- `styles.css` 拆成两层：`:root` 只放结构变量（圆角、Element Plus 映射），配色全部下移到
  `:root, [data-theme='jade']` 与 `[data-theme='ocean'|'violet'|'ember']` 四段。
  加主题 = 复制一段、只改色值。
- 散落在规则里的硬编码颜色收敛成 token：34 处 `rgba(52,211,153,x)` 之类改成
  `color-mix(in srgb, var(--mint) x%, transparent)`，底色叠色改用 `var(--bg)`，
  `#05221c` → `var(--on-accent)`、`#f7d47a` → `var(--warn-text)`、`#fca5a5` → `var(--bad-text)`。
  否则换主题时绿色边框会残留在紫色界面上。
- `models/theme.ts`：只保存"选了哪个"并写 `<html data-theme>`；色值唯一来源是 CSS，
  不在 JS 里再存一份（两处写迟早不一致）。选择存 `localStorage`。
- `main.ts` 在挂载前套用，避免首屏先闪默认色。`--el-*` 映射只在 `:root` 写一次。

**验证**：
- 临时把默认主题设为 ocean 构建，截图确认整屏（侧栏、卡片、按钮、弹窗主色）都跟随；
  重启后仍是 ocean —— 说明 `localStorage` 持久化生效。
- 最后清掉 WebView 数据、还原代码重建，截图确认回到默认「墨玉」。
- `confirm`：`pnpm build`（含 `vue-tsc`）通过；门禁全绿。

### 修复：端口卡片上的流光不见了（主题重构引入）

**根因**：做主题时把散落的 `rgba(52, 211, 153, 0.05)` 批量改写成
`color-mix(in srgb, var(--mint) 0.05%, transparent)`——**把 alpha 当成了百分数**。
0.05 的透明度变成 0.05%，等于全透明，那圈 conic-gradient 流光就看不见了。

受影响的不只流光：58 处转换全都偏小 100 倍（边框、外发光、悬浮底色都一起变淡），
只是它们本来就极淡，肉眼不易察觉，而流光靠的正是这点亮度，于是最明显。

**改法**：统一把系数 ×100（`0.05 → 5%`、`0.45 → 45%`、`0.85 → 85%`），
58 处一次性修正，取值区间回到 5%~95%。

**验证**：放大截图核对"运行中"隧道的卡片顶边——可见沿边框转动的紫→粉弧光，
确认 `@property --tl-angle` + conic-gradient 的动画恢复；门禁全绿。

**教训**：批量正则替换颜色时，`rgba(..., a)` 的 `a` 是 0~1，
`color-mix(..., N%)` 的 `N` 是 0~100，必须显式换算，且改完要挑一个"看得见的地方"验证
（流光就是那个地方）。

### 修复：点一个卡片的开关，影响到其它卡片

**根因**：两条转发的 `id` 完全相同（实机数据里 `测试x` 与 `测试` 都是 `tun____9396`）。
id 生成是 `tun_{slug}_{pid % 10000}`——同一进程里 pid 部分是常量，而中文名的 slug 会被
整段替换成 `_`，于是"改名后重建同名转发"必然撞车。id 一撞：前端 `v-for` 的 key 重复
（Vue 复用实例，视觉上点一个动两个），后端按 id 的 PATCH/start 也会打到同一条。

**改法**：
- `new_id(name, taken)` 接收现有 id 集合，base 被占用就追加 `_2`、`_3`…，保证唯一；
- 新增 `dedupe_ids()`：`load()` 时修复历史脏数据（保留第一条，其余重新分配）并回写，
  只改 id、不动名字与端口；
- 单测两条：slug 相同时 id 不重复；文件里带重复 id 时加载后唯一且条目不丢。

**验证（实机）**：
```
tun_mypc_____2948   mypc远程桌面
tun_santang_492     santang
tun____9396         测试x
tun____9396_2       测试      ← 自动修复
IDS: GREEN
```
再调 `PATCH /tunnels/{id}` 只翻其中一条的 `auto_start`：
`自己变了=True  另一条被连带改了=False`（ISOLATION: GREEN，跑完自动还原）。
界面截图：四张卡片各自显示自己的自启状态（测试x 关、测试 开）。

### 开关去标签 + 语义核对 + 轮询提速

**去标签**：卡片头部不再写「自启」。这个开关就是"这条转发开不开"，旁边挂「自启」会让人以为是
第二个概念（要不要随隧道启动）。语义改由 `title` 说明：启用 → 隧道连上就启动、下次打开保持；
停用 → 不启动。同时补 `aria-label`。

**语义核对**：开关绑定 `def.auto_start`（持久化），点击 = 立即 `start`/`stop` + 写回该字段，
所以"当前状态"和"下次启动保持"是同一份数据，不会分叉。
已知边界：隧道本身未启用时打开开关，意图会保存但实际起不来，卡片显示「已停止」+ 黄色提示
「隧道 [x] 未启用」。（是否要"顺带启用隧道"需要产品决定，见下。）

**轮询提速**：`POLL_ONLINE_MS 5000 → 1200`，失联仍 5s，页面不可见 5s；
窗口从后台切回时立刻补一次（`visibilitychange`）。
依据：实测 `GET /tunnels` 中位 **2.3ms**、p95 3.8ms——延迟完全由间隔决定，不是接口慢。
（`scripts/check-latency.py` 可复测。）

### 实时化：核心主动推送（SSE）

**为什么做**：`GET /tunnels` 往返中位 2.3ms，而轮询间隔 5s——延迟 100% 来自"等下一次问"，
不是接口慢。压间隔只能治标（1.2s 仍然是人能感知的滞后）。

**做法**：
- 核心：`TunnelStore` 内置 `broadcast` 通道，状态迁移（`apply`）、错误（`set_last_error`）、
  增删改都敲一下；新增 `GET /events`（SSE），**只推"变了"不带载荷**，
  界面收到就重拉 `/tunnels`。这样核心不必维护增量补丁，界面也不会因漏事件长期跑偏。
- 界面：手写 `api/events.ts` 消费事件流——**不用 `EventSource`**，因为它无法设置
  `Authorization` 头，而把 token 塞进 URL 会进日志。用 fetch 读流自己解析 `\n\n` 分帧。
  断线指数退避重连（500ms→5s 封顶），页面卸载时 `close()`。
- 事件连发（connecting→running）用 **120ms 合并窗口**收敛成一次拉取。
- 轮询降级为"保险丝"：在线 5s、失联 5s、页面不可见 5s。

**验证**：
- `scripts/check-sse.py`：改一条转发 → **事件到达 5ms**（对比轮询 1200ms）。
- 端到端：用 API 翻某条转发的 `auto_start`，**0.9s 后截图**，卡片已从「已停止」变「运行中」、
  顶部计数 2→3，证明"推送→重拉→渲染"整条链通。
- 文字量：新增 `tokio-stream`（BroadcastStream）依赖。

### 开关语义确认（选 A）

保持现状：开关 = 这条转发的启用意图，落 `auto_start` 持久化；点击立即启停。
隧道本身未启用时打开开关，意图保存但实际起不来，卡片显示「已停止」+ 黄色提示
「隧道 [x] 未启用」，隧道启用后会自动补起。

### 修复：偶发 500「原子替换失败 os error 2」

**现象**：连续改同一条转发时，第二次 PATCH 有时返回
`{"code":"internal_error","message":"原子替换失败：tunnels.json.tmp -> tunnels.json"}`。
一开始误以为是杀毒/索引器占用（Windows 常见），加了 rename 重试反而没好转——
把 os error 带进消息后才看清是 **2（文件不存在）**，不是 5/32（共享冲突）。

**根因**：临时文件名是固定的 `<file>.tmp`。两个写入方并发时，A 先 rename 把 tmp 搬走，
B 再来 rename 就找不到自己的 tmp —— 报 os error 2。上一次的"偶发"其实就是并发概率。

**改法**：
- `temp_path()` 加进程号 + 自增序号，临时文件唯一；
- `is_transient()` 只认 5/32/33（共享冲突）才重试，NotFound 不重试（重试也找不回）；
- rename 失败消息带上 `os error` 码，下次一眼能分清两类原因；
- 新增并发单测：8 线程 × 5 次写同一文件，全部成功、最终文件合法、不留 tmp。

**验证**：`check-sse.py` 连跑 10 次，10/10 GREEN、无 500（此前 8 次里挂 2~3 次）。

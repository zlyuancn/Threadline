# 服务器终端 · 设计

> 状态：**已拍板**（2026-11-19，用户要求「主机管理开发支持终端」）；同日经独立审查修订（2 阻断 + 9 重要项已并入）。
> 实施进度：**代码侧已完成**（2026-11-19，A1–B5 全部落地，门禁全绿）；真机验收（U-42 / U-43 / U-44）待有真实 SSH 服务器时补跑。B6（容器控制台共用纯件）本轮未做，记在 `docs/TODO.md`。
> 本次交付：把管理页「终端」模块从 `Planned` 转为 `Ready`——即 `2026-11-16-server-admin-design.md` 里 P2 的 terminal 占位落地。
> 前提已具备：`engine::open_pty`、WebSocket 帧协议、`ContainerConsole.vue`（P3c 真机验证通过）。

## 目标

隧道连上之后，用户不必另开一个终端模拟器：在管理页左侧点「终端」，直接拿到这台服务器的登录 shell，能跑 `top`、`vim`、`systemctl`，也能在 shell 里自己 `sudo`。

## 导航顺序（2026-11-19 拍板）

左侧模块顺序固定为：

```
概览 · 终端 · 文件管理 · 容器管理 · 虚拟机 · 进程管理 · 网络管理 · 防火墙 · 服务与自启 · 日志
```

顺序由核心 `models/server_admin/mod.rs::modules()` 的 vec 决定（清单下发，前端只按序渲染），并有一条顺序断言守着——`module_order_matches_the_agreed_navigation`。改动顺序只改那一处，不要在前端排序。

## 原则

| 原则 | 说明 |
|---|---|
| 复用会话 | 不新建 SSH 连接：借隧道已有的那条会话（`runner.session_handle`，未连接时按需建立一次），与端口转发、概览采集共用 |
| 命令由核心给 | 前端不能传命令串。核心只有一条固定命令（登录 shell），避免这条 WS 变成「任意命令执行」的通用口子 |
| 身份不加工 | 配置里是哪个用户就以哪个用户登录，登录后是什么身份就是什么身份；界面不提供 root/su 档位，需要提权由用户在 shell 里自己决定 |
| 生命周期跟管理页 | 管理页内切模块（概览 / 容器 / 终端）**不断开**；离开管理页（返回隧道页）立即断开，远端 shell 一起退出 |
| 共用底座，生命周期各自持有 | 后端 PTY↔WS 收发循环抽一处 `pty_bridge`；前端只共享**纯件**（终端实例创建、帧解析、状态归约）。抽屉（`v-if`）与模块（`v-show` 常驻）的生命周期本质不同，把可见性策略塞进共用件会把已验证的容器控制台带坏 |
| 不假装能用 | 远端不是 Linux 或没有 shell 时，把远端报错原文显示在终端里，不弹「未知错误」 |

## 交互

- **入口**：左侧导航「终端」由淡点（规划中）变为可点，说明文案改为「登录 shell（bash 优先）」。不加隧道卡片按钮，不加托盘入口。
- **进页即连**：切到终端模块自动建立会话，不做「点开始才连」的两段式。
- **工具栏**（一行）：
  - 状态灯：连接中 / 已连接 / 已结束 / 失败；
  - **启动命令**：显示核心实际使用的整条串（`sh -c "command -v bash … && exec bash -l || exec sh"`），hover 给全文。**不承诺**「现在用的是 bash 还是 sh」——hello 帧只有一个 `command` 字段，核心无法知道 `command -v` 走了哪一支，硬编「bash 优先」就是说谎；
  - 「清除」（只清屏，不断会话）；
  - 「重开」（`exit` 后或连接异常时重建会话）。
- **区域**：终端占满管理页右侧剩余高度（最小 320px），宽度随窗口，`ResizeObserver` 触发 `fit` 并把新尺寸推给远端 PTY。
- **提示**：终端区底部一行说明——会话不落盘（无历史、无回放），离开管理页即断开，远端的 shell 会一起退出。
- **失败态**：连接失败时在终端上方显示原因（与隧道页同源的错误文案），并给「重试」。

### 页头服务器切换器（2026-11-19 追加需求）

管理页顶部的服务器名（`deva` / `root@192.168.28.98:33303`）变成可点下拉，**不离开管理页**就能换一台机器看。

- **展开**：点击名称区弹出浮层（绝对定位卡片，宽约 320px），点外部或 `Esc` 关闭；当前项高亮 + 勾选。
- **列表**：只列 `kind !== 'local'` 的服务器（「本机」没有会话，管理入口本来也不给它）。每项显示名称 + `user@host:port` + 连接状态点，语义与侧栏卡片的三态一致（已连接 / 已知未连接 / 未查到）。顺序与侧栏一致（用户的排序就是他的心智顺序）。
- **过滤**：条目多于 6 个时，浮层顶部出现过滤输入框，按名称与 `host` 匹配。
- **切换后**：**停在当前模块**（在容器管理页切过去就是新机器的容器列表）；`active` 不变，概览数据清空重采，容器面板重载，终端按 `:key="serverId"` 重建（旧 shell 退出、新 shell 建立）。不做二次确认——终端里有没有跑东西由用户自己判断。
- **状态归属**：`adminServerId` 仍在 `App.vue`（唯一真相），管理页只 `emit('switch-server', id)`；侧栏高亮随之跟随（`tunnelActive` 已经按 `adminServerId` 判定）。

## 生命周期

| 时刻 | 发生什么 |
|---|---|
| 点「终端」 | 面板挂载（首次）→ xterm 建实例 → WS 连接 `/admin/terminal` → 核心借会话 → `open_pty` → hello 帧 → 状态置「已连接」 |
| 切到「概览」/「容器」 | 面板用 `v-show` 隐藏、**不卸载**：WS 保持，输出继续写入 xterm 缓冲（scrollback 2000 行），回来时画面还在、`cd` 的位置还在 |
| 隐藏期间尺寸 | `fit()` 在 `display:none` 里会算出 2×1（FitAddon 只在 cell 尺寸为 0 时才返回 undefined），观察器必须先判 `offsetParent` / `offsetWidth`，不可见就跳过；重新可见时补一次 `fit()` + `refresh()` |
| 返回隧道页 | `ServerAdminView` 卸载 → 面板卸载 → 发 `{type:"close"}` 并 `close()` → 核心丢掉指令端 → PTY 任务 `eof` + `close` 远端 channel → 远端 shell 退出 |
| 概览刷新失败 | 终端会话**不受影响**：终端面板与概览的状态链并列渲染，不挂在 `v-else-if="overview"` 分支里 |
| 换服务器（页头下拉） | `active` 保持不变；`admin.load()` 检测到 id 变化就先清掉 `overview` 再采（`models/serverAdmin.ts:39-42` 已有）；容器面板 `watch(props.serverId)` 关抽屉并重载；终端面板带 `:key="serverId"` 强制重建，旧 shell 退出、新 shell 建立 |
| 远端 `exit` / `Ctrl+D` | 核心侧只表现为 `PtyEvent::Closed`（不发 `exit` 帧），前端从 `onclose` 推「已结束」+ 一行灰字提示；工具栏「重开」可重建 |
| 核心重启 / App 重启 | WS `onclose` → 状态「已结束」，提示点「重开」；**重开前先刷新一次 bootstrap token**（核心每次启动轮换 token，停在终端页时全局轮询不保证已刷新），否则第一次重开会撞 401 并被误报成「连不上核心」 |

## 接口契约

```
GET /api/v1/servers/{id}/admin/terminal?cols=100&rows=30&token=<token>
→ 101 Switching Protocols
```

- **帧协议完全沿用容器控制台**，不新增协议、**不动 `tl-protocol`**（`ServerFrame` / `ClientFrame` 是 interface 私有枚举）：
  - 服务端：`{"type":"hello","command":"sh -c \"…\"","mode":"shell"}` / 二进制原始输出 / `{"type":"exit","code":0}` / `{"type":"error","message":"…"}`
  - 客户端：`{"type":"input","data":"…"}` / `{"type":"resize","cols":100,"rows":30}` / `{"type":"close"}`
  - 二进制而不是 base64：多字节 UTF-8 会被切在包边界上，配 `TextDecoder({stream:true})` / `Uint8Array` 才不乱码。
- **令牌走 query**：浏览器不能给 WebSocket 设请求头。白名单从「仅 `/console`」扩为「`/console` 或 `/admin/terminal`」（后者更窄，不误伤任何其它后缀）。其余路由仍必须 `Authorization`。
- 鉴权必须有**路由级**测试：无 token → 401；带 token → 走到升级失败（400），证明新路由挂在 `protected` 上而不是只测了判断函数。
- `cols` / `rows` 沿用现有 `clamp_cols(20..400)` / `clamp_rows(5..200)`，0 值已被夹住。

## 分层归属

| 位置 | 职责 |
|---|---|
| `models/server_admin/shell.rs`（新） | `pick_shell(login: bool)`：唯一的 shell 选择规则。容器控制台调 `pick_shell(false)`（与现有字符串逐字一致），终端调 `pick_shell(true)`（`bash -l` → `sh` 兜底）。两条命令不再各写一份 |
| `models/server_admin/terminal.rs`（新） | 终端模块：`login_shell_command()` = `pick_shell(true)`；不接收任何参数 |
| `models/server_admin/mod.rs` | 注册 `pub mod shell; pub mod terminal;` 与再导出；`terminal` 模块状态 `Planned → Ready`；模块文档里把「只读」限定为采集类模块 |
| `business/admin.rs` | 新增 `server_terminal()`：确认隧道 → 取会话 → `open_pty`；`container_session()` 改名 `admin_session()`（全文件 7 处调用 + `overview()` 里的重复写法一并收口） |
| `business/mod.rs` | `Services::server_terminal()` 薄封装 |
| `interface/mod.rs` | 新路由；把 `console_loop` 的收发循环抽成 `pty_bridge(socket, opened, mode_label)`，**一次纯搬运提交**，不带行为改动（容器控制台退化时能单独 bisect） |
| `interface/auth.rs` | `console_query_token` 改名 `pty_query_token`，匹配 `/console` 与 `/admin/terminal` |
| `src/api/index.ts` | `openServerTerminal()`：async，先 `refreshBootstrap()` 再构造 URL（与 `openConsole` 同样的老问题，顺手一起修） |
| `src/models/ptySession.ts`（新，纯件） | `THEME`、`createTerminal(host, opts)`、`parseServerFrame(raw)`、`reducePtyState(state, event)`。**纯函数/无 DOM 状态机，vitest 在 node 环境可测** |
| `src/views/admin/TerminalPanel.vue`（新） | 自己的 `connect` / `teardown`（含不可见时跳过 `fit`、恢复可见补 `fit` + `refresh`）+ 工具栏；异步组件 |
| `src/views/ServerAdminView.vue` | 终端面板提到 `.admin-main` 直属（与概览状态链**并列**）；三态分支加 `active === 'overview'` 限定；`terminalMounted` + `v-show`；`:key="serverId"` |
| `src/views/ServerAdminView.vue`（页头） | 服务器切换器：浮层、过滤、状态点、当前项高亮；只 `emit('switch-server', id)`，不自己改 id |
| `src/App.vue` | 接 `@switch-server` 更新 `adminServerId`（唯一真相）；侧栏高亮已按它判定，不需改 |
| `src/models/serverAdmin.ts` | `pickableServers(servers, keyword)` 纯函数：剔除 `kind === 'local'`、按名称与 `host` 过滤；node 环境可单测 |
| `src/views/admin/ContainerConsole.vue` | 只把 `THEME` / `parseServerFrame` / `createTerminal` 换成共用的纯件，**生命周期逻辑不动**（可选任务，回归不过就整条回退） |

## 安全与边界

- **命令固定**：`pick_shell(true)` 产出 `sh -c "command -v bash >/dev/null 2>&1 && exec bash -l || exec sh"`。整段是双引号包裹的**单个参数**，内部无 `$`、反引号、`!`，csh/fish/zsh/dash 都会原样交给内层 `sh`。**纪律：以后任何插值（`cd $DIR`、`$USER`）都会在 csh/fish 下炸**，不做插值。
- **不落盘**：无日志文件、无输入回放、无命令历史；scrollback 只在内存，关掉即消失。
- **不做占用互斥**：容器控制台要互斥是因为 `docker exec` 共享容器 TTY，两人同进会串输入；SSH shell 各自独立 PTY，两条会话互不干扰，互斥只会让第二个窗口莫名失败。
- **不设总超时**：交互会话可能开很久；握手失败由 `error` 帧给原因，断开由 WS 关闭表达。
- **不改隧道运行态**：终端只用会话，不启动/停止转发，不动 `auto_start`。
- **已知限制（写进文档，不本期改）**：
  - 概览采集失败时，左侧导航清单拿不到（`modules` 唯一来源是 `overview.modules`），「用终端去救一台采集失败的机器」这条路径走不通。要修得让模块清单不依赖概览采集，属于概览模块的事，本期不做。
  - `PtyChannel::run` 的 `event_tx` 是无界通道，背压只经桥的 `socket.send().await` 间接生效；隐藏的终端里跑 `yes` / `dmesg -w` 会把内存顶起来（容器控制台同病）。本期与容器保持一致，不改通道类型。

## 明确不做

多标签、root/su 档位、SFTP 文件传输、会话恢复（断开后保持 shell 存活）、命令历史与搜索、字号/主题/录屏、Windows 远端适配（不专门探测，交给远端报错）、移动端、模块清单与概览解耦。

## 验收

1. 隧道已连接 → 点「终端」→ 3 秒内出现提示符，起始目录是登录用户家目录。
2. 切到「概览」再切回「终端」→ 仍是同一个会话（进页前 `cd /tmp`，回来仍在 `/tmp`，无重连痕迹）。
3. **会话释放**：进入终端时 `echo $$` 记下 PID，返回隧道页后在远端 `ps -p <pid>` 无输出。（不用 `grep "[b]ash -l"`：登录 bash 会把 `argv[0]` 改写成 `-bash`，那条判据恒为 0，永远假通过。）
4. 在终端页点右上角「刷新」把概览打挂（或用一台概览采集失败的机器）→ 终端会话**不受影响**，仍在原处。（概览失败时导航清单为空、终端入口不出现，属已知限制，见边界。）
5. 隧道未建立 → 点「终端」→ 自动建会话后可用；建不起来时终端上方给出与隧道页一致的失败原因。
6. 远端没有 bash（如 alpine）→ 自动退到 `sh`，仍有提示符。
7. `vim` / `top` / `htop` 全屏程序排版正常，`Ctrl+C` 能中断当前命令；窗口缩放后排版跟着重排；**切走再切回后排版不变成 20 列**。
8. 终端里 `exit` 或 `Ctrl+D` → 状态「已结束」+ 提示，点「重开」可再进。
9. 关掉核心（或重启 App）→ 终端显示已结束；核心恢复后**第一次**点「重开」就能连上（token 已刷新）。
10. 容器控制台回归：共用件替换后，exec / root 两档仍可用，同一容器第二条会话仍被拒。
11. **页头切换器**：列表不含「本机」条目，当前项高亮；在「容器管理」页切到 devb → 仍停在容器管理且显示 devb 的容器；切回 deva 正常；`Esc` 与点外部都能关闭浮层。
12. **终端切服务器**：在终端页切到 devb → 旧 shell 退出（`ps -p <旧 PID>` 无输出）、新 shell 在 3 秒内出现提示符；切回 deva 同样成立。
13. `bash scripts/verify.sh` 全绿（两套 workspace + 前端 + msocheck）。

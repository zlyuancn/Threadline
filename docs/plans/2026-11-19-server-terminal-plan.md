# 服务器终端 实施计划

> **给执行者**：REQUIRED SUB-SKILL — `superpowers:executing-plans`，按任务顺序逐条执行。
> 设计与交互已定稿：`docs/plans/2026-11-19-server-terminal-design.md`（2026-11-19 经独立审查修订）。
> 本计划只回答"先写哪个文件、写到什么程度"。

**目标**：管理页「终端」模块从 `Planned` 转 `Ready`——浏览器内一条 WS 直连远端 PTY，拿到登录 shell。

**范围**：单个终端会话、单个登录 shell。多标签 / root 档 / 文件传输 / 会话恢复不做。

**技术栈**：Rust（tl-core / axum / russh，**不新增 crate**）、Vue 3 + TypeScript、xterm.js 5.5.0（已在 `package.json`）。

**复用面**：`engine::open_pty`、interface 的帧协议、`ContainerConsole.vue` 的 xterm 处理。核心工作量是**收敛重复逻辑**（后端 `pty_bridge`、后端 shell 选择、前端纯件），不是新写一套。

---

## 前置约束

1. **只在本地 commit，不 push**（决策 0003）。
2. 分层：壳 → 接口 → 业务 → 模型 → 底座。命令构造在 `models/server_admin`；`business` 只编排；`interface` 只做参数校验、WS 桥与错误翻译。
3. WS 升级后的失败**只能发 `error` 帧**（已 101，改不了状态码）；路由级参数校验错误才走 `interface/error.rs`。
4. 前端 `views/` 不直连 HTTP（msocheck R3），WS 构造放 `src/api/`，可复用纯逻辑放 `src/models/`。
5. **测试环境是 `environment: 'node'`（`vite.config.ts:28-31`），没有 jsdom、没有 `@vue/test-utils`**：不新增这两个依赖，只测**纯函数**（帧解析、状态归约、命令字符串）。任何 `new Terminal()` / `document` 相关逻辑都不写单测，靠真机冒烟。
6. 每个 Task 末尾跑对应测试；阶段结束跑 `bash scripts/verify.sh`（两套 workspace + 前端 + msocheck）。**跑之前先关掉正在运行的 Threadline**（`verify.ps1:55-58`），且 `node_modules` 必须在位。
7. `pty_bridge` 的抽取必须是**一次纯搬运提交**（不带任何行为改动），便于容器控制台退化时 bisect。

---

## 阶段 A：核心侧

### Task A1：shell 选择规则收敛 + 终端命令

**Files:**
- Create: `crates/tl-core/src/models/server_admin/shell.rs`
- Create: `crates/tl-core/src/models/server_admin/terminal.rs`
- Modify: `crates/tl-core/src/models/server_admin/containers.rs`（`PICK_SHELL` 改为调用共用函数）
- Modify: `crates/tl-core/src/models/server_admin/mod.rs`（模块声明、再导出、`terminal` 状态）

**Step 1：`shell.rs`（唯一规则）**

```rust
//! 交互 shell 的选择规则。
//!
//! 归属：**模型层**。容器控制台与服务器终端共用同一条规则——
//! 两处各写一份 `command -v bash …` 迟早会漂移（比如都该加 `-i` 时只改了一边）。

/// 选 shell 的包装命令。`login = true` 用登录 shell（读 profile，PATH 完整）。
///
/// 整段是双引号包裹的**单个参数**，内部没有 `$` / 反引号 / `!`，
/// 各家登录 shell（csh / fish / zsh / dash）都会原样交给内层 `sh`。
/// **不要在这里加任何插值**：`$USER`、`cd $DIR` 在 csh/fish 下会被提前展开。
pub fn pick_shell(login: bool) -> String {
    let flag = if login { " -l" } else { "" };
    format!("sh -c \"command -v bash >/dev/null 2>&1 && exec bash{flag} || exec sh\"")
}
```

**Step 2：容器侧改调用（字符串必须逐字不变）**

`containers.rs` 删掉 `const PICK_SHELL`，改为 `let shell = pick_shell(false);` 后拼进 `format!`。
单测断言：`pick_shell(false)` 与旧常量**完全相同**（把旧串写成断言里的字面量），`console_command(...)` 的现有断言全部保持通过。

**Step 3：`terminal.rs`**

```rust
//! 服务器终端：登录 shell 的启动命令。
//!
//! 只拥有"用什么命令进这台机器"这一条规则，**不接收任何参数**：
//! 前端传进来的字符串一律不参与拼接，否则这条 WS 就成了通用"任意命令执行"接口。

use super::shell::pick_shell;

/// 登录 shell：bash 优先（`-l` 读 profile），没有就 sh。
pub fn login_shell_command() -> String {
    pick_shell(true)
}
```

**Step 4：`mod.rs`**

- 模块声明：`pub mod shell;` `pub mod terminal;`（`collect / containers / parse` 之后）
- 再导出：`pub use terminal::login_shell_command;`（`shell::pick_shell` 由 `containers` 直接 `use`，不必再导出）
- `modules()` 里 `terminal`：`AdminModuleState::Planned` → `Ready`，说明改为 `"登录 shell（bash 优先），离开管理页即断开"`
- **导航顺序已按 2026-11-19 拍板排好**（概览 / 终端 / 文件 / 容器 / 虚拟机 / 进程 / 网络 / 防火墙 / 服务 / 日志），vec 顺序即导航顺序，顺带加了 `module_order_matches_the_agreed_navigation` 断言；本 Task 不要再动顺序
- 文件头注释补一句：`//! - **采集只读**：概览/容器等采集脚本里没有写操作；终端是用户自己的交互通道，不属于"核心替用户写远端"。`

**Step 5：单测**（`shell.rs` / `terminal.rs` 各一个 `mod tests`）

- `pick_shell(false)` == 旧常量字面量；`pick_shell(true)` 含 `exec bash -l`；
- 两条命令都**不含** `$`、反引号、`!`、单引号；
- `login_shell_command()` 等于 `pick_shell(true)`；
- `modules()` 里 `terminal.state == Ready` 且 `note` 非空（现有 `planned_modules_still_carry_a_note` 不受影响，它只过滤非 Ready）。

**验证**：`cargo test -p tl-core server_admin` + `cargo build -p tl-core`

---

### Task A2：业务编排

**Files:**
- Modify: `crates/tl-core/src/business/admin.rs`
- Modify: `crates/tl-core/src/business/mod.rs`

**Step 1：会话获取改通用名**

`container_session()` → `admin_session()`。**它是 7 处调用**（定义行 + 6 个容器函数；另 `overview()` 里有同样逻辑），用 `grep -n "container_session" crates/` 全部替换。顺手把 `overview()` 里"查存在 + 取会话"的重复代码也换成 `admin_session()`。

**Step 2：新增编排**

```rust
/// 开一条服务器登录 shell。
///
/// 不占占用表：SSH shell 各自独立 PTY，两条会话不会串输入
/// （容器控制台要互斥，是因为 `docker exec` 共享同一个容器 TTY）。
pub async fn server_terminal(
    servers: &ServerStore,
    runner: &Arc<TunnelRunner>,
    server_id: &str,
    cols: u32,
    rows: u32,
) -> Result<(engine::PtyChannel, String)> {
    let session = admin_session(servers, runner, server_id).await?;
    let command = server_admin::login_shell_command();
    let pty = engine::open_pty(&session, &command, cols, rows)
        .await
        .map_err(|e| anyhow!("{e}"))?;
    Ok((pty, command))
}
```

**Step 3：`Services::server_terminal`**（`business/mod.rs`，紧挨 `container_console`）

```rust
pub async fn server_terminal(
    &self, server_id: &str, cols: u32, rows: u32,
) -> Result<(crate::models::tunnel::engine::PtyChannel, String)> {
    admin::server_terminal(&self.servers, &self.runner, server_id, cols, rows).await
}
```

**验证**：`cargo test -p tl-core` + `cargo clippy -p tl-core --all-targets -- -D warnings`

---

### Task A3：接口、鉴权与共用桥

**Files:**
- Modify: `crates/tl-core/src/interface/mod.rs`
- Modify: `crates/tl-core/src/interface/auth.rs`

**Step 1：抽 `pty_bridge`（纯搬运，单独提交）**

把 `console_loop` 的收发循环（现 `interface/mod.rs:659-720`）搬成：

```rust
/// PTY ↔ WebSocket 的双向桥。打开方式由调用方给，循环本身两处一致。
///
/// `opened` 的第三项是容器控制台的占用凭据：桥内 `let _guard = guard;`
/// 持有到函数结束，`Drop` 时机与改造前的 `_guard` 绑定等价。
async fn pty_bridge(
    mut socket: WebSocket,
    opened: Result<(PtyChannel, String, Option<ConsoleGuard>), String>,
    mode_label: &'static str,
) { /* 原循环逐字搬入：hello → select! 收发 → drop 指令端 → 等 PTY 任务 → close */ }
```

`console_loop` 变为：

```rust
async fn console_loop(socket, state, server_id, cid, mode, cols, rows) {
    let opened = state.services
        .container_console(&server_id, &cid, mode, cols, rows).await
        .map(|(pty, command, guard)| (pty, command, Some(guard)))
        .map_err(|e| format!("{e:#}"));
    pty_bridge(socket, opened, mode.as_str()).await;
}
```

**本步不提交行为改动**：改完先跑 `cargo test -p tl-core`，再单独 commit。

**Step 2：终端路由**

```rust
.route(
    &format!("{API_PREFIX}/servers/{{id}}/admin/terminal"),
    get(server_terminal),
)
```

```rust
#[derive(Debug, Default, Deserialize)]
struct TerminalQuery { cols: Option<u32>, rows: Option<u32> }

async fn server_terminal(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    Query(query): Query<TerminalQuery>,
) -> Response {
    let cols = clamp_cols(query.cols.unwrap_or(100));
    let rows = clamp_rows(query.rows.unwrap_or(30));
    ws.on_upgrade(move |socket| async move {
        let opened = state.services.server_terminal(&id, cols, rows).await
            .map(|(pty, command)| (pty, command, None))
            .map_err(|e| format!("{e:#}"));
        pty_bridge(socket, opened, "shell").await;
    })
}
```

**Step 3：鉴权白名单**

`auth.rs` 的 `console_query_token` 改名 `pty_query_token`：

```rust
/// 浏览器不能给 WebSocket 设请求头。只有这两条 WS 吃 `?token=`：
/// 容器控制台 `/admin/containers/{cid}/console`、服务器终端 `/admin/terminal`。
fn is_pty_route(path: &str) -> bool {
    path.ends_with("/console") || path.ends_with("/admin/terminal")
}
```

**Step 4：测试（函数级 + 路由级，两层都要）**

- `auth.rs`：`/admin/terminal?token=abc` 取得到；`/admin/terminals?token=abc` 取不到；`/admin/terminal` 无 token 取不到。
- `interface/mod.rs`：照现有 `console_route_accepts_query_token_only_with_token`（`:1667-1701`）写一条 `terminal_route_accepts_query_token_only_with_token`——**无 token → 401；带 token → 400（不是升级请求）**。只测判断函数不测路由，路由挂错到 `public` 时测试照样全绿。

**验证**：`cargo test -p tl-core` + `cargo clippy -p tl-core --all-targets -- -D warnings`

---

## 阶段 B：前端

### Task B1：API

**Files:** Modify `src/api/index.ts`

```ts
/**
 * 服务器终端。令牌走 query（白名单见 `interface/auth.rs` 的 `pty_query_token`）。
 *
 * 先刷新一次 bootstrap：核心每次启动都轮换 token，停在终端页时全局轮询
 * 不保证已刷新，否则核心重启后第一次「重开」会撞 401，被误报成"连不上核心"。
 */
openServerTerminal: async (id: string, cols: number, rows: number): Promise<WebSocket> => {
  await refreshBootstrap();
  /* 与 openConsole 同构，路径换 /admin/terminal */
},
```

顺手更新 `openConsole` 上方那句已经过期的注释（「核心只对 `/console` 开这个口子」），并把同样的 `refreshBootstrap()` 加给它。

**验证**：`pnpm typecheck`

---

### Task B2：前端纯件 + 单测

**Files:**
- Create: `src/models/ptySession.ts`
- Create: `src/models/ptySession.test.ts`

内容（**不碰 DOM、不 new Terminal**）：

```ts
export const PTY_THEME = { /* 从 ContainerConsole.vue 原样搬出 */ };

export type PtyStatus = 'idle' | 'connecting' | 'open' | 'closed' | 'error';

/** 服务端文本帧 → 结构化事件；非 JSON 返回 null（当普通输出处理）。 */
export function parseServerFrame(raw: string): { type: 'hello'; command: string }
  | { type: 'exit'; code: number }
  | { type: 'error'; message: string }
  | null;

/** 状态归约：error 一旦出现不被后续 close 覆盖。 */
export function reducePtyState(state: PtyStatus, event: 'connect' | 'hello' | 'exit' | 'error' | 'close'): PtyStatus;

/** xterm 实例 + fit addon 的创建/销毁（只在浏览器里调用，不写单测）。 */
export function createTerminal(host: HTMLElement, opts?: { fontSize?: number }): {
  term: Terminal; fit: () => void; dispose: () => void;
};
```

单测覆盖：
- `parseServerFrame` 四分支（hello / exit / error / 非 JSON 与畸形 JSON）；
- `reducePtyState`：`connecting → open → closed`；`open` 之后 `close` 得 `closed`；`error` 后 `close` 仍是 `error`；`idle` 收到 `close` 不变。

**验证**：`pnpm test -- ptySession`

---

### Task B3：终端面板

**Files:** Create `src/views/admin/TerminalPanel.vue`

- 结构：工具栏（状态 · 启动命令 · 清除 · 重开）+ `<div ref="host" class="tp-term">` + 底部说明。
- 自己的 `connect()` / `teardown()`（不复用容器控制台的，原因见设计）：`seq` 防重入、`teardown()` 先发 `close` 帧再 `close()`、`onBeforeUnmount` 时断开。
- **尺寸守卫（关键）**：`display:none` 下 `fit()` 会得到 2×1（FitAddon 只在 cell 尺寸为 0 时返回 undefined），因此 `ResizeObserver` 回调里先判 `if (!el.offsetParent || el.offsetWidth === 0) return;`；父级把面板 `v-show` 切回来时显式补一次 `fit()` + `term.refresh(0, term.rows - 1)`。**不写这条，切走再切回就是一堆 20 列乱行**。
- `canConnect: () => true`（隧道不存在时核心在 `error` 帧里给原因）。
- 高度：`.tp { display:flex; flex-direction:column; min-height:0 }`、`.tp-term { flex:1; min-height:320px }`。
- 状态文案：`connecting` → 「正在连接…」；`open` → 绿点；`closed` → 「会话已结束，点重开」；`error` → 红条 + 重试。
- 终端底色沿用 `#0a171a`，与现有卡片同色温。

**验证**：`pnpm typecheck`

---

### Task B4：管理页接线（并列渲染 + 保挂载）

**Files:** Modify `src/views/ServerAdminView.vue`

**关键：终端面板必须与概览的三态链并列**，不能挂在 `v-else-if="overview"` 里——否则在终端页点一次「刷新」把概览打挂，整个分支被替换成「采集失败」块，面板卸载、会话断掉。

```ts
const TerminalPanel = defineAsyncComponent(() => import('./admin/TerminalPanel.vue'));
const terminalMounted = ref(false);
watch(active, (id) => { if (id === 'terminal') terminalMounted.value = true; });
const isPlaceholder = computed(() => !['overview', 'containers', 'terminal'].includes(active.value));
```

```vue
<section class="admin-main" :class="{ 'is-terminal': active === 'terminal' }">
  <!-- 概览三态：限定 active === 'overview'，不再包住其它模块 -->
  <div v-if="active === 'overview' && admin.error" class="admin-error"> … </div>
  <div v-else-if="active === 'overview' && admin.loading && !overview" class="admin-loading"> … </div>
  <template v-else-if="active === 'overview' && overview"> … </template>

  <ContainersPanel v-if="active === 'containers'" :server-id="serverId" />
  <TerminalPanel
    v-if="terminalMounted"
    :key="serverId"
    v-show="active === 'terminal'"
    :server-id="serverId"
  />
  <section v-if="isPlaceholder" class="panel admin-placeholder"> … </section>
</section>
```

- `:key="serverId"` 强制换服务器时重建，避免旧 xterm 缓冲与旧 WS 跨服务器复用。
- 样式：**不要动全局 `.admin-main`**（`src/styles.css:2872-2877`，概览与容器都靠它）。只在终端态挂修饰类：`.admin-main.is-terminal { display:flex; flex-direction:column; overflow:hidden; padding:0 }`。
- 确认 `watching()` 仍含 `active === 'overview'`（现有 `:154-156`），切到终端概览轮询自动停、切回自动续。

**验证**：`pnpm typecheck && pnpm build`（xterm 仍只出现在异步 chunk，主 chunk 体积不涨）

---

### Task B5：页头服务器切换器

**Files:**
- Modify: `src/views/ServerAdminView.vue`（页头）
- Modify: `src/App.vue`（接线一行）
- Modify: `src/models/serverAdmin.ts`（加纯函数）
- Modify: `src/models/serverAdmin.test.ts`（补测试）

**Step 1：纯函数（可测部分）**

```ts
/**
 * 页头下拉的可选项：剔除「本机」（没有会话，管理页进不去），
 * 按名称或 host 过滤。关键词去空格、忽略大小写。
 */
export function pickableServers<T extends { name: string; host: string; kind?: string }>(
  servers: T[],
  keyword: string,
): T[] {
  const term = keyword.trim().toLowerCase();
  return servers
    .filter((server) => server.kind !== 'local')
    .filter((server) => !term || server.name.toLowerCase().includes(term) || server.host.toLowerCase().includes(term));
}
```

单测：剔除 `local`；空关键词返回全部；按名称命中；按 host 命中；大小写与首尾空格不敏感。

**Step 2：页头交互**

- `.admin-id` 加 `position: relative`，名称区变成 `button`（`aria-expanded`、`aria-haspopup="listbox"`），带一个 `chevron-down`。
- 浮层：宽 320px、绝对定位、`z-index` 高于内容区；`max-height: 60vh; overflow-y: auto`。
- 每项：名称 + `user@host:port`（`mono` 小字）+ 状态点（复用侧栏三态：`servers.sessionConnected(id)` → `is-on`；`servers.sessionKnownOf(id)` 为 false → `is-warn`；否则无点）+ 当前项勾选。
- 关闭：点浮层外部（`pointerdown` + `wrapper.contains` 判断）、`Esc`、选中后自动关。
- 过滤框：`servers.length > 6` 时才渲染，`autofocus`。
- 选中：`emit('switch-server', id)`，**自己不改 `props.serverId`**（真相在 `App.vue`）。

**Step 3：App.vue 接线**

```vue
<ServerAdminView
  v-else-if="tab === 'admin'"
  :server-id="adminServerId"
  @switch-server="adminServerId = $event"
  @back="tab = 'forwards'"
/>
```

侧栏高亮不用改：`tunnelActive()` 已经按 `adminServerId` 判定（`App.vue:189-194`）。

**Step 4：切换后的连锁（确认已有逻辑，不新写）**

- 概览：`watch(() => props.serverId, load)`（`ServerAdminView.vue:269`）+ `admin.load()` 的「id 变了先清 `overview`」（`models/serverAdmin.ts:39-42`）已经覆盖。
- 容器：`ContainersPanel` 已 `watch(props.serverId)`（`:248-253`）关抽屉并重载。
- 终端：B4 的 `:key="serverId"` 会重建面板（旧 WS 断、新 WS 连）。
- `active` 不动——这就是"停在当前模块"。

**验证**：`pnpm test -- serverAdmin` + `pnpm typecheck`；手动：概览页切另一台，指标换机器且旧数字不闪；容器页切另一台，抽屉关闭、列表换机器；终端页切另一台，旧 shell 退出、新 shell 出现。

---

### Task B6：容器控制台只换纯件（可选，回归不过就整条回退）

**Files:** Modify `src/views/admin/ContainerConsole.vue`

- 只替换三处：`THEME` → `PTY_THEME`、手写 JSON 解析 → `parseServerFrame`、`new Terminal(...)` + `FitAddon` → `createTerminal(...)`。
- **生命周期逻辑（`connect` / `teardown` / `seq` / `watch`）一行不动**。抽屉是 `v-if`，不存在 `display:none` 里的 fit 问题，不需要可见性守卫；把两套策略合并只会把已验证路径带坏。
- 顺带给它的 `openConsole` 补上 async `refreshBootstrap`（B1 已改签名）。

**验证**：`pnpm test` + `pnpm build`；真机回归：进任一自建测试容器，exec 与 root 两档各开一次，`echo hi` 有回显，关抽屉后远端无残留 `docker exec`。

---

## 阶段 C：收口

### Task C1：文档

- `docs/TODO.md`：勾掉「P2 终端」，注明完成日期与门禁结果；路线图状态改「完成」（收口时整条搬 `DONE.md`）。
- `docs/TESTPLAN.md`：新增「服务器管理 · 终端」段，抄设计「验收」1–12 条（注意第 3 条的 PID 判据，不要用 `grep "bash -l"`）。
- `docs/plans/2026-11-16-server-admin-design.md`：P2 表格 `terminal` 从「占位」改「✅ 完整（见 2026-11-19-server-terminal-design.md）」。
- `src/api/index.ts` 里「只对 `/console` 开口」的注释随 B1 一起改（C1 复核）。
- `docs/DONE.md`：收口时整条搬入（带日期、门禁结果）。

### Task C2：门禁

**先关掉正在运行的 Threadline**，然后：

```
bash scripts/verify.sh
```

全绿标准：根 workspace 与 `src-tauri` 两套 fmt/clippy/test + 前端 `typecheck`/`test`/`build` + msocheck。

### Task C3：真机冒烟

按设计「验收」逐条验，重点六条：

1. `cd /tmp` → 切概览 → 切回终端，仍在 `/tmp`；
2. 返回隧道页 → `ps -p <进终端时记下的 PID>` 无输出；
3. 终端页点「刷新」把概览打挂 → 会话不断；
4. `vim` / `top` 排版正常，切走再切回不出现 20 列乱行；
5. 无 bash 的机器（alpine）退到 `sh` 仍有提示符；
6. 页头下拉切到另一台：仍停在当前模块、数据换成新机器、终端旧 shell 退出且新 shell 起来。

结果记入 `docs/TESTPLAN.md`，视情况写 `docs/archive/` 战报。

---

## 任务依赖

```
A1 → A2 → A3（pty_bridge 单独提交）→ B1 → B2 → B3 → B4 → B5 → C2 门禁 → C3 真机
                                                           └→ B6（可与 C3 同批，回归不过就回退）
C1 贯穿：每个 Task 收口就补对应文档条目
```

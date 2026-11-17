# 容器管理 实施计划

> **给执行者**：REQUIRED SUB-SKILL — `superpowers:executing-plans`，按任务顺序逐条执行。
> 设计与交互已定稿：`docs/plans/2026-11-17-container-admin-design.md`；本计划只回答"先写哪个文件、写到什么程度"。

**目标**：服务器管理页新增「容器管理」模块——列表（两档密度）、详情抽屉、日志跟随、启动 / 停止 / 重启、删除（可勾选删镜像）、控制台（exec 进容器）。

**范围**：引擎只认 `docker`（`podman` / `nerdctl` 留到以后，探测点已预留）。虚拟机不做，见设计文档「虚拟机管理（规划）」。

**技术栈**：Rust（tl-core / tl-protocol / axum / russh）、Vue 3 + TypeScript + Pinia、xterm.js（仅阶段 C）。

**进度（2026-11-17）**：阶段 A（只读）、B（动作 / 删除）、C（控制台）**全部完成**，控制台渲染层已是 xterm.js。
真机验收已在隧道 `devb`（18 个容器、docker 27.5.1）上跑完三轮，全过：

| 轮次 | 项数 | 结果 |
|---|---|---|
| 只读（列表 / 资源 / 详情 / 日志 / 注入） | 18 | 全过 |
| 控制台（exec 双向 / root 档 / 单会话占用 / 非法 mode） | 4 | 全过 |
| 写操作（stop / start / pause / unpause / restart 拒绝 / force 删除 / 删镜像两分支） | 13 | 全过 |
| 移除暂停后回归（`pause` 报 400、菜单只剩重启 / 停止） | 3 | 全过 |
| debug 档（调试图进容器） | 3 | 全过（需先让本机有 busybox） |
| 删镜像新顺序（先删容器 → 查占用 → 再 rmi） | 8 | 全过 |

写操作全部在**自建的 `tl-wt-*` 容器**上做，镜像用 `redis:8.10.3` 的副本 tag `tl-redis-wt:tmp`——删镜像删掉的是副本 tag，原镜像始终完好（验后核对 `sha256:1f8829…` 未变）。用户的 18 个容器一个没动。

真机一共暴露 5 个只有实机才看得见的缺陷，都已修并补了回归测试：

1. `ps --no-trunc` 给 64 位 id，`stats` 给 12 位，两档数据合并只能靠前缀猜（18 条资源一条都没对上）
2. `c_detail` 主数据行（命令里带 `=`）被当成第一条环境变量混进 env
3. `allowed_actions("running")` 漏了 `Pause`，「暂停」在任何状态下都不可达
4. `docker rmi` 删的是 tag，共用同一 tag 时不会报错，会把别人的镜像引用删成悬空
5. 「状态不允许」「容器正在运行」报成 500，应为 409

**debug 档未验**（会拉 busybox 镜像，属于写操作）。
`cargo test -p tl-core` 239 项全过（含新增 21 项容器 / 控制台 / 鉴权单测），`clippy` 无警告，前端 136 项（新增 26 项：展示口径 17 + store 合并/筛选/写操作 9）与 `vue-tsc` / `vite build` / `msocheck` 均通过。
xterm 走异步组件：独立 chunk 296.53 kB / gzip 75.13 kB，主 chunk 维持 401 kB（gzip），打开控制台页签才下载。
完整的 `scripts/verify.sh` 还差 agent 侧一遍，需先关掉正在运行的 Threadline。真机冒烟（验收 1–12 条）待有远端 docker 的环境。

---

## 前置约束

1. **只在本地 commit，不 push**。
2. 每个 Task 末尾跑对应测试；阶段结束时跑 `bash scripts/verify.sh`（两套 workspace + 前端 + msocheck）。
3. 分层：壳 → 接口 → 业务 → 模型 → 底座。容器规则（脚本、解析、动作白名单、状态机）**全部**在 `models/server_admin/containers.rs`；`business` 只编排；`interface` 只校验参数与翻译错误。
4. 脚本一律 `sh -c '...'` 包裹，**正文不得出现单引号**；采集脚本必须只读（单测断言）。
5. 错误消息要命中 `interface/error.rs` 的关键词映射：400（不能为空 / 必须 / 无效 / 格式）、409（已存在 / 已被 / 占用 / 未启用 / 无法 / 待确认），否则落 500。
6. 前端 `views/` 不直连 HTTP（msocheck R3），请求封装在 `src/api/`，状态与轮询在 `src/models/`。
7. `cid`（容器 id 或名字）**双重校验**：正则 `^[A-Za-z0-9][A-Za-z0-9_.-]{0,79}$` + 必须出现在本次会话采集到的清单里。

---

## 阶段 A：只读（P3a）

### Task A1：契约先行

**Files:**
- Modify: `crates/tl-protocol/src/lib.rs`
- Modify: `src/api/types.ts`

**Step 1：DTO**

```rust
/// 列表里的一行。字段比概览用的 `ContainerRow` 全：概览只要 5 个字段，
/// 管理页要端口、卷、网络、资源与创建时间。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ContainerItem {
    pub id: String,
    pub name: String,
    pub image: String,
    pub state: String,        // running / exited / paused / restarting / created
    pub status: String,       // Up 3 days (healthy) / Exited (1) 4 hours ago
    #[serde(default)] pub health: Option<String>,   // healthy / unhealthy / none
    #[serde(default)] pub compose_project: Option<String>,
    #[serde(default)] pub created_at: String,       // 2026-11-03 21:04
    #[serde(default)] pub exit_code: Option<i32>,
    #[serde(default)] pub restarts: u32,
    #[serde(default)] pub ports: Vec<ContainerPort>,
    #[serde(default)] pub mounts: Vec<ContainerMount>,
    #[serde(default)] pub networks: Vec<ContainerNetwork>,
    #[serde(default)] pub cpu_percent: f64,
    #[serde(default)] pub mem_bytes: u64,
    #[serde(default)] pub mem_limit_bytes: u64,
    #[serde(default)] pub net_rx_bps: f64,
    #[serde(default)] pub net_tx_bps: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerPort {
    pub host_ip: String,       // 0.0.0.0 / 127.0.0.1
    pub host_port: u16,
    pub container_port: u16,
    pub proto: String,         // tcp / udp
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerMount {
    pub source: String,        // 宿主路径；匿名卷时是卷名
    pub destination: String,
    pub mode: String,          // rw / ro
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerNetwork {
    pub name: String,
    pub ip: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ContainerList {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    pub total: usize,
    pub running: usize,
    pub exited: usize,
    pub paused: usize,
    pub restarting: usize,
    #[serde(default)] pub items: Vec<ContainerItem>,
    #[serde(default)] pub collected_at: String,
    #[serde(default)] pub elapsed_ms: u64,
    /// 采集失败的块，例如 `资源` / `网络与卷`。
    #[serde(default)] pub degraded: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ContainerDetail {
    pub item: ContainerItem,
    pub command: String,
    pub restart_policy: String,
    #[serde(default)] pub env: Vec<ContainerEnv>,
    #[serde(default)] pub networks: Vec<ContainerNetwork>,
    #[serde(default)] pub mounts: Vec<ContainerMount>,
    #[serde(default)] pub ports: Vec<ContainerPort>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerEnv {
    pub key: String,
    pub value: String,        // 敏感键（含 PASSWORD / SECRET / TOKEN / KEY）由核心打码
    pub masked: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ContainerLogs {
    pub lines: Vec<String>,
    pub truncated: bool,
    pub next_since: String,   // rfc3339，跟随用
}
```

**Step 2：前端镜像**

```ts
export interface ContainerItem { /* 字段一一对应，数字用 number */ }
export type ContainerPort = { host_ip: string; host_port: number; container_port: number; proto: string };
export type ContainerMount = { source: string; destination: string; mode: string };
export type ContainerNetwork = { name: string; ip: string };
export interface ContainerList { engine?: string; total: number; running: number; exited: number; paused: number; restarting: number; items: ContainerItem[]; collected_at: string; elapsed_ms: number; degraded: string[] }
```

**验证**：`cargo test -p tl-protocol && pnpm typecheck`

**commit**：`feat(protocol): 容器管理契约`

---

### Task A2：模型 —— 脚本与解析

**Files:**
- Create: `crates/tl-core/src/models/server_admin/containers.rs`
- Modify: `crates/tl-core/src/models/server_admin/mod.rs`（`pub mod containers;`）

**Step 1：三个采集脚本**（都用 `Probe` 同款"一次往返"形态，节名 `@@c_*`）

| 函数 | 命令 | 何时跑 |
|---|---|---|
| `list_script()` | `docker ps -a --no-trunc --format '<id>\|<names>\|<image>\|<state>\|<status>\|<ports>\|<created>\|<label compose.project>'` | 5s |
| `inspect_script(cid)` | `docker inspect --format '<json>' <cid>`（只取 .State / .Config / .NetworkSettings / .Mounts / .HostConfig.RestartPolicy） | 详情抽屉、10s 批量 |
| `stats_script()` | `docker stats --no-stream --format '<id>\|<cpupct>\|<memusage>\|<memperc>\|<netio>\|<blockio>'` | 只在抽屉打开 / 显式要资源时 |

批量拿网络与卷（10s 一档）：

```
docker inspect --format '<name>|<range .NetworkSettings.Networks><key>=<ip>,<end>|<range .Mounts><source>=><destination>:<mode>,<end>' $(docker ps -aq)
```

**Step 2：解析**（纯函数，容错优先）

```rust
pub fn parse_list(raw: &str) -> ContainerList;
pub fn parse_inspect(raw: &str) -> Option<(Vec<ContainerPort>, Vec<ContainerMount>, Vec<ContainerNetwork>, Vec<ContainerEnv>)>;
pub fn merge_inspect(list: &mut ContainerList, raw: &str);   // 10s 那档把网络/卷补进列表
```

要点：
- `ports` 字段形如 `0.0.0.0:2283->2283/tcp, [::]:2283->2283/tcp`：拆成结构化、**去掉 IPv6 重复项**（`[::]` 与 `0.0.0.0` 同一条映射只留一条）。
- `created` 的 docker 输出是 `2026-11-03 21:04:05 +0800 CST`，只留 `YYYY-MM-DD HH:MM`。
- 解析失败的块进 `degraded`，不打空整页。
- 单引号纪律：脚本正文用 `|` 与 `,` 分隔，不用引号。

**Step 3：单测**（与 `parse.rs` 同风格，内联 `SAMPLE` 常量）

- 端口解析：`0.0.0.0:2283->2283/tcp` 拆对、IPv6 去重、无端口时不 panic。
- 卷解析：`/mnt/user/appdata/immich=>/data:rw` → `{source, destination, mode}`。
- 网络解析：多网络 `immich-net=172.18.0.4,` 逐项拆开。
- 状态计数：`total/running/exited/paused/restarting` 与 items 一致。
- 脚本纪律：每段 `strip_prefix("sh -c '")` 后断言无单引号、无 `rm `/`kill `。
- `__no_engine__` → `engine: None`，不记 degraded。

**commit**：`feat(admin): 容器列表脚本与解析`

---

### Task A3：动作白名单与状态机（模型内，先建不用）

**Files:**
- Modify: `crates/tl-core/src/models/server_admin/containers.rs`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerAction { Start, Stop, Restart, Pause, Unpause }

impl ContainerAction {
    pub fn parse(raw: &str) -> Option<Self>;
    pub fn as_docker(&self) -> &'static str;      // "start" / "stop" ...
}

/// 当前状态下允许的动作。列表菜单与后端校验共用这一份，两边不能各写一张表。
pub fn allowed_actions(state: &str) -> &'static [ContainerAction];

/// 容器 id / 名字校验：正则 + 非空 + 长度上限。
pub fn validate_cid(cid: &str) -> Result<(), String>;

/// 动作脚本：`docker <verb> <cid>`，超时单独给 20s。
pub fn action_script(action: ContainerAction, cid: &str) -> String;
/// 删除脚本：force 时 `docker rm -f`，image=1 时在删除后追加 `docker rmi <image>`（失败不吞）。
pub fn remove_script(cid: &str, force: bool, image: Option<&str>) -> String;
```

单测：
- `allowed_actions("exited")` 只含 `Start`；`running` 含 `Stop/Restart`；`paused` 含 `Unpause/Stop`；`restarting` 只含 `Stop`。
- `validate_cid` 拒绝 `; rm -rf /`、`$(id)`、空格、引号、超长串。
- `action_script` 生成串不含单引号、不含 `|`。

**commit**：`feat(admin): 容器动作白名单与状态机`

---

### Task A4：业务编排 + 只读接口

**Files:**
- Modify: `crates/tl-core/src/business/admin.rs`
- Modify: `crates/tl-core/src/interface/mod.rs`
- Modify: `services` 里对 `business::admin` 的暴露（跟随现有 `server_overview` 的写法）

**Step 1：业务函数**

```rust
pub async fn containers(servers, runner, server_id) -> Result<ContainerList>;
pub async fn container_detail(servers, runner, server_id, cid) -> Result<ContainerDetail>;
pub async fn container_logs(servers, runner, server_id, cid, tail, since) -> Result<ContainerLogs>;
```

沿用 `overview()` 的骨架：确认隧道存在 → `runner.session_handle` → `exec_capture` → 解析 → 补 `collected_at`/`elapsed_ms`。**清单校验放在业务层调用模型之前**：先 `containers()` 拿清单，`cid` 不在清单里直接 404。

**Step 2：路由**

```
GET /api/v1/servers/{id}/admin/containers
GET /api/v1/servers/{id}/admin/containers/{cid}
GET /api/v1/servers/{id}/admin/containers/{cid}/logs?tail=200&since=<rfc3339>
```

错误翻译：容器不存在 → 404（消息含"不存在"）；无 docker → 409/503 走现有 `degraded` 约定，保持与概览一致。

**Step 3：验证**

`cargo test -p tl-core && cargo clippy --workspace --all-targets -- -D warnings`

**commit**：`feat(admin): 容器列表/详情/日志接口`

---

### Task A5：前端 store 与面板

**Files:**
- Create: `src/models/containers.ts`
- Create: `src/views/admin/ContainersPanel.vue`
- Modify: `src/api/index.ts`（`containersApi`）
- Modify: `src/views/ServerAdminView.vue`（`v-else-if="active === 'containers'"`）

**store（`src/models/containers.ts`）**

```ts
export const useContainersStore = defineStore('containers', () => {
  // 状态：list / detail / logs / loading / error / filter / query / density
  // 调度：列表 5s、批量 inspect 10s、stats 3s（仅抽屉打开）、日志 2s（仅跟随）
  // 判定：visible()（筛选 + 搜索）、can(action, item)
});
```

- 轮询调度照抄 `serverAdmin.ts` 的 `shouldPoll` 判据（有 serverId + 窗口焦点 + `document.hidden` 为假 + 停在容器模块）。
- **写操作后就地改状态**：`stop` 后立刻把该行 `state` 改 `exited`、计数重算，响应回来再以服务端为准纠正。
- 密度偏好 `localStorage['tl.containers.density']`，默认 `detail`。

**面板（`src/views/admin/ContainersPanel.vue`）**：按原型的结构与文案实现——
- 工具条：引擎与版本、统计、搜索、筛选三档、密度两档
- 行：`⋯` 菜单（菜单项由 `allowed_actions` 决定，前端不另写条件）
- 抽屉：详情 / 日志 / 环境（控制台留到阶段 C）
- 空态：无引擎 / 无权限 / 无匹配，三种文案分开

组件样式沿用 `src/styles.css` 里的 `.admin-*` 与 `--tint/--line/--mint` 变量，不新建主题色。

**验证**：`pnpm typecheck && pnpm build` + 手工对照原型

**commit**：`feat(admin): 容器列表面板与抽屉`

---

### Task A6：模块状态转 Ready

**Files:**
- Modify: `crates/tl-core/src/models/server_admin/mod.rs`

`containers` 的 state：有引擎且本模块已实现 → `Ready`（不再是 `Planned`）；无引擎仍 `Unavailable`，note 文案改成「未检测到 docker」。同步改三个既有单测的期望值。

**验证**：`cargo test -p tl-core` + 进页面看侧边栏容器项不再有点。

**commit**：`feat(admin): 容器模块标记为可用`

---

## 阶段 B：写操作（P3b）

### Task B1：动作接口

**Files:** `business/admin.rs`、`interface/mod.rs`

```
POST   /api/v1/servers/{id}/admin/containers/{cid}/action
body   { "action": "start" | "stop" | "restart" | "pause" | "unpause" }
→      ContainerActionResult { ok, message, state }
```

规则：
- 后端**必须**用 `allowed_actions` 再校验一次状态（前端菜单不是安全边界）。
- 动作后回读 `docker inspect -f '{{.State.Status}}'`，不拿命令退出码当成功。
- `stop` 超时（默认 10s + 余量）→ 409「容器仍在运行」。
- 失败消息要能命中 error.rs 关键词（"无法" → 409）。

**commit**：`feat(admin): 容器启停动作接口`

---

### Task B2：删除（含删镜像）

**Files:** 同上 + `containers.rs` 的 `remove_script`

```
DELETE /api/v1/servers/{id}/admin/containers/{cid}?force=0|1&image=0|1
```

- 运行中必须 `force=1`，否则 409「容器正在运行，请先停止或强制删除」。
- `image=1`：先 `rm -f` 成功，再 `rmi <image>`。`rmi` 失败**不改变** `ok=true`，`message` 写「容器已删除，镜像仍被 N 个容器使用，未删除」（N 从 `docker ps -a --filter ancestor=<image>` 数出来）。
- 永不带 `-v`。

单测：脚本串拼接、force/image 四种组合、消息文案。

**commit**：`feat(admin): 删除容器（可选删镜像）`

---

### Task B3：前端确认框与回执

**Files:** `src/models/containers.ts`、`src/views/admin/ContainersPanel.vue`

- 所有写操作走统一确认框：标题按动作、正文写明具体命令、危险动作用红色确认按钮。
- 删除框：`同时删除镜像 <tag>` 复选框（默认不勾）；运行中提示「会先强制停止」。
- 成功后顶部一条 4 秒回执（绿），失败用红条并保留服务端原文。
- 操作期间整行禁用（防连点）。

**验证**：手工点完五种动作 + 删除两分支；刷新后状态与 `docker ps` 一致。

**commit**：`feat(admin): 容器写操作确认与回执`

---

## 阶段 C：控制台（P3c）

### Task C1：`engine::exec_pty`

**Files:** `crates/tl-core/src/models/tunnel/engine.rs`

```rust
pub struct PtySession { /* channel + write half */ }

impl PtySession {
    pub async fn write(&mut self, data: &[u8]) -> Result<(), String>;
    pub async fn resize(&mut self, cols: u32, rows: u32) -> Result<(), String>;
    pub async fn read_chunk(&mut self) -> Option<Vec<u8>>;
}

/// 与 `exec_capture` 并存：它是"跑完收结果"，这条是"开一条交互通道"。
pub async fn exec_pty(
    session: &SessionHandle,
    command: &str,
    cols: u32,
    rows: u32,
) -> Result<PtySession, String>;
```

要点：`channel_open_session` → `request_pty("xterm-256color", cols, rows, ...)` → `exec(command)` → 读写循环。PTY 的 EOF 与 exit status 都要能区分（退出码 vs 断开）。

**验证**：单测用假 command 跑不通（需要真 server），改为**冒烟脚本**：本机 sshd 跑 `sh`，断言能写入 `echo hi` 并读回 `hi`。没有条件的机器跳过。

**commit**：`feat(tunnel): 交互式 PTY 通道`

---

### Task C2：控制台桥接与 WebSocket

**Files:** `business/admin.rs`、`interface/mod.rs`、`containers.rs`（命令映射）

```
GET /api/v1/servers/{id}/admin/containers/{cid}/exec?mode=exec|root|debug   （WebSocket 升级）
帧：← {"type":"hello","shell":"/bin/bash"} | {"type":"data","data":"…"} | {"type":"exit","code":0}
    → {"type":"input","data":"…"} | {"type":"resize","cols":100,"rows":30} | {"type":"close"}
```

- 命令映射在模型里：`exec` → `docker exec -it <cid> /bin/bash`；`root` → 加 `-u root`；`debug` → `docker run --rm -it --pid=container:… --net=container:… --volumes-from:… busybox sh`。
- shell 探测：`docker exec <cid> sh -c 'command -v bash'`（注意单引号在外层已被 `sh -c` 占用 → 用 `docker exec <cid> which bash`，逐个试 bash/sh/ash）。
- 非 `running` 直接拒绝（409），不建通道。
- 一个容器同时只允许一条控制台：内存里一张 `HashMap<cid, 会话标记>`，进程内并发即可（核心单实例）。
- 抽屉关闭 / 断开隧道 → 先发 `close` 再断。

**commit**：`feat(admin): 容器控制台 WebSocket`

---

### Task C3：前端 xterm 与控制台页签

**Files:** `package.json`（`@xterm/xterm`）、`ContainersPanel.vue`、`models/containers.ts`

- 终端实例只在控制台页签挂载时创建，切走即 dispose（不做多标签）。
- 三档 `exec / root / 调试图` 切换 → 重连（旧连接先关）。
- 抽屉宽度变化 → 发 `resize`（`ResizeObserver` 节流 120ms）。
- 引入体积先量：`pnpm build` 后看 gzip 增量，超过 120 kB 就退回「串口式只读 + 一次性命令」。

**commit**：`feat(admin): 容器控制台页签`

---

## 阶段 D：验收

1. `bash scripts/verify.sh` 全绿（两套 workspace 的 fmt/clippy/test + 前端 + msocheck）。
2. 真机冒烟（对照设计文档验收 1–12 条）：列表与 `docker ps -a` 一致；停止/启动/重启/暂停状态回读正确；删除两分支；`127.0.0.1` 与 `0.0.0.0` 端口区分显示；控制台能 `ls`/`pwd`/`exit`。
3. 无 docker 的机器：模块不可用，不是空列表。
4. 非 docker 用户：明确报权限。
5. 注入尝试（`;`、`$()`、空格、引号、超长）→ 400，脚本未执行。

---

## 风险与回滚

| 风险 | 处置 |
|---|---|
| `docker stats --no-stream` 慢（1–2s），30 个容器时更慢 | 只在抽屉打开或显式开资源列时跑 3s 一档，失败显示 `—` 并记 degraded |
| 批量 `inspect $(docker ps -aq)` 命令行过长 | 容器超过 200 个时退回分页（一次 100 个，两批 inspect），或用 `--format` 只取需要字段控制输出体积 |
| PTY 通道在弱网下卡死 | 输入走队列 + 写超时；关闭时先 `close` 帧再断，避免远端残留 shell |
| xterm.js 体积 | 阶段 C 先量体积，超预算砍成"只读日志 + 一次性命令"，控制台降级不做 |
| 误删 | 删除在菜单最后一项、二次确认、运行中 force 文案、可选删镜像默认关闭 |

回滚：阶段 A 只读，合入后即使后续阶段不做也是完整可用的只读面板；阶段 B/C 各自独立 commit，出问题单独 revert。

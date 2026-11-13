# 配置导入导出 实施计划（v2，已按独立审查修订）

> **给执行者**：REQUIRED SUB-SKILL — `superpowers:executing-plans`，按任务顺序逐条执行。
> v1 的 15 条审查意见（2 条 P0、9 条 P1、4 条 P2）已并入本版，改动点在各任务里用「审查修正」标出。

**目标**：把「隧道（SSH 连接）+ 转发」导出为可分享的 `.threadline` 包、并支持导入落地（不含密码/私钥）。

**架构**：前端负责归档格式（`fflate` 压/解 zip、图标校验、弹窗交互、偏好读写）；Core 负责规则（`profile/plan` 只读预判、`profile/import` 逐条落地、无凭据连接状态）。设计见 `docs/plans/2026-11-13-profile-import-export-design.md`。

**技术栈**：Rust（tl-core / tl-protocol / axum）、Vue 3 + TypeScript + Pinia、vitest、fflate、Tauri 2。

---

## 前置约束

1. **只在本地 commit，不 push**（2026-11-13 起私有远端停止推送）。
2. 每个 Task 末尾跑对应测试；阶段结束时跑 `bash scripts/verify.sh`。
3. 新增错误消息必须能命中 `crates/tl-core/src/interface/error.rs` 的关键词映射：400（不能为空/必须/无效/格式）、409（已存在/已被/占用/未启用/无法/待确认），否则落 500。
4. 前端新增文件放 `src/models/`（纯逻辑）与 `src/components/`（视图）；views 里不直连 HTTP（`tools/msocheck` R3）。

---

## 阶段 A：Core 契约与「待填凭据」

### Task A1：契约先行（tl-protocol + 前端类型镜像）

**Files:**
- Modify: `crates/tl-protocol/src/lib.rs`
- Modify: `src/api/types.ts`
- Modify: 所有构造 `ServerDto` 的测试（`crates/tl-protocol/src/lib.rs` 的 golden 测试、`tl-core` 各 fixture）

**审查修正**：v1 把 protocol 与 model 拆成两个任务，第一个 commit 编译不过；本版合并且一次补全所有构造点。

**Step 1：`ServerDto` 加派生字段**

```rust
/// 还没有登录材料（导入分享包后的初始状态）。由 `credential_id` 是否为空派生：
/// **不落盘**（`skip`），每次返回前重算。
#[serde(skip)]
pub needs_credential: bool,
```

**Step 2：profile DTO（一次写全，字段与设计 §3 对齐）**

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileForward {
    pub id: String,
    pub name: String,
    pub local_host: String,
    pub local_port: u16,
    pub remote_host: String,
    pub remote_port: u16,
    #[serde(default = "default_scheme")]
    pub scheme: String,
    #[serde(default)]
    pub domain: String,
    /// 远端实际协议；缺省由 scheme 推导——漏了它，https+远端 http 的本地 TLS 终止会变成透传
    #[serde(default)]
    pub remote_scheme: Option<String>,
    #[serde(default)]
    pub open_path: String,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub in_app: bool,
    #[serde(default)]
    pub auto_start: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileServer {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub credential_kind: CredentialKind,
    #[serde(default = "default_keepalive")]
    pub keepalive_secs: u32,
    #[serde(default)]
    pub host_key_fingerprint: Option<String>,
    #[serde(default)]
    pub forwards: Vec<ProfileForward>,
}
```

`ProfilePlanRequest { servers: Vec<ProfileServer> }`、`ProfilePlan { servers: Vec<PlannedServer> }`（**无 preferences**——主题在 localStorage、快捷键在 agent.json，Core 取不到）。

**Step 3：补所有构造点**

```bash
cargo check --workspace --all-targets
```

报错的 `ServerDto { ... }` 一律补 `needs_credential: false`（测试 fixture 除外，见各任务）。

**Step 4：前端镜像**

```ts
export interface ServerDto {
  // …原有字段
  /** 还没有登录材料（导入分享包后的状态）；这类连接不能启用。 */
  needs_credential?: boolean;
}
```

**Step 5：验证**

```bash
cargo test -p tl-protocol && pnpm typecheck
```

**Step 6：commit**

```bash
git commit -am "feat(protocol): 导入导出契约与 needs_credential"
```

---

### Task A2：`server` 模型允许空凭据（含派生统一重算）

**Files:**
- Modify: `crates/tl-core/src/models/server/mod.rs`
- Test: 同文件 `mod tests`

**审查修正**：v1 用「load 时回填」，但 `patch`/`set_enabled` 返回的是 clone，填完密码仍显示待填。改为统一的 `decorate`。

**Step 1：写失败测试**

```rust
#[test]
fn server_without_credential_is_pending_not_broken() {
    let f = fixture("pending-credential");
    let mut req = create_req("imported", "");
    req.secret = None;
    let dto = f.servers.create(req).expect("create without credential");
    assert!(dto.credential_id.is_empty());
    assert!(dto.needs_credential);

    let err = f.servers.set_enabled(&dto.id, true).expect_err("must reject");
    assert!(err.to_string().contains("还没填密码"), "{err}");

    let err = f.servers.connect_params(&dto.id).expect_err("must reject");
    assert!(err.to_string().contains("还没填密码"), "{err}");
}

#[test]
fn needs_credential_is_derived_on_every_return_path() {
    // 截图里最容易漏的一条：patch 之后返回的 DTO 也必须重算
    let f = fixture("derive-pending");
    let mut req = create_req("p", "");
    req.secret = None;
    let dto = f.servers.create(req).unwrap();
    // 模拟 business 层填好凭据后回填 credential_id
    let patched = f.servers.patch(&dto.id, PatchServerRequest {
        credential_id: Some(f.credential_id.clone()),
        ..Default::default()
    }).unwrap();
    assert!(!patched.needs_credential);
}
```

**Step 2：跑测试确认失败**

```bash
cargo test -p tl-core server_without_credential_is_pending_not_broken
```

**Step 3：实现**

```rust
/// 派生字段统一在这里算：任何返回 DTO 的路径都要过一遍，
/// 否则填完密码后列表仍会显示「待填」。
fn decorate(&self, dto: &mut ServerDto) {
    dto.needs_credential = dto.credential_id.trim().is_empty();
}
```

- `load()` 里删掉待填回填，改为在 `list` / `create` / `patch` / `set_enabled` 的返回值上调用 `decorate`
- `validate_create` 删掉 `credential_id` 非空校验
- `create` 里 `require_credential` 只在非空时调用
- `set_enabled` 开头：无凭据且 `enabled=true` → `Err(anyhow!("这条隧道还没填密码，无法启用"))`
- `connect_params`：无凭据 → `Err(anyhow!("这条隧道还没填密码，无法连接"))`

> 字段从磁盘反序列化时 `#[serde(skip)]` 会给 `false`，所以落盘文件不会带这个字段；检查 `servers.json` 快照测试不会因此变红。

**Step 4：跑测试**

```bash
cargo test -p tl-core server_without_credential && cargo test -p tl-core
```

**Step 5：commit**

```bash
git commit -am "feat(core): 允许无凭据的连接以待填状态存在"
```

---

### Task A3：`patch_server` 的待填凭据分支

**Files:**
- Modify: `crates/tl-core/src/business/mod.rs`（`patch_server`，约 257-300 行）

**审查修正（P0）**：v1 把外层判据换成 `has_new_secret || !credential_id.is_empty()`，正常连接恒为真，会导致**每次改端口都重建凭据并丢掉私钥口令**。外层判据必须保持原样。

**Step 1：写失败测试**

```rust
#[test]
fn patch_pending_server_without_secret_keeps_it_pending() {
    // 只有 username/is_some 或 secret.is_some 才进材料分支；待填凭据且没给 secret 时
    // 只更新字段，不读材料、不建凭据
}

#[test]
fn patch_normal_server_does_not_rebuild_credential() {
    // 回归锁：已有凭据的连接只改端口时，credential_id 不能变
}
```

**Step 2：跑测试确认失败**

```bash
cargo test -p tl-core patch_pending_server_without_secret
```

**Step 3：实现（保留外层判据）**

```rust
if username.is_some() || secret.is_some() {
    let current = /* 原查找 */;
    let has_new_secret = secret.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false);

    if has_new_secret {
        // 原逻辑：建新材料、切换 credential_id、删旧材料
    } else if !current.credential_id.trim().is_empty() {
        // 原逻辑：沿用旧材料，只改用户名
    }
    // 待填凭据且没给 secret：什么都不做。
    // 注意：username/kind 在函数开头已被 take()，这里要把它们放回 req，
    // 否则后面的字段更新分支拿不到值，改名会静默失效。
    req.username = username;
    req.kind = kind;
}
```

**Step 4：跑测试**

```bash
cargo test -p tl-core
```

**Step 5：commit**

```bash
git commit -am "fix(core): 待填凭据的连接可改名，且不会重建已有凭据"
```

---

### Task A4：未填密码不进入重试循环

**Files:**
- Modify: `crates/tl-core/src/business/tunnel_runner.rs`
- Modify: `crates/tl-core/src/interface/mod.rs`（`set_server_enabled` 错误映射）
- Modify: `src/api/client.ts`（`ApiFailure.credentialMissing`）

**审查修正（P1）**：`ensure_session` 把 `connect_params` 的错误翻成 `NetworkFailed`，keeper 会无限退避重试。

**Step 1：写失败测试**

```rust
#[test]
fn missing_credential_is_not_retryable() {
    // 拿一个无凭据连接跑 ensure_session，断言得到的结果不是 NetworkFailed，
    // 且 keeper 的 wants_retry 对它返回 false
}
```

**Step 2：实现**

- `Outcome` 增加 `CredentialMissing`（或等价变体），`describe` 文案含「还没填密码」
- `ensure_session` 在 `connect_params` 报该错误时映射到它
- `wants_retry` 对该变体返回 false
- `set_server_enabled` handler：

```rust
.map_err(|e| {
    if e.to_string().contains("还没填密码") {
        ApiFailure::conflict("credential_missing", e.to_string())
    } else {
        e.into()
    }
})?
```

**Step 3：前端**

```ts
get credentialMissing(): boolean {
  return this.code === 'credential_missing';
}
```

**Step 4：验证**

```bash
cargo test -p tl-core && pnpm typecheck
```

**Step 5：commit**

```bash
git commit -am "fix(core): 未填密码的连接不进入重试循环"
```

---

### Task A5：配置级冲突查询 + 连接 id 唯一化

**Files:**
- Modify: `crates/tl-core/src/models/tunnel/mod.rs`
- Modify: `crates/tl-core/src/models/server/mod.rs`

**审查修正（P0/P2）**：`is_port_taken(port)` 只查运行期监听表、不分 host，不能用于导入预判；配置级判定在私有 `ensure_unique_in`。`ServerStore::new_id` 同进程内同名会撞 id。

**Step 1：写失败测试**

```rust
#[test]
fn config_conflict_reports_existing_name_and_domain() {
    // 现有转发占 127.0.0.1:3389 与域名 a.example.com
    // 传同一 host+port → 返回 Some(占用者名称)
    // 传同一域名 → 返回 Some(占用者名称)
    // 传 exclude_id=自己 → None（编辑自己不算冲突）
}

#[test]
fn server_ids_are_unique_within_one_process() {
    // 连续创建两条同名连接，id 必须不同
}
```

**Step 2：实现**

```rust
/// 配置级冲突：返回占用者名称，供 plan/create/patch 共用。
///
/// 与 `is_port_taken` 的区别：那个查的是**运行期**监听表、不分 host，
/// 只能回答"现在能不能 bind"，不能回答"配置里是否已经有一条"。
pub fn config_conflict(
    &self,
    local_host: &str,
    local_port: u16,
    domain: &str,
    exclude_id: Option<&str>,
) -> Option<String>
```

- 内部复用 `effective_addr` 与域名比较逻辑（把 `ensure_unique_in` 的实现抽出来，两处都调它）
- `ServerStore::new_id(name, taken: &HashSet<String>)`：照抄 `tunnel::new_id` 的去重写法

**Step 3：验证**

```bash
cargo test -p tl-core
```

**Step 4：commit**

```bash
git commit -am "feat(core): 配置级冲突查询与连接 id 去重"
```

---

## 阶段 B：Core 判定端点

### Task B1：`profile/plan`

**Files:**
- Modify: `crates/tl-protocol/src/lib.rs`
- Create: `crates/tl-core/src/business/profile.rs`
- Modify: `crates/tl-core/src/business/mod.rs`
- Modify: `crates/tl-core/src/interface/mod.rs`
- Test: `business/profile.rs` 内联

**契约**

```rust
#[derive(Serialize)]
pub struct ProfilePlan {
    pub servers: Vec<PlannedServer>,
}

#[derive(Serialize)]
pub struct PlannedServer {
    pub index: usize,
    pub name: String,
    /// ok / exists（同 id）/ name_taken / invalid
    pub action: String,
    pub reason: Option<String>,
    /// 同名时建议的新名字（如「mypc (2)」），前端可直接采用
    pub suggested_name: Option<String>,
    pub forwards: Vec<PlannedForward>,
}

#[derive(Serialize)]
pub struct PlannedForward {
    pub index: usize,
    pub name: String,
    /// ok / exists / name_taken / port_taken / domain_taken / invalid
    pub action: String,
    pub reason: Option<String>,
    pub occupied_by: Option<String>,
}
```

**Step 1：写失败测试**

```rust
#[test]
fn plan_reports_port_and_domain_conflicts() {
    // 已有连接 a + 转发（127.0.0.1:3389, a.example.com）
    // 包内：连接 a（同名）、转发占 3389、转发占 a.example.com、新端口 18080
    // 断言 action 与 occupied_by
}

#[test]
fn plan_detects_conflicts_inside_the_package_itself() {
    // 同一个包里两条转发抢 18080：后一条必须 port_taken
}

#[test]
fn plan_accepts_open_path_and_remote_scheme() {
    // 反序列化不缺字段（防 v1 漏字段回归）
}
```

**Step 2：实现**

- 遍历包内 server：`servers.list()` 查同 id / 同名 → `exists` / `name_taken` + `suggested_name`
- 遍历 forwards：先与**现有配置**比（`config_conflict`），再与**本包已接受的前序条目**比
- 只读，不落盘

**Step 3：接路由**

```rust
.route(&format!("{API_PREFIX}/profile/plan"), post(profile_plan))
```

**Step 4：验证**

```bash
cargo test -p tl-core profile
```

**Step 5：commit**

```bash
git commit -am "feat(core): 新增 profile/plan 导入预判端点"
```

---

### Task B2：`profile/import`

**Files:**
- Modify: `crates/tl-protocol/src/lib.rs`
- Modify: `crates/tl-core/src/business/profile.rs`
- Modify: `crates/tl-core/src/interface/mod.rs`
- Test: `business/profile.rs`

**契约**

```rust
#[derive(Deserialize)]
pub struct ProfileImportRequest {
    pub servers: Vec<ProfileImportServer>,
    /// 导入后保持停用；**缺省必须为 true**，漏传不能变成导入即启用
    #[serde(default = "default_true")]
    pub keep_disabled: bool,
}

#[derive(Deserialize)]
pub struct ProfileImportServer {
    pub source_id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub credential_kind: CredentialKind,
    pub keepalive_secs: u32,
    /// 仅 action=create 时采用；overwrite 忽略（不改写对方的安全状态）
    pub host_key_fingerprint: Option<String>,
    /// create / overwrite / skip
    pub action: String,
    pub forwards: Vec<ProfileImportForward>,
}

#[derive(Deserialize)]
pub struct ProfileImportForward {
    // …与 ProfileForward 同字段
    /// 前端从包里读出的图标 data URI；空 = 不带
    #[serde(default)]
    pub icon: String,
    /// create / overwrite / skip
    pub action: String,
    /// 覆盖时按此 id 找现有转发
    #[serde(default)]
    pub target_id: Option<String>,
}

#[derive(Serialize)]
pub struct ProfileImportResult {
    pub created_servers: usize,
    pub created_forwards: usize,
    pub skipped: usize,
    pub failed: Vec<ProfileImportFailure>,
}
```

**Step 1：写失败测试**

```rust
#[test]
fn import_creates_pending_server_and_forwards() {
    // keep_disabled 缺省 → true；连接 needs_credential=true、enabled=false
    // 转发的 auto_start 保持文件里的值
}

#[test]
fn overwrite_keeps_existing_credential_and_fingerprint() {
    // 覆盖已有连接：credential_id 与 host_key_fingerprint 原样保留
}

#[test]
fn import_is_idempotent_by_skip_action() {
    // 同一条目第二次按 skip 提交：不新建、不报错、计入 skipped
}
```

**Step 2：实现要点**

- **create**：`servers.create` 直调（绕过 `business::create_server` 的「必须填密码」），`enabled=false`
- **overwrite**：按 `source_id` 找现有连接 → 只改名称/主机/端口/用户名/认证方式/保活；**不传 secret、不动 `credential_id` 与 `host_key_fingerprint`**
- **skip**：计入 `skipped`
- 转发：
  - `create` 走 `tunnels.create`；名称冲突时用 `suggested_name` 或返回失败
  - `overwrite` 走 `tunnels.patch(target_id, ...)`；无 `target_id` 时降级为 `create`
- `icon` 超过 Core 的 `normalize_icon` 上限（200KB 字符）时**清空图标再创建**，不要整条失败
- 失败不回滚已成功的，逐条记 `failed`

**Step 3：接路由**

```rust
.route(&format!("{API_PREFIX}/profile/import"), post(profile_import))
```

**Step 4：验证**

```bash
cargo test -p tl-core
```

**Step 5：commit**

```bash
git commit -am "feat(core): 新增 profile/import 逐条导入端点"
```

---

## 阶段 C：前端归档与偏好

### Task C1：包读写（TDD）

**Files:**
- Create: `src/models/profile.ts`
- Create: `src/models/profile.test.ts`
- Modify: `package.json`（`pnpm add fflate`）

**Step 1：装依赖**

```bash
pnpm add fflate
```

**Step 2：写失败测试**

```ts
it('round-trips a profile through the package', async () => {
  const bytes = await writePackage(SAMPLE_PROFILE, ICONS);
  const back = await readPackage(bytes);
  expect(back.profile.servers[0].forwards[0].open_path).toBe('/dash');
  expect(back.icons.get('91c2')).toMatch(/^data:image\/png;base64,/);
});

it('rejects unknown kind or version', async () => {
  await expect(readPackage(await writePackage({ ...SAMPLE_PROFILE, version: 99 }, new Map())))
    .rejects.toThrow(/版本/);
});

it('drops oversized icons but keeps the forward', async () => {
  // 300KB 图标：解析后 icon 为空、条目仍在
});

it('ignores entries outside profile.json and icons/', async () => {
  // 构造 ../evil.png 与 nested/dir/x.png，断言被过滤
});
```

**Step 3：实现**

- `buildProfile(selectedServers, selectedForwards, options)`：勾选收集、去重、把 data URI 图标拆成 `icons/<shortId>.png`（`shortId` 取转发 id 的 4~8 位哈希）
- `writePackage(profile, icons)`：`fflate.zipSync`
- `readPackage(bytes)`：

```ts
// 先过滤再解压：全量解压后再校验等于没有上限
const files = unzipSync(bytes, {
  filter: (file) => {
    if (file.name !== 'profile.json' && !/^icons\/[^/]+$/.test(file.name)) return false;
    if (file.originalSize > MAX_ICON_BYTES && file.name !== 'profile.json') return false;
    return true;
  },
});
```

- 上限：文件 ≤10MB、条目 ≤200、解压总量 ≤20MB
- 图标：白名单 MIME + **128KB 原始字节**（base64 后约 17 万字符，落在 Core 200KB 之内）
- `kind` / `version` 校验

**Step 4：跑测试**

```bash
npx vitest run src/models/profile.test.ts
```

**Step 5：commit**

```bash
git add src/models/profile.ts src/models/profile.test.ts package.json pnpm-lock.yaml
git commit -m "feat(ui): 配置包读写与图标校验"
```

---

### Task C2：偏好快照与差异

**Files:**
- Create: `src/models/preferences.ts`
- Create: `src/models/preferences.test.ts`

```ts
export interface ProfilePreferences {
  theme: string;
  hotkey: string;
}

/** 读当前本机偏好（主题在 localStorage，快捷键走 Agent）。 */
export async function readPreferences(): Promise<ProfilePreferences>;

/** 与包里的偏好比较，返回给界面展示的差异项。 */
export function diffPreferences(
  current: ProfilePreferences,
  incoming: ProfilePreferences | undefined,
): string[];

/** 写回：主题只接受已知 id；快捷键失败（被占用）时不覆盖，返回失败原因。 */
export async function applyPreferences(p: ProfilePreferences): Promise<string[]>;
```

**测试**：未知主题 id 被忽略；快捷键写失败时保留原值并返回原因；`incoming` 缺省时不产生差异。

**commit**：`feat(ui): 偏好快照与导入差异`

---

### Task C3：API 封装（含长超时）

**Files:**
- Create: `src/api/profile.ts`
- Modify: `src/api/client.ts`
- Modify: `src/api/types.ts`
- Modify: `src/api/index.ts`（若存在统一出口）

**审查修正**：`client.ts` 默认 15s 超时，200 条批量 import 会超时。

**Step 1：给 request 加超时参数**

```ts
async function request<T>(
  method: string,
  path: string,
  body?: unknown,
  retriedAuth = false,
  allowStatus: number[] = [],
  timeoutMs = REQUEST_TIMEOUT_MS,
): Promise<T>
```

```ts
export const http = {
  // …
  /** 批量操作专用：默认 60s，避免 200 条逐条落地被 15s 掐断。 */
  postSlow: <T>(path: string, body?: unknown, timeoutMs = 60_000) =>
    request<T>('POST', path, body, false, [], timeoutMs),
};
```

**Step 2：封装**

```ts
export const profileApi = {
  plan: (servers: ProfileServer[]) => http.post<ProfilePlan>('/profile/plan', { servers }),
  import: (req: ProfileImportRequest) => http.postSlow<ProfileImportResult>('/profile/import', req),
};
```

**Step 3：验证**

```bash
pnpm typecheck
```

**Step 4：commit**

```bash
git commit -am "feat(ui): profile plan/import API 与长超时"
```

---

## 阶段 D：界面

### Task D1：导出弹窗

**Files:**
- Create: `src/components/ProfileExportDialog.vue`

**要点**

- 两级勾选树（连接行 + 展开的转发行；勾连接=全勾，单勾转发=连接半选）
- 三个开关：包含图标（默认开）/ 包含主机指纹（默认开）/ 包含本机偏好（默认关）
- 底部：`已选 N 条连接 / M 条转发`、预计大小（图标像素总量估算）、「导出」
- 导出：`buildProfile` → `writePackage` → `Blob` + `<a download>`（与 `exportCa` 同款）
- 文件名：`<YYYY-MM-DD>-threadline.threadline`

**验证**：导出后解压确认 `profile.json`、`icons/`、`README.txt`、`exported_at`、`app_version`；关图标后 `icon` 全空。

**commit**：`feat(ui): 导出配置弹窗`

---

### Task D2：导入弹窗

**Files:**
- Create: `src/components/ProfileImportDialog.vue`

**流程**

1. `<input type="file" accept=".threadline">` → `readPackage`
2. `profileApi.plan`
3. 清单：连接行（动作下拉，默认按 `action` 选跳过；同名显示 `suggested_name` 可直接采用）、转发行（端口输入框可改、显示 `occupied_by`）
4. 顶部：含域名条目提示「导入后需到设置里同步 hosts」
5. 底部开关：「导入后保持停用」（默认开）、「不覆盖本机偏好」（默认开）
6. 偏好差异：当包里带 `preferences` 且用户允许覆盖时，列出「主题 A→B、快捷键 X→Y」（来自 `diffPreferences`）
7. 提交 → `applyPreferences`（若允许）→ 结果汇总（成功/跳过/失败 + 每条原因 + 「重试失败项」按钮）
8. 关闭后 `useTunnelsStore().refresh()` 与 `useServersStore().refresh()`

**验证**：连续导入两次全部跳过；删一条后重导只补一条；失败项能重试。

**commit**：`feat(ui): 导入配置弹窗`

---

### Task D3：设置页挂载 + 「待填密码」徽标

**Files:**
- Modify: `src/views/SettingsView.vue`
- Modify: `src/views/HomeView.vue`（连接卡片/列表）
- Modify: `src/components/ForwardCard.vue`（如转发被待填连接阻断时的文案）

**审查修正**：v1 先挂空组件导致 `vue-tsc` 编译失败；本版把挂载放到两个弹窗之后。

**设置页**

```html
<div class="panel">
  <h3 class="panel-title">隧道管理</h3>
  <div class="service-row"><span class="hint" style="margin: 0">侧栏与切换条里的先后次序</span>
    <button class="btn" :disabled="!servers.servers.length" @click="openOrder">调整顺序</button></div>
  <div class="service-row"><span class="hint" style="margin: 0">导出可分享的配置包（不含密码/私钥）</span>
    <button class="btn" :disabled="!servers.servers.length" @click="exportOpen = true">导出配置</button></div>
  <div class="service-row"><span class="hint" style="margin: 0">从别人的配置包导入（导入后需自填密码）</span>
    <button class="btn" @click="importOpen = true">导入配置</button></div>
</div>
<ProfileExportDialog v-model="exportOpen" />
<ProfileImportDialog v-model="importOpen" />
```

**待填密码徽标**

- 连接行/服务卡片：`server.needs_credential` → 黄色标签「待填密码」，`title` 说明「导入的连接不含密码，编辑填一次即可」
- 启用开关对该连接禁用，点击提示「先填密码」
- `ApiFailure.credentialMissing` 时给同样的提示，不弹红色错误横幅

**验证**：`pnpm build`；导入后连接显示黄标、开关不可用、编辑填密码后恢复。

**commit**：`feat(ui): 隧道管理入口与待填密码提示`

---

## 阶段 E：收口

### Task E1：文档

- `docs/TESTPLAN.md`：新增 U-16（导出）、U-17（导入）
- `docs/DONE.md`：实施完成后记一条（日期、改法、门禁结果）
- `docs/TODO.md`：删掉「配置导入导出（已拍板，待实施）」
- 设计文档状态保持「已拍板」，补一行「实施：plan v2」

### Task E2：门禁

```bash
bash scripts/verify.sh
```

预期：`Local gate GREEN.`。失败逐项修，不跳过任何一步（脚本会拒绝跳过）。

### Task E3：真机手测

1. 勾 2 条连接（含 1 条 RDP 转发 + 1 条带 `open_path` 的 web 转发）导出
2. 解压确认结构与图标文件
3. 同机导入：全部跳过
4. 换一个临时数据目录（`tl-core --data-dir` 起新核心）导入：连接标「待填密码」、转发在、应用页可见、域名条目提示同步 hosts
5. 填密码 → 启用 → 转发跑通；确认未填密码时**不会反复重连**（日志里看不到退避循环）

---

## 风险与回滚

| 风险 | 应对 |
|---|---|
| 待填连接被 keeper 反复重试 | A4 非重试 outcome + 导入默认 `enabled=false` |
| 覆盖导入清掉对方密码/指纹 | B2 覆盖只改展示字段；单测锁死 |
| 预览与落地判定漂移 | 两者共用 `config_conflict` |
| zip 炸弹/路径穿越 | C1 `unzipSync` filter + 条目名白名单 + 总量上限 |
| 图标超限导致整条失败 | 前端 128KB 上限 + Core 侧超限清空图标 |
| 批量导入超时 | C3 60s 专用超时；失败项可重试 |
| 连接 id 撞车 | A5 id 去重 |

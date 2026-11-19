# HTTP 头改写开关（每条转发独立）设计

- 日期：2026-11-19
- 状态：设计已定（用户两次拍板：① 关闭=全部停掉；② 能看到明文的入口都可开）
- 关联：`docs/plans/2026-11-12-domain-access-design.md`、`docs/plans/2026-11-18-local-machine-access-design.md`

## 1. 背景

v0.2.1 起，② 本地 TLS 终止（HTTPS→HTTP）会把上行 `Host` 换成转发里填的远端地址，
并补 `X-Forwarded-Proto/Host`、把响应 `Location` 换回 `https://<域名>`。
这是**无条件**的：只要入口终止了 TLS 就改写。

问题：远端服务按域名做虚拟主机（`server_name` 匹配入口域名）时，被换掉的 `Host`
反而让它 404；用户没有任何办法关掉。

## 2. 目标与非目标

目标：每条转发一个独立开关，**默认关闭**；关闭时完全透明转发（`Host` /
`X-Forwarded-*` / `Location` 一律不动）；开启时才改写，行为与今天一致。

非目标：不做按域名/路径的细粒度规则；不引入 HTTP 反代（仍是字节管道 + 首块头改写）。

## 3. 数据模型

`TunnelDto` 新增：

| 字段 | 类型 | 默认 | 含义 |
|---|---|---|---|
| `rewrite_http_headers` | `bool` | `false` | 是否改写 HTTP 头（Host / X-Forwarded-* / Location） |

同步：
- `CreateTunnelRequest`：`#[serde(default)] bool`；
- `PatchTunnelRequest`：`Option<bool>`（不传 = 不改）；
- `ProfileForward` / `ProfileImportForward`：`#[serde(default)] bool`，导入导出不丢这个设置。

**不升 `SCHEMA_VERSION`**：`#[serde(default)]` 即可，老配置读成 `false`。

**行为变更（明示）**：老配置里 ② 的转发升级后不再改写 Host——这正是用户要的默认值。

## 4. 生效条件（模型层唯一口径）

```rust
pub fn http_rewrite_enabled(def: &TunnelDto) -> bool {
    if !def.rewrite_http_headers || def.domain.is_empty() {
        return false;
    }
    match entry_mode(def) {
        Ok(EntryMode::Terminate) => true,          // ② HTTPS→HTTP
        Ok(EntryMode::Plain) => def.scheme == "http", // ① http→http
        Ok(EntryMode::Passthrough) | Err(_) => false, // ③ 透传 / tcp 类
    }
}
```

- ③ 透传读到的是 TLS 密文，物理上改不了；`tcp/rdp/ssh` 没有 HTTP 语义。
- ① 明文直通能看到明文，因此也支持（用户确认「能更改的」都支持）。

判定放在 `models/tunnel`，`tunnel_runner` 只把结果传给引擎；引擎不再用
`entry_tls.is_some()` 自己推导。

## 5. 引擎改动

`engine::serve_listener` / `serve_listener_inner` 增加参数 `rewrite_http: bool`，
替换内部 `let rewrite_http = entry_tls.is_some();`。

- ① 时 `entry_tls = None` 且 `rewrite_http = true`：入口明文流直接进
  `pipe_through`，首块头照样能改写，TLS 路径不变。
- 协议不能硬编码：`X-Forwarded-Proto` 与回写的 `Location` 跟着入口协议走——
  终止了 TLS（②）是 `https`，明文直通（①）是 `http`。引擎按 `entry_tls` 是否存在
  得出 `Option<&'static str>` 传进管道，`None` = 不改写。
- 调用点：`tunnel_runner` 两处（SSH / 本机）、`tl-mobile::forward`（恒 `false`，
  安卓不做域名）、引擎单测。

## 6. 前端

新增/编辑端口弹窗「协议」下方增加一行：

- 开关「改写 HTTP 头」，默认关；
- 不可用（`tcp/rdp/ssh`、无域名、③ 透传）时置灰，提示写明为什么；
- 可用时提示：开启后把 `Host` 换成远端地址、补 `X-Forwarded-Proto/Host`、
  把远端重定向换回域名地址；关闭则原样转发（远端按自己的 `server_name` 匹配）。

`ForwardForm` / `emptyForward` / `openForward` / 保存（create/patch）同步该字段；
`src/api/types.ts` 三处 DTO 与 `ProfileImportForward` 加可选字段；
`src/models/profile.ts` 与 `ProfileImportDialog.vue` 导入导出透传。

## 7. 测试

- 模型：`http_rewrite_enabled` 各格（开关关 / 无域名 / ① / ② / ③ / tcp）；
  create/patch 落盘与老配置缺字段读成 `false`。
- 引擎：现有 `rewrite_head` 单测保留（改写本身不变）；`serve_listener` 参数化后
  原有本机转发链路测试不变。
- 前端：`pnpm test` + 构建。
- 真机（TESTPLAN U-19 改写）：开关关闭 → 远端收到浏览器的 `Host: <域名>`、
  无 `X-Forwarded-*`，远端 302 的 `Location` 原样；开关开启 → 行为同旧版。

## 8. 落点

| 层 | 文件 |
|---|---|
| 契约 | `crates/tl-protocol/src/lib.rs`（5 个结构体） |
| 模型 | `crates/tl-core/src/models/tunnel/mod.rs`（判定 + create/patch + 单测） |
| 引擎 | `crates/tl-core/src/models/tunnel/engine.rs`（参数 + 调用点） |
| 业务 | `crates/tl-core/src/business/tunnel_runner.rs`、`business/profile.rs` |
| 门面 | `crates/tl-mobile/src/forward.rs`（`false`） |
| 前端 | `src/views/HomeView.vue`、`src/api/types.ts`、`src/models/profile.ts`、`src/components/ProfileImportDialog.vue` |
| 文档 | 本文件、`docs/TESTPLAN.md`、`docs/TODO.md` |

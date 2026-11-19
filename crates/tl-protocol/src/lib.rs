//! Threadline 跨进程契约：控制 API（HTTP）与磁盘文件格式。
//!
//! 归属：**共享契约**。拥有：全部线上 DTO（请求 + 响应）、`control.json` / `servers.json` /
//! `tunnels.json` / `credentials.dat` / `install.json` / `autostart.result.json` 的结构、枚举。
//! 提供：Core / Agent / Wake 共用的类型定义。**不含任何业务规则与判定逻辑**。
//! 允许调用：仅 serde / serde_json。
//!
//! 纪律（CAW C1）：Agent 与 Wake **只**通过本契约描述的回环 HTTP API 与 Core 交互。
//!
//! 这是"契约镜像"：两侧依赖同一份定义，任何字段改动都要同步更新 golden 单测。

use serde::{Deserialize, Serialize};

pub mod proxy_override;

/// 控制 API 的版本前缀。
pub const API_PREFIX: &str = "/api/v1";

// ─────────────────────── 发现契约：control.json ───────────────────────

/// `control.json` 的两种状态。用**内部 tag `status`**（不用 `untagged`）：
/// `{"stopped":false}` 之类的人为/半写内容不会被静默误判成「已停止」。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ControlFile {
    /// 运行中：Wake/Agent 据此发现端口与 token。
    Running(ControlInfo),
    /// 已停止：端口不可用，Wake 直接走拉起分支。
    Stopped(ControlStopped),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlInfo {
    pub version: String,
    /// 每次启动生成；`/healthz` 必须回传同值，用于区分「我们的核心」与「占端口的别的东西」。
    pub instance_id: String,
    pub pid: u32,
    /// `SYSTEM` 或 `user:<account>`。
    pub identity: String,
    pub run_mode: RunMode,
    pub host: String,
    pub port: u16,
    /// Bearer token（Base64），每次启动轮换。
    pub token: String,
    /// RFC3339 本地时间。
    pub started_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlStopped {
    pub version: String,
}

impl ControlFile {
    pub fn running(&self) -> Option<&ControlInfo> {
        match self {
            ControlFile::Running(info) => Some(info),
            ControlFile::Stopped(_) => None,
        }
    }
}

// ─────────────────────── 枚举 ───────────────────────

/// 核心这一次是**怎么起来的**（由启动参数决定，不是可切换的状态）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunMode {
    /// 任务计划在开机时拉起（SYSTEM）。
    Boot,
    /// 双击 `tl.exe` 拉起（当前用户），且开机自启是关闭的。
    Manual,
    /// 开机自启已开、SYSTEM 核心却没起来，Wake 兜底拉起（当前用户）——降级。
    StandIn,
    /// 无法注册任务计划时的前台回退运行（R7）。
    Fallback,
}

/// 降级标记（CAW I9）。正交集合。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Degradation {
    /// 该由 SYSTEM 跑，实际是用户身份（`run_mode = stand_in`）。
    StandInCore,
    /// 无法注册监督者，处于前台回退形态。
    FallbackRun,
    /// 凭据文件 ACL 未能收紧到「交互用户 + SYSTEM + 管理员」。
    CredentialAclRelaxed,
    /// 凭据不可读，能力缩减（CAW C3）。
    CredentialUnreadable,
    /// 已绑定的本地端口在运行期丢失（极罕见，`SO_EXCLUSIVEADDRUSE` 下不应发生）。
    LocalPortLost,
}

/// 隧道运行状态（语义表见设计 8）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TunnelState {
    Stopped,
    Connecting,
    /// 主机密钥待确认：监听已持有，新连接快速失败。
    NeedsTrust,
    Running,
    /// 断线重连中：**监听保持**，新连接快速失败。
    Backoff,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialKind {
    Password,
    PrivateKey,
}

/// 访问对象的种类：远端 SSH 服务器，还是**跑在这台电脑上**的程序。
///
/// `Local` 是内置条目（见 [`LOCAL_SERVER_ID`]）：不落盘、无凭据、无主机密钥，
/// 恒为已连接。它的转发不走 SSH，直接连本机端口。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ServerKind {
    /// 远端 SSH 服务器（历史行为，也是旧数据的缺省值）。
    #[default]
    Ssh,
    /// 本机：相当于一条已经连上的隧道。
    Local,
}

/// 内置「本机」条目的固定 id。
///
/// 生成器只产出 `srv_*`，所以不会与用户创建的条目撞号；
/// 落盘文件里的 `local` 会在读取时被归一成 `ssh`，避免伪造出第二个本机。
pub const LOCAL_SERVER_ID: &str = "local";

// ─────────────────────── 发现 / 状态响应 ───────────────────────

/// `GET /healthz`（免鉴权）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    pub instance_id: String,
}

impl HealthResponse {
    pub fn ok(version: impl Into<String>, instance_id: impl Into<String>) -> Self {
        Self {
            status: "ok".to_string(),
            version: version.into(),
            instance_id: instance_id.into(),
        }
    }
}

/// 开机自启开关的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutostartStatus {
    /// 逻辑开关：任务计划存在且启用。`task_readable = false` 时来自上一次设置结果。
    pub enabled: bool,
    pub task_exists: bool,
    pub task_enabled: bool,
    /// 当前身份能否读到任务计划定义。
    ///
    /// 用户身份的核心读不到 SYSTEM 任务（`Access is denied`），此时 `task_exists` /
    /// `task_enabled` 不可信、`enabled` 是上次成功设置的方向——界面必须如实说明，
    /// 不能显示成「不存在 / 关闭」。
    pub task_readable: bool,
    /// 当前进程身份改这个开关是否需要一次提权（用户身份 Core 改 SYSTEM 任务 → true）。
    pub privilege_required: bool,
}

/// 上一次成功设置的开机自启方向（读不到任务定义时用来兜底显示）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutostartIntent {
    pub enabled: bool,
    pub at: String,
}

/// `GET /whoami`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WhoamiResponse {
    pub instance_id: String,
    pub version: String,
    pub run_mode: RunMode,
    /// `SYSTEM` 或 `user:<account>`。
    pub identity: String,
    pub degradations: Vec<Degradation>,
    pub autostart: AutostartStatus,
}

/// 磁盘文件结构版本。
///
/// **只有这一处定义**：写入方（各 Store）与作废判断（启动时的留档逻辑）
/// 必须用同一个数，否则会出现「写 1、判 2」——每次启动都把新配置当旧数据清掉。
pub const SCHEMA_VERSION: u32 = 2;

// ─────────────────────── 自启开关 ───────────────────────

/// `POST /autostart`：只改注册层，**不停正在跑的服务**。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetAutostartRequest {
    pub enabled: bool,
}

/// `--autostart on|off` 一次性提权进程写下的结果（Agent 读它闭环，不需要新 IPC）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutostartResult {
    pub ok: bool,
    /// 与 Core 退出码同域（0 成功 / 2 参数错 / 7 数据目录不可用）。
    pub code: u32,
    /// 字段级失败时必填（CAW R5）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    pub message: String,
    pub at: String,
}

// ─────────────────────── 服务器 ───────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerDto {
    pub id: String,
    pub name: String,
    /// `ssh` = 远端服务器；`local` = 本机（内置条目，恒已连接）。
    #[serde(default)]
    pub kind: ServerKind,
    pub host: String,
    pub port: u16,
    /// 内部引用：登录材料由隧道自己创建并持有，界面上不再有「凭据」这一层。
    #[serde(default)]
    pub credential_id: String,
    /// 登录用户名（冗余存在隧道上，列表与表单都直接用它）。
    #[serde(default)]
    pub username: String,
    /// 认证方式（密码 / 私钥）。编辑时用来回显当前用的是哪种。
    #[serde(default = "default_credential_kind")]
    pub credential_kind: CredentialKind,
    /// `None` = 尚未信任（首次连接进 `NeedsTrust`）。
    pub host_key_fingerprint: Option<String>,
    pub keepalive_secs: u32,
    /// 用户意图：这条隧道是否应保持连接。
    ///
    /// 持久化，且**连不上不会自动改回 false**——连不上是现实问题，
    /// 不该篡改"我想让它连着"这件事。
    #[serde(default)]
    pub enabled: bool,
    /// 最近一次建立会话失败的原因（运行时，不落盘）。
    ///
    /// 隧道行只有"未连接"两个字时，用户无从判断是网络、指纹还是被拦，
    /// 所以把原因一起带到列表里。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_error: Option<String>,
    /// 还没有登录材料（导入分享包后的初始状态）。
    ///
    /// 由 `credential_id` 是否为空派生，每次返回前由模型重算覆盖。
    ///
    /// **不能用 `skip`**：它连序列化一起跳过，HTTP 响应里就没有这个字段，
    /// 界面永远看不到「待填」。落盘多一份旧值无害——读回来必被 `decorate` 覆盖。
    #[serde(default)]
    pub needs_credential: bool,
}

/// 调整隧道显示顺序。
///
/// 只带 id 列表：顺序在这个数组里，不在每个隧道上。
/// 缺席的 id 会被追加到末尾（而不是被丢弃），避免界面部分提交时把隧道弄丢。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReorderServersRequest {
    pub ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerList {
    pub items: Vec<ServerDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateServerRequest {
    pub name: String,
    pub host: String,
    pub port: u16,
    /// 已有凭据（内部用；新界面不再传）。
    #[serde(default)]
    pub credential_id: String,
    /// 登录用户名。与 `secret` 同时给出时，隧道会自行创建登录材料。
    #[serde(default)]
    pub username: String,
    /// 认证方式：密码 / 私钥。缺省为密码。
    #[serde(default)]
    pub kind: Option<CredentialKind>,
    /// 密码或私钥全文。**只在创建/修改时传，永不回显**。
    #[serde(default)]
    pub secret: Option<String>,
    #[serde(default)]
    pub passphrase: Option<String>,
    /// 可预置指纹（预置后无需 trust 确认）。
    #[serde(default)]
    pub host_key_fingerprint: Option<String>,
    #[serde(default = "default_keepalive")]
    pub keepalive_secs: u32,
    /// 创建时就希望它连着（默认否，避免"建一条就自动连"的意外）。
    #[serde(default)]
    pub enabled: bool,
}

fn default_keepalive() -> u32 {
    30
}

fn default_credential_kind() -> CredentialKind {
    CredentialKind::Password
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchServerRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub credential_id: Option<String>,
    #[serde(default)]
    pub keepalive_secs: Option<u32>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub kind: Option<CredentialKind>,
    #[serde(default)]
    pub secret: Option<String>,
    #[serde(default)]
    pub passphrase: Option<String>,
}

/// `POST /servers/{id}/enabled` 的请求体。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetServerEnabledRequest {
    pub enabled: bool,
}

/// `POST /servers/{id}/enabled` 的结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerEnabledResult {
    pub enabled: bool,
    /// 操作后会话是否连着。
    pub session_connected: bool,
    /// 受影响的转发数量（启用=恢复了几条；关闭=停掉了几条）。
    pub affected_forwards: usize,
}

/// 一条隧道的 SSH 会话状态。
///
/// 会话是**按隧道共享**的：一条会话承载挂在上面的所有端口转发。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionState {
    pub connected: bool,
    /// 当前挂在这条会话上的转发数量（含未启动的）。
    pub attached_forwards: usize,
}

/// `POST /servers/{id}/session/disconnect` 的结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionClosed {
    /// 顺带被停掉的端口转发数量。
    pub stopped_forwards: usize,
}

/// 用还没保存的表单内容试连一次（新建连接时「测试」按钮走这里）。
///
/// 密码/私钥只在本次调用里用：不落库、不进日志。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeDraftRequest {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub credential_kind: CredentialKind,
    /// 密码，或私钥 PEM。
    pub secret: String,
    #[serde(default)]
    pub passphrase: Option<String>,
    /// 编辑已有连接时带上已信任的指纹；新建留空。
    #[serde(default)]
    pub host_key_fingerprint: Option<String>,
}

/// `POST /servers/{id}/probe` 的结果：告诉界面卡在哪一步。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeServerReport {
    /// `tcp` / `ssh` / `fingerprint` / `auth` / `ok`
    pub stage: String,
    /// 握手观测到的指纹（走到 SSH 阶段才有）。
    pub fingerprint: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostKeyView {
    pub server_id: String,
    pub trusted_fingerprint: Option<String>,
    pub pending_fingerprint: Option<String>,
    pub pending_observed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptHostKeyRequest {
    /// true = 拒绝待确认指纹（隧道转 Stopped）；false = 接受并写入。
    pub reject: bool,
}

// ─────────────────────── 隧道 ───────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelDto {
    pub id: String,
    pub name: String,
    pub server_id: String,
    pub local_host: String,
    pub local_port: u16,
    pub remote_host: String,
    pub remote_port: u16,
    pub auto_start: bool,
    /// 备注：给这条转发写一句话说明（界面在名称后面显示，也参与搜索）。
    #[serde(default)]
    pub note: String,
    /// 点「打开本地页面」时补在端口后面的路径（仅 http/https 有意义）。空 = `/`。
    #[serde(default)]
    pub open_path: String,
    /// 本地这一端用什么协议访问：`tcp` / `http` / `https`。
    ///
    /// **有 `domain` 时这是真实入口协议**：`https` + `remote_scheme=http` 走本地 TLS 终止，
    /// `https` + `remote_scheme=https` 走透传。`domain` 为空时仍只是界面提示，
    /// 转发本身永远是 TCP 字节管道。
    #[serde(default = "default_scheme")]
    pub scheme: String,
    /// 访问域名（如 `home.example.com`）。空 = 不使用域名访问，行为与老版本完全一致。
    #[serde(default)]
    pub domain: String,
    /// 域名入口自动分配的回环地址（如 `127.0.0.5`）。`domain` 为空时恒为空；
    /// 不接受外部输入，只由模型分配。
    #[serde(default)]
    pub domain_host: String,
    /// 远端实际协议 `http` / `https`。`None` = 由 `scheme` 推导（`https`→`https`，其余→`http`）。
    ///
    /// 只有 `domain` 非空时参与入口模式判定；`scheme=tcp` 时忽略。
    #[serde(default)]
    pub remote_scheme: Option<String>,
    /// 自定义图标（URL 或 `data:image/...`）。空 = 由界面按入口自动取 favicon。
    #[serde(default)]
    pub icon: String,
    /// 自定义分组（应用页里的分组标题）。空 = 未分组。
    #[serde(default)]
    pub group: String,
    /// 是否出现在「应用」页。老配置没有这个字段时默认显示。
    #[serde(default = "default_true")]
    pub in_app: bool,
    /// 应用页里的显示顺序（越小越靠前）。
    ///
    /// **只被应用页读取**：转发逻辑（fsm / 监听 / 会话 / hosts）完全不看它，
    /// 改布局不会影响任何一条转发的运行。全为 0 时按配置原顺序稳定排列（旧配置零迁移）。
    #[serde(default)]
    pub app_order: i32,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelStatus {
    pub id: String,
    pub state: TunnelState,
    pub active_connections: u32,
    /// 累计发送字节（本机 → 远端）。单调递增；速率由界面按时间窗口算。
    #[serde(default)]
    pub bytes_up: u64,
    /// 累计接收字节（远端 → 本机）。
    #[serde(default)]
    pub bytes_down: u64,
    pub last_error: Option<String>,
    /// `Backoff` 时距下次重连的秒数。
    pub retry_in_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelView {
    pub definition: TunnelDto,
    pub status: TunnelStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelList {
    pub items: Vec<TunnelView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateTunnelRequest {
    pub name: String,
    pub server_id: String,
    #[serde(default = "default_local_host")]
    pub local_host: String,
    pub local_port: u16,
    #[serde(default = "default_local_host")]
    pub remote_host: String,
    pub remote_port: u16,
    #[serde(default)]
    pub auto_start: bool,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub open_path: String,
    #[serde(default = "default_scheme")]
    pub scheme: String,
    /// 访问域名；空 = 不用域名访问。`domain_host` 不接受外部输入。
    #[serde(default)]
    pub domain: String,
    /// 远端协议；`None` = 由 `scheme` 推导。
    #[serde(default)]
    pub remote_scheme: Option<String>,
    /// 自定义图标（URL 或 `data:image/...`）；空 = 自动。
    #[serde(default)]
    pub icon: String,
    /// 自定义分组；空 = 未分组。
    #[serde(default)]
    pub group: String,
    /// 是否出现在「应用」页；不传 = 显示。
    #[serde(default = "default_true")]
    pub in_app: bool,
}

fn default_local_host() -> String {
    "127.0.0.1".to_string()
}

pub fn default_scheme() -> String {
    "tcp".to_string()
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchTunnelRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub server_id: Option<String>,
    #[serde(default)]
    pub local_host: Option<String>,
    #[serde(default)]
    pub local_port: Option<u16>,
    #[serde(default)]
    pub remote_host: Option<String>,
    #[serde(default)]
    pub remote_port: Option<u16>,
    #[serde(default)]
    pub auto_start: Option<bool>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub open_path: Option<String>,
    #[serde(default)]
    pub scheme: Option<String>,
    /// 访问域名；空串表示清空（回到不带域名的行为）。
    #[serde(default)]
    pub domain: Option<String>,
    /// 远端协议；空串或 `auto` 表示回到「由 `scheme` 推导」。
    #[serde(default)]
    pub remote_scheme: Option<String>,
    /// 自定义图标；空串表示清空（回到自动取 favicon）。
    #[serde(default)]
    pub icon: Option<String>,
    /// 自定义分组；空串表示清空（回到未分组）。
    #[serde(default)]
    pub group: Option<String>,
    /// 是否出现在「应用」页。
    #[serde(default)]
    pub in_app: Option<bool>,
}

// ─────────────────────── 凭据（绝不含密钥字段） ───────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialSummary {
    pub id: String,
    /// 使用者起的名字，便于在列表里认人；为空时前端回退显示用户名。
    #[serde(default)]
    pub name: String,
    pub kind: CredentialKind,
    pub username: String,
    pub has_passphrase: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialList {
    pub items: Vec<CredentialSummary>,
}

/// 明文只在请求体里出现，Core 收到即加密。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateCredentialRequest {
    /// 留空则由服务端用 `username` 兜底。
    #[serde(default)]
    pub name: String,
    pub kind: CredentialKind,
    pub username: String,
    /// 密码明文 或 私钥文本（PEM/OpenSSH）。
    pub secret: String,
    #[serde(default)]
    pub passphrase: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// 局部更新：只改名就只传 `name`；换密钥才传 `secret`。
pub struct PatchCredentialRequest {
    #[serde(default)]
    pub name: Option<String>,
    /// `None` = 不改密钥材料。
    #[serde(default)]
    pub secret: Option<String>,
    /// 仅当同时提供 `secret` 时生效。
    #[serde(default)]
    pub passphrase: Option<String>,
}

// ─────────────────────── 其他响应 ───────────────────────

/// `POST /shutdown`（202，先返回再退出）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShutdownAccepted {
    pub accepted: bool,
    /// 建议客户端等待多少毫秒后再探测端口。
    pub retry_after_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogsResponse {
    pub lines: Vec<String>,
    pub truncated: bool,
}

/// 统一错误体：`{"error":{"code","message","field?"}}`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub error: ApiError,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    pub code: String,
    pub message: String,
    /// 字段级错误必须带字段名（CAW R5）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
}

impl ErrorBody {
    pub fn new(code: impl Into<String>, message: impl Into<String>, field: Option<String>) -> Self {
        Self {
            error: ApiError {
                code: code.into(),
                message: message.into(),
                field,
            },
        }
    }
}

// ─────────────────────── 应用页分组（/app-groups） ───────────────────────

/// 应用页的一个分组。
///
/// 成员归属仍由 `TunnelDto.group`（组名）决定；这里只负责**组的顺序**与
/// **空组的存在性**——没有成员的组无法从转发派生，必须单独存一份清单。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppGroup {
    pub name: String,
    pub order: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppGroupList {
    pub items: Vec<AppGroup>,
}

/// `PUT /app-groups`：全量覆盖（新建、删除空组、调整组顺序）。
///
/// 只改清单，不动成员归属：改名/删组请走 `/app-groups/rename|delete`，
/// 那两个由核心一次改完成员的 `group`，避免前端拆成 N 次写。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetAppGroupsRequest {
    pub items: Vec<AppGroup>,
}

/// `POST /app-groups/rename`：改名 + 该组所有成员一起改。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenameAppGroupRequest {
    pub from: String,
    pub to: String,
}

/// `POST /app-groups/delete`：删组 + 成员回落「未分组」。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteAppGroupRequest {
    pub name: String,
}

/// `PUT /tunnels/app-order`：应用页磁贴的全局顺序。
///
/// 与 `PUT /servers/order` 同规则：顺序在这个数组里，缺席的 id 追加到末尾。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReorderAppRequest {
    pub ids: Vec<String>,
}

// ─────────────────────── 服务器管理（/servers/{id}/admin） ───────────────────────

/// 管理页的一个模块。
///
/// 清单**由核心给**，前端只按它渲染导航与占位：新增模块时只改核心的清单，
/// 界面不需要跟着改结构。`state` 说明这一格现在能不能用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminModule {
    /// 稳定标识（`overview` / `processes` / `files` …），前端按它选组件。
    pub id: String,
    pub title: String,
    pub state: AdminModuleState,
    /// 一句话说明：能用时是数据来源，不能用时是原因或"将做什么"。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdminModuleState {
    /// 本期可用。
    Ready,
    /// 已规划，尚未实现（界面显示"规划中"，不假装能用）。
    Planned,
    /// 这台机器上不可用（缺 docker、非 Linux 等）。
    Unavailable,
}

/// `GET /servers/{id}/admin/overview` 的响应。
///
/// 一次 SSH exec 采完，所有字段都是"采集到就填、采不到就缺省"：
/// 单块失败进 `degraded`，不影响其它块。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerOverview {
    pub collected_at: String,
    /// 本次采集（含远端二次采样等待）耗时，界面用来判断"是不是卡了"。
    pub elapsed_ms: u64,
    pub system: SystemInfo,
    pub cpu: CpuStat,
    pub load: LoadStat,
    pub memory: MemoryStat,
    /// 内存去向的进一步拆分（VM / 容器 / ZFS ARC）。拿不到时全是 0。
    #[serde(default)]
    pub mem_breakdown: MemBreakdown,
    /// GPU 列表（NVIDIA 走 `nvidia-smi`，否则退回 `/sys/class/drm`）。没有 GPU 时为空。
    #[serde(default)]
    pub gpus: Vec<GpuStat>,
    #[serde(default)]
    pub disks: Vec<DiskUsage>,
    /// 物理盘清单（`lsblk`，TYPE=disk）。界面按"盘"展示容量，不看挂载点。
    #[serde(default)]
    pub block_disks: Vec<BlockDisk>,
    /// ZFS 池清单（`zpool list -v`）。没有 ZFS 时为空。
    #[serde(default)]
    pub zfs_pools: Vec<ZfsPool>,
    #[serde(default)]
    pub network: Vec<NetInterface>,
    #[serde(default)]
    pub disk_io: Vec<DiskIo>,
    #[serde(default)]
    pub processes: Vec<ProcessRow>,
    pub containers: ContainerSummary,
    /// 导航清单（含未实现模块）。
    #[serde(default)]
    pub modules: Vec<AdminModule>,
    /// 采集失败的块，例如 `进程列表`、`容器`。空 = 全部采到。
    #[serde(default)]
    pub degraded: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemInfo {
    pub hostname: String,
    /// `/etc/os-release` 的 `PRETTY_NAME`；读不到时留空。
    #[serde(default)]
    pub os: String,
    #[serde(default)]
    pub kernel: String,
    #[serde(default)]
    pub arch: String,
    #[serde(default)]
    pub uptime_secs: u64,
    /// 主出口 IPv4（跳过回环）。取不到时留空，界面就不显示这一行。
    #[serde(default)]
    pub host_ip: String,
}

/// 一块**物理盘**（`lsblk` 里 TYPE=disk 的那一层）。
///
/// 界面关心的是"这台机器有几块盘、各自用掉多少"，而不是"挂了哪些目录"：
/// Unraid 会把阵列里的每个共享都挂一遍，按挂载点列出来就是同一块盘重复十几遍。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockDisk {
    /// 设备名，例如 `nvme0n1`、`sda`。
    pub name: String,
    /// 容量（字节，`lsblk -b`）。
    pub size_bytes: u64,
    /// 该盘所有已挂载分区的文件系统总量（kB）。0 = 没挂载（例如交给阵列托管）。
    #[serde(default)]
    pub total_kb: u64,
    /// 已用量（kB）。0 = 未知。
    #[serde(default)]
    pub used_kb: u64,
    /// 该盘第一个挂载点，悬停时用来说明"挂在哪"。空 = 未挂载。
    #[serde(default)]
    pub mount: String,
    /// `lsblk` 报的文件系统类型（`ext4`、`zfs_member`…）。空 = 没有文件系统。
    #[serde(default)]
    pub fstype: String,
    /// 所属 ZFS 池名。空 = 不属于任何池，或这台机器没装 ZFS。
    ///
    /// ZFS 的成员盘自己**没有挂载点**（池才有），只看 `df` 会以为它们"未挂载"。
    #[serde(default)]
    pub pool: String,
}

/// 一个 ZFS 池。成员盘的容量由池统一使用，所以用量要报在池上。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZfsPool {
    pub name: String,
    /// 池的总容量（字节）。
    #[serde(default)]
    pub size_bytes: u64,
    /// 已分配（字节）。
    #[serde(default)]
    pub alloc_bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CpuStat {
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub cores: u32,
    /// 两次 `/proc/stat` 采样算出的使用率（0–100）。
    #[serde(default)]
    pub usage_percent: f64,
    #[serde(default)]
    pub user_percent: f64,
    #[serde(default)]
    pub system_percent: f64,
    #[serde(default)]
    pub iowait_percent: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LoadStat {
    #[serde(default)]
    pub one: f64,
    #[serde(default)]
    pub five: f64,
    #[serde(default)]
    pub fifteen: f64,
    /// 运行中 / 总进程数（`/proc/loadavg` 第 4 列 `1/234`）。
    #[serde(default)]
    pub running: u32,
    #[serde(default)]
    pub total: u32,
}

/// 内存与 Swap，单位统一为 kB（与 `/proc/meminfo` 一致，省一次换算）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryStat {
    #[serde(default)]
    pub total_kb: u64,
    #[serde(default)]
    pub available_kb: u64,
    #[serde(default)]
    pub used_kb: u64,
    #[serde(default)]
    pub buffers_kb: u64,
    #[serde(default)]
    pub cached_kb: u64,
    /// tmpfs / shmem 占用。它被内核算在 `cached_kb` 里，但删不掉也回收不了，
    /// 所以"真正能腾出来的缓存"要减掉它。
    #[serde(default)]
    pub shmem_kb: u64,
    #[serde(default)]
    pub swap_total_kb: u64,
    #[serde(default)]
    pub swap_used_kb: u64,
}

/// 内存去向的进一步拆分。
///
/// 只做"能低成本拿到"的三类：`qemu/kvm` 进程的 RSS、docker 容器的 cgroup 用量、
/// ZFS 的 ARC。三者都没有时全是 0，界面退回「已用 / 缓存 / Buffers」三段。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemBreakdown {
    /// libvirt / qemu 虚拟机进程的 RSS 合计（kB）。
    #[serde(default)]
    pub vm_kb: u64,
    /// docker 容器的 `memory.current` 合计（kB）。
    #[serde(default)]
    pub container_kb: u64,
    /// ZFS ARC 大小（kB）。它通常被算进 cached，所以是"可回收"那一侧。
    #[serde(default)]
    pub zfs_arc_kb: u64,
}

/// 一块 GPU。NVIDIA 走 `nvidia-smi`，其余厂商退回 `/sys/class/drm`。
///
/// 驱动不报的字段一律是 0（例如核显没有独立显存、老驱动不给温度），
/// 界面据此决定显示哪几行，而不是显示一串 0。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GpuStat {
    /// 型号，例如 `NVIDIA GeForce RTX 4090`。
    pub name: String,
    /// 0–100；驱动不支持时是 0。
    #[serde(default)]
    pub usage_percent: f64,
    #[serde(default)]
    pub memory_used_kb: u64,
    #[serde(default)]
    pub memory_total_kb: u64,
    /// 摄氏度；读不到是 0。
    #[serde(default)]
    pub temperature_c: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DiskUsage {
    pub filesystem: String,
    pub mount: String,
    #[serde(default)]
    pub total_kb: u64,
    #[serde(default)]
    pub used_kb: u64,
    #[serde(default)]
    pub available_kb: u64,
    #[serde(default)]
    pub use_percent: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NetInterface {
    pub name: String,
    #[serde(default)]
    pub rx_bytes: u64,
    #[serde(default)]
    pub tx_bytes: u64,
    /// 字节/秒（两次采样差分）。
    #[serde(default)]
    pub rx_rate: f64,
    #[serde(default)]
    pub tx_rate: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DiskIo {
    pub device: String,
    #[serde(default)]
    pub read_bytes: u64,
    #[serde(default)]
    pub write_bytes: u64,
    /// 字节/秒（两次采样差分）。
    #[serde(default)]
    pub read_rate: f64,
    #[serde(default)]
    pub write_rate: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProcessRow {
    pub pid: u32,
    #[serde(default)]
    pub cpu_percent: f64,
    #[serde(default)]
    pub mem_percent: f64,
    #[serde(default)]
    pub rss_kb: u64,
    #[serde(default)]
    pub command: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ContainerSummary {
    /// `docker` / `podman`；`None` = 这台机器上没有容器引擎。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    #[serde(default)]
    pub total: usize,
    #[serde(default)]
    pub running: usize,
    #[serde(default)]
    pub items: Vec<ContainerRow>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ContainerRow {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub image: String,
    /// `running` / `exited` / `paused` …（docker 的 `State`）。
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub status: String,
}

// ─────────────────────── 容器管理（管理页模块） ───────────────────────
//
// 与上面的 `ContainerSummary` / `ContainerRow` 分开：那两个是**概览卡片**用的，
// 只要 5 个字段；管理页要端口、卷、网络、资源与创建时间，塞进同一个结构会让
// 概览每秒白白多传几十个字段。

/// 列表里的一行。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ContainerItem {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub image: String,
    /// `running` / `exited` / `paused` / `restarting` / `created`。
    #[serde(default)]
    pub state: String,
    /// `Up 3 days (healthy)` / `Exited (1) 4 hours ago`。
    #[serde(default)]
    pub status: String,
    /// `healthy` / `unhealthy`；没有健康检查时是 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compose_project: Option<String>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub restarts: u32,
    #[serde(default)]
    pub ports: Vec<ContainerPort>,
    #[serde(default)]
    pub mounts: Vec<ContainerMount>,
    #[serde(default)]
    pub networks: Vec<ContainerNetwork>,
    /// 以下各项只在带 `stats` 的那一档里出现；没采到就是 0。
    #[serde(default)]
    pub cpu_percent: f64,
    #[serde(default)]
    pub mem_bytes: u64,
    #[serde(default)]
    pub mem_limit_bytes: u64,
    /// 累计收发字节。`docker stats` 给的是累计值而不是速率；
    /// 要算速率得在模型里缓存上一拍，而本模型是**无状态**的，所以照实传累计。
    #[serde(default)]
    pub net_rx_bytes: u64,
    #[serde(default)]
    pub net_tx_bytes: u64,
}

/// 一条端口映射。
///
/// `host_ip` 是必填而不是可选：`0.0.0.0` 与 `127.0.0.1` 决定这个端口是否对局域网开放，
/// 只留端口号的话界面上看不出访问边界。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerPort {
    pub host_ip: String,
    pub host_port: u16,
    pub container_port: u16,
    /// `tcp` / `udp`。
    #[serde(default)]
    pub proto: String,
}

/// 一条卷映射。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerMount {
    /// 宿主路径；匿名卷时这里是卷名（不是路径），界面据此标注。
    pub source: String,
    pub destination: String,
    /// `rw` / `ro`。
    #[serde(default)]
    pub mode: String,
}

/// 容器在某张网络里的地址。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerNetwork {
    pub name: String,
    pub ip: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ContainerList {
    /// `docker`；`None` = 这台机器上没有容器引擎。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    #[serde(default)]
    pub total: usize,
    #[serde(default)]
    pub running: usize,
    #[serde(default)]
    pub exited: usize,
    #[serde(default)]
    pub paused: usize,
    #[serde(default)]
    pub restarting: usize,
    #[serde(default)]
    pub items: Vec<ContainerItem>,
    #[serde(default)]
    pub collected_at: String,
    #[serde(default)]
    pub elapsed_ms: u64,
    /// 采集失败的块，例如 `网络与卷`。
    #[serde(default)]
    pub degraded: Vec<String>,
}

/// 资源那一档（3s）单独回，不混进列表：`stats --no-stream` 自带 1–2s 采样窗口，
/// 塞进 5s 的列表请求里会把列表拖成 3s 一次。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ContainerStats {
    pub id: String,
    #[serde(default)]
    pub cpu_percent: f64,
    #[serde(default)]
    pub mem_bytes: u64,
    #[serde(default)]
    pub mem_limit_bytes: u64,
    #[serde(default)]
    pub net_rx_bytes: u64,
    #[serde(default)]
    pub net_tx_bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ContainerDetail {
    pub item: ContainerItem,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub restart_policy: String,
    #[serde(default)]
    pub env: Vec<ContainerEnv>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerEnv {
    pub key: String,
    /// 敏感键（`PASSWORD` / `SECRET` / `TOKEN` / `KEY`）由核心打码，前端拿不到明文。
    pub value: String,
    #[serde(default)]
    pub masked: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ContainerLogs {
    pub lines: Vec<String>,
    /// 超过 `tail` 上限时为真，界面据此提示「只显示最后 N 行」。
    #[serde(default)]
    pub truncated: bool,
    /// rfc3339，跟随用。
    #[serde(default)]
    pub next_since: String,
}

/// 写操作白名单。**枚举而不是任意字符串**：命令拼装只认这几个值。
///
/// 只有启动 / 停止 / 重启。`pause` 不做：它是 cgroup 冻结，进程还活着、连接还在，
/// 表现为「调用方无限挂起」而不是快速失败——对在线服务（redis、mariadb 这类）
/// 比 stop 更难排查，也没有实际用得上的场景。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContainerAction {
    Start,
    Stop,
    Restart,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ContainerActionResult {
    pub ok: bool,
    #[serde(default)]
    pub message: String,
    /// 动作后回读到的状态；不拿命令退出码当成功依据。
    #[serde(default)]
    pub state: String,
    /// 删容器时勾了「同时删镜像」才有值：镜像是否真的删掉。
    #[serde(default)]
    pub image_removed: bool,
}

// ─────────────────────── 配置导入导出 ───────────────────────

/// 分享包里的一条转发：是 `TunnelDto` 的**可分享子集**。
///
/// 不带 `server_id`（靠嵌套关系挂在连接下）、不带 `domain_host`（本机派生的回环地址）、
/// `icon` 是包内相对路径而不是 data URI。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileForward {
    pub id: String,
    pub name: String,
    #[serde(default = "default_local_host")]
    pub local_host: String,
    pub local_port: u16,
    pub remote_host: String,
    pub remote_port: u16,
    #[serde(default = "default_scheme")]
    pub scheme: String,
    #[serde(default)]
    pub domain: String,
    /// 远端实际协议。漏了它，`https` + 远端 `http` 的本地 TLS 终止会退化成透传。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_scheme: Option<String>,
    #[serde(default)]
    pub open_path: String,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub group: String,
    #[serde(default = "default_true")]
    pub in_app: bool,
    #[serde(default)]
    pub auto_start: bool,
}

/// 分享包里的一条连接。**不含密码/私钥**，也不含 `credential_id`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileServer {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub username: String,
    #[serde(default = "default_credential_kind")]
    pub credential_kind: CredentialKind,
    #[serde(default = "default_keepalive")]
    pub keepalive_secs: u32,
    /// 分享者已信任的主机指纹；导入方可以选择重新确认。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_key_fingerprint: Option<String>,
    #[serde(default)]
    pub forwards: Vec<ProfileForward>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfilePlanRequest {
    pub servers: Vec<ProfileServer>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfilePlan {
    pub servers: Vec<PlannedServer>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedServer {
    pub index: usize,
    pub name: String,
    /// `ok` / `exists`（同 id）/ `name_taken` / `invalid`
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// 重名时建议的新名字（如「mypc (2)」），界面可直接采用。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_name: Option<String>,
    pub forwards: Vec<PlannedForward>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedForward {
    pub index: usize,
    pub name: String,
    /// `ok` / `exists` / `name_taken` / `port_taken` / `domain_taken` / `invalid`
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// 端口/域名被谁占：界面显示「本机 3389 已被 mypc远程桌面 占用」。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occupied_by: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileImportRequest {
    pub servers: Vec<ProfileImportServer>,
    /// 导入后保持停用。**缺省必须为 true**：漏传不能变成"导入即启用"，
    /// 否则没密码的连接会被 keeper 反复重连。
    #[serde(default = "default_true")]
    pub keep_disabled: bool,
    /// 分享者的应用页组清单（组名数组，顺序即顺序）。
    ///
    /// 只**追加**本机没有的组，不覆盖本机已有的组顺序——导入别人的配置不该把
    /// 自己的排版打乱。空数组或老包缺这个字段 = 不动本机的组清单。
    #[serde(default)]
    pub app_groups: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileImportServer {
    /// 包里的 id，用来识别「同一条」。
    pub source_id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub username: String,
    #[serde(default = "default_credential_kind")]
    pub credential_kind: CredentialKind,
    #[serde(default = "default_keepalive")]
    pub keepalive_secs: u32,
    /// 仅 `action = create` 时采用；覆盖时忽略（不改写对方的安全状态）。
    #[serde(default)]
    pub host_key_fingerprint: Option<String>,
    /// `create` / `overwrite` / `skip`
    pub action: String,
    #[serde(default)]
    pub forwards: Vec<ProfileImportForward>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileImportForward {
    pub name: String,
    #[serde(default = "default_local_host")]
    pub local_host: String,
    pub local_port: u16,
    pub remote_host: String,
    pub remote_port: u16,
    #[serde(default = "default_scheme")]
    pub scheme: String,
    #[serde(default)]
    pub domain: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_scheme: Option<String>,
    #[serde(default)]
    pub open_path: String,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub group: String,
    #[serde(default = "default_true")]
    pub in_app: bool,
    #[serde(default)]
    pub auto_start: bool,
    /// 前端已从包里读出的图标（data URI）；空 = 不带。
    #[serde(default)]
    pub icon: String,
    /// 分享者应用页里的排列序号。`None` = 包没带布局（老包），按"追加到末尾"处理。
    ///
    /// 只影响落地后的显示顺序：全部落完再统一重排一次，避免逐条插队把别人的位置顶乱。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_order: Option<i32>,
    /// `create` / `overwrite` / `skip`
    pub action: String,
    /// 覆盖时按此 id 找现有转发。
    #[serde(default)]
    pub target_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileImportResult {
    /// 写入的连接条数（**新建 + 覆盖**）。
    pub created_servers: usize,
    /// 写入的转发条数（**新建 + 覆盖**）。
    pub created_forwards: usize,
    pub skipped: usize,
    pub failed: Vec<ProfileImportFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileImportFailure {
    pub name: String,
    pub reason: String,
}

// ─────────────────────── 磁盘文件契约 ───────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServersFile {
    pub schema_version: u32,
    pub items: Vec<ServerDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelsFile {
    pub schema_version: u32,
    pub items: Vec<TunnelDto>,
}

/// 凭据条目：**只存在于 DPAPI 密文内**，任何 HTTP 端点都不会返回它。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialItem {
    pub id: String,
    /// 老版本文件没有这个字段，`default` 保证升级后仍可读。
    #[serde(default)]
    pub name: String,
    pub kind: CredentialKind,
    pub username: String,
    /// Base64（密码或私钥文本）。
    pub secret_b64: String,
    /// Base64（口令）；`None` = 未设置。
    pub passphrase_b64: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialsFile {
    pub schema_version: u32,
    pub items: Vec<CredentialItem>,
}

/// `install.json`（安装器写入，Core 只读）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallInfo {
    pub schema_version: u32,
    /// 安装时交互用户的 SID（`S-1-5-21-...`）。
    pub interactive_user_sid: Option<String>,
    /// `per_machine` / `portable`。
    pub install_scope: String,
}

// ─────────────────────────── 域名访问 / hosts ───────────────────────────

/// hosts 托管区块里的一条。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedHostEntry {
    pub domain: String,
    pub address: String,
    pub tunnel_id: String,
}

/// 块外已经指向目标域名的行（不覆盖，报冲突）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostsConflict {
    pub domain: String,
    /// 那行原本指向的地址。
    pub existing: String,
}

/// 系统代理例外（WinINET `ProxyOverride`）的同步状态。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyOverrideStatus {
    /// 期望托管的域名（从隧道配置算出）。
    pub desired: Vec<String>,
    /// 上次由 Agent 回报的实际托管条目。
    pub managed: Vec<String>,
    /// `desired != managed`，需要 Agent 再同步一次。
    pub pending: bool,
    /// 系统代理是否启用（由 Agent 上报；Core 是 SYSTEM 时读不到登录用户的设置）。
    pub enabled: Option<bool>,
    /// 是否检测到 PAC 自动配置（`AutoConfigURL` 非空）。
    pub pac: Option<bool>,
    pub last_error: Option<String>,
    pub last_synced_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostsStatus {
    pub path: String,
    /// 当前期望写入托管区块的条目。
    pub managed: Vec<ManagedHostEntry>,
    /// 托管区块里还缺的条目（`address domain`）。空表示块内容与期望一致。
    pub missing: Vec<String>,
    /// 块外冲突（有冲突时不写入，等用户处理）。
    pub conflicts: Vec<HostsConflict>,
    /// 当前进程没有权限写 hosts，需要用户点「写入」触发 UAC。
    pub needs_elevation: bool,
    /// 期望区块与 hosts 实际内容不一致。
    pub pending: bool,
    /// 当前进程是否在提升令牌下（SYSTEM 档为 true）。
    pub elevated: bool,
    pub last_error: Option<String>,
    pub last_synced_at: Option<String>,
    pub proxy: ProxyOverrideStatus,
}

/// `POST /hosts/sync` 的结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostsSyncResult {
    /// 是否已写入（或本来就一致）。
    pub written: bool,
    pub needs_elevation: bool,
    pub status: HostsStatus,
}

/// `tl-core hosts sync` 子命令写给 Agent 的结果文件（与 `AutostartResult` 同形）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostsSyncFileResult {
    pub ok: bool,
    pub code: u32,
    #[serde(default)]
    pub message: Option<String>,
    pub at: String,
}

/// 本地网关根证书状态。
///
/// `trusted` **只来自 Agent 上报**：SYSTEM 档的 Core 查的是 `S-1-5-18` 的根库，
/// 自己判断会永远显示「未信任」（设计 §3.4）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatewayCaStatus {
    pub dir: String,
    /// SHA-256（展示与「是不是同一张根」的判据）。
    pub thumbprint: String,
    /// SHA-1：`certutil -user -store Root <hash>` 只认这个（实测 SHA-256 返回 NOT_FOUND）。
    pub thumbprint_sha1: String,
    pub trusted: Option<bool>,
    pub last_probed_at: Option<String>,
    pub last_error: Option<String>,
    /// 根证书已重新生成但 Core 还没重启，② 的监听仍用旧根。
    pub restart_required: bool,
}

/// Agent 上报的信任探测结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatewayTrustResult {
    pub trusted: bool,
    pub thumbprint: String,
    pub at: String,
}

/// Agent 回报代理例外同步结果（与 `AutostartResult` 同形）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxySyncResult {
    pub ok: bool,
    pub code: Option<i32>,
    /// 实际写入后的托管条目（Core 用它覆盖管理集）。
    #[serde(default)]
    pub applied: Vec<String>,
    #[serde(default)]
    pub message: Option<String>,
    pub at: String,
    /// Agent 顺带探测到的系统代理开关（Core 只存，不自己判断）。
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Agent 顺带探测到的 PAC（`AutoConfigURL` 非空）。PAC 生效时例外不生效。
    #[serde(default)]
    pub pac: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running() -> ControlFile {
        ControlFile::Running(ControlInfo {
            version: "0.1.0".into(),
            instance_id: "3f9c1a7e".into(),
            pid: 12876,
            identity: "SYSTEM".into(),
            run_mode: RunMode::Boot,
            host: "127.0.0.1".into(),
            port: 18921,
            token: "pXh2==".into(),
            started_at: "2026-10-19T09:12:03+08:00".into(),
        })
    }

    #[test]
    fn health_carries_instance_id() {
        let h = HealthResponse::ok("0.1.0", "3f9c1a7e");
        assert_eq!(
            serde_json::to_string(&h).unwrap(),
            r#"{"status":"ok","version":"0.1.0","instance_id":"3f9c1a7e"}"#
        );
        assert_eq!(
            h,
            serde_json::from_str::<HealthResponse>(&serde_json::to_string(&h).unwrap()).unwrap()
        );
    }

    #[test]
    fn control_file_is_internally_tagged() {
        let json = serde_json::to_string(&running()).unwrap();
        assert!(json.contains(r#""status":"running""#));
        assert!(json.contains(r#""run_mode":"boot""#));

        let stopped = ControlFile::Stopped(ControlStopped {
            version: "0.1.0".into(),
        });
        assert_eq!(
            serde_json::to_string(&stopped).unwrap(),
            r#"{"status":"stopped","version":"0.1.0"}"#
        );

        let back: ControlFile = serde_json::from_str(&json).unwrap();
        assert_eq!(back.running().map(|c| c.port), Some(18921));
        let back_stopped: ControlFile =
            serde_json::from_str(r#"{"status":"stopped","version":"0.1.0"}"#).unwrap();
        assert!(back_stopped.running().is_none());
    }

    #[test]
    fn control_file_rejects_ambiguous_or_broken_content() {
        assert!(
            serde_json::from_str::<ControlFile>(r#"{"stopped":false,"version":"0.1.0"}"#).is_err()
        );
        assert!(
            serde_json::from_str::<ControlFile>(r#"{"status":"running","version":"0.1.0"}"#)
                .is_err()
        );
        assert!(
            serde_json::from_str::<ControlFile>(r#"{"status":"nope","version":"0.1.0"}"#).is_err()
        );
        assert!(
            serde_json::from_str::<ControlFile>(&serde_json::to_string(&running()).unwrap())
                .is_ok()
        );
    }

    #[test]
    fn run_mode_and_state_wire_format_is_stable() {
        assert_eq!(
            serde_json::to_string(&RunMode::StandIn).unwrap(),
            r#""stand_in""#
        );
        assert_eq!(serde_json::to_string(&RunMode::Boot).unwrap(), r#""boot""#);
        assert_eq!(
            serde_json::to_string(&RunMode::Fallback).unwrap(),
            r#""fallback""#
        );
        assert_eq!(
            serde_json::to_string(&TunnelState::NeedsTrust).unwrap(),
            r#""needs_trust""#
        );
        assert_eq!(
            serde_json::to_string(&Degradation::StandInCore).unwrap(),
            r#""stand_in_core""#
        );
    }

    #[test]
    fn autostart_is_a_single_switch() {
        let req: SetAutostartRequest = serde_json::from_str(r#"{"enabled":true}"#).unwrap();
        assert!(req.enabled);

        let st = AutostartStatus {
            enabled: true,
            task_exists: true,
            task_enabled: true,
            task_readable: true,
            privilege_required: false,
        };
        assert_eq!(
            serde_json::to_string(&st).unwrap(),
            r#"{"enabled":true,"task_exists":true,"task_enabled":true,"task_readable":true,"privilege_required":false}"#
        );

        let result = AutostartResult {
            ok: false,
            code: 7,
            field: Some("data_dir".into()),
            message: "数据目录不可写".into(),
            at: "2026-10-19T22:01:12+08:00".into(),
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains(r#""field":"data_dir""#));
        assert_eq!(
            result,
            serde_json::from_str::<AutostartResult>(&json).unwrap()
        );
    }

    #[test]
    fn whoami_reports_run_mode_and_autostart_separately() {
        let who = WhoamiResponse {
            instance_id: "abc".into(),
            version: "0.1.0".into(),
            run_mode: RunMode::StandIn,
            identity: "user:alice".into(),
            degradations: vec![Degradation::StandInCore],
            autostart: AutostartStatus {
                enabled: true,
                task_exists: true,
                task_enabled: true,
                task_readable: true,
                privilege_required: true,
            },
        };
        let json = serde_json::to_string(&who).unwrap();
        assert!(json.contains(r#""run_mode":"stand_in""#));
        assert!(json.contains(r#""degradations":["stand_in_core"]"#));
        assert!(json.contains(r#""autostart":{"enabled":true"#));
    }

    #[test]
    fn patch_requests_allow_partial_updates() {
        let p: PatchServerRequest = serde_json::from_str(r#"{"port":2222}"#).unwrap();
        assert_eq!(p.port, Some(2222));
        assert!(p.host.is_none());
        let t: PatchTunnelRequest = serde_json::from_str(r#"{"auto_start":true}"#).unwrap();
        assert_eq!(t.auto_start, Some(true));
        assert!(t.local_port.is_none());
    }

    #[test]
    fn create_requests_have_defaults() {
        let s: CreateServerRequest = serde_json::from_str(
            r#"{"name":"web-01","host":"192.0.2.90","port":22,"credential_id":"cred_1"}"#,
        )
        .unwrap();
        assert_eq!(s.keepalive_secs, 30);
        assert!(s.host_key_fingerprint.is_none());

        let t: CreateTunnelRequest = serde_json::from_str(
            r#"{"name":"web","server_id":"srv_1","local_port":18080,"remote_port":8080}"#,
        )
        .unwrap();
        assert_eq!(t.local_host, "127.0.0.1");
        assert_eq!(t.remote_host, "127.0.0.1");
        assert!(!t.auto_start);
    }

    #[test]
    fn credential_payloads_never_appear_in_summaries() {
        let c = CredentialSummary {
            id: "cred_1".into(),
            name: "生产 root".into(),
            kind: CredentialKind::PrivateKey,
            username: "dev".into(),
            has_passphrase: true,
        };
        let json = serde_json::to_string(&c).unwrap();
        assert_eq!(
            json,
            r#"{"id":"cred_1","name":"生产 root","kind":"private_key","username":"dev","has_passphrase":true}"#
        );
        assert!(!json.contains("secret"));
    }

    #[test]
    fn error_body_carries_named_field() {
        let e = ErrorBody::new(
            "local_port_in_use",
            "本地端口 18080 已被占用",
            Some("local_port".into()),
        );
        assert_eq!(
            serde_json::to_string(&e).unwrap(),
            r#"{"error":{"code":"local_port_in_use","message":"本地端口 18080 已被占用","field":"local_port"}}"#
        );
        assert!(
            !serde_json::to_string(&ErrorBody::new("not_found", "不存在", None))
                .unwrap()
                .contains("field")
        );
    }

    #[test]
    fn disk_files_roundtrip() {
        let servers = ServersFile {
            schema_version: 1,
            items: vec![ServerDto {
                id: "srv_1".into(),
                name: "web-01".into(),
                kind: ServerKind::Ssh,
                host: "192.0.2.90".into(),
                port: 22,
                credential_id: "cred_1".into(),
                username: "root".into(),
                credential_kind: CredentialKind::Password,
                host_key_fingerprint: None,
                keepalive_secs: 30,
                enabled: false,
                session_error: None,
                // 派生字段：落盘冗余一份，读回后由模型按 credential_id 重算覆盖
                needs_credential: false,
            }],
        };
        assert_eq!(
            servers,
            serde_json::from_str::<ServersFile>(&serde_json::to_string(&servers).unwrap()).unwrap()
        );

        let install = InstallInfo {
            schema_version: 1,
            interactive_user_sid: Some("S-1-5-21-1-2-3-1001".into()),
            install_scope: "per_machine".into(),
        };
        assert_eq!(
            install,
            serde_json::from_str::<InstallInfo>(&serde_json::to_string(&install).unwrap()).unwrap()
        );
    }

    #[test]
    fn needs_credential_reaches_the_client() {
        // 它是从 credential_id 派生的；但不能用 skip —— 那会连响应字段一起吞掉，
        // 界面就永远看不到「待填密码」。
        let dto = ServerDto {
            id: "srv_1".into(),
            name: "web-01".into(),
            kind: ServerKind::Ssh,
            host: "192.0.2.90".into(),
            port: 22,
            credential_id: String::new(),
            username: "root".into(),
            credential_kind: CredentialKind::Password,
            host_key_fingerprint: None,
            keepalive_secs: 30,
            enabled: false,
            session_error: None,
            needs_credential: true,
        };
        let text = serde_json::to_string(&dto).unwrap();
        assert!(text.contains("\"needs_credential\":true"), "{text}");
    }

    #[test]
    fn profile_dtos_tolerate_missing_optional_fields() {
        // 包可能来自旧版本：缺 open_path / remote_scheme / group 时必须还能读
        let f: ProfileForward = serde_json::from_str(
            r#"{"id":"f1","name":"rdp","local_port":3389,"remote_host":"h","remote_port":3389}"#,
        )
        .unwrap();
        assert_eq!(f.scheme, "tcp");
        assert_eq!(f.local_host, "127.0.0.1");
        assert!(f.remote_scheme.is_none());

        // keep_disabled 缺省必须为 true（漏传不能变成"导入即启用"）
        let req: ProfileImportRequest = serde_json::from_str(r#"{"servers":[]}"#).unwrap();
        assert!(req.keep_disabled);
    }

    #[test]
    fn credential_file_keeps_material_separate_from_http_dtos() {
        let file = CredentialsFile {
            schema_version: 1,
            items: vec![CredentialItem {
                id: "cred_1".into(),
                name: "dev".into(),
                kind: CredentialKind::Password,
                username: "dev".into(),
                secret_b64: "cGFzcw==".into(),
                passphrase_b64: None,
            }],
        };
        let json = serde_json::to_string(&file).unwrap();
        assert!(json.contains("secret_b64"));
        assert_eq!(
            file,
            serde_json::from_str::<CredentialsFile>(&json).unwrap()
        );
    }

    #[test]
    fn host_key_view_supports_pending_and_trusted() {
        let v = HostKeyView {
            server_id: "srv_1".into(),
            trusted_fingerprint: None,
            pending_fingerprint: Some("SHA256:abc".into()),
            pending_observed_at: Some("2026-10-19T09:20:00+08:00".into()),
        };
        let json = serde_json::to_string(&v).unwrap();
        assert!(json.contains(r#""pending_fingerprint":"SHA256:abc""#));
        assert!(json.contains(r#""trusted_fingerprint":null"#));
    }

    #[test]
    fn api_prefix_is_versioned() {
        assert_eq!(API_PREFIX, "/api/v1");
    }
}

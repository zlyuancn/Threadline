// 控制 API 的类型镜像（与 crates/tl-protocol 的 DTO 一一对应）。
// 这是"契约镜像"：两侧各自持有定义，字段改动必须同时改两边。

export type RunMode = 'boot' | 'manual' | 'stand_in' | 'fallback';

export type Degradation =
  | 'stand_in_core'
  | 'fallback_run'
  | 'credential_acl_relaxed'
  | 'credential_unreadable'
  | 'local_port_lost';

export type TunnelState =
  | 'stopped'
  | 'connecting'
  | 'needs_trust'
  | 'running'
  | 'backoff'
  | 'error';

export type CredentialKind = 'password' | 'private_key';

export interface AutostartStatus {
  enabled: boolean;
  task_exists: boolean;
  task_enabled: boolean;
  privilege_required: boolean;
}

export interface HealthResponse {
  status: string;
  version: string;
  instance_id: string;
}

export interface WhoamiResponse {
  instance_id: string;
  version: string;
  run_mode: RunMode;
  identity: string;
  degradations: Degradation[];
  autostart: AutostartStatus;
}

export interface ServerEnabledResult {
  enabled: boolean;
  session_connected: boolean;
  /** 受影响（恢复/停止）的转发数量。 */
  affected_forwards: number;
}

export interface ServerDto {
  id: string;
  name: string;
  host: string;
  port: number;
  // 登录用户不单独存：唯一来源是凭据的 username。
  credential_id?: string;
  /** 登录用户名（隧道自带，不再有独立凭据）。 */
  username: string;
  /** 当前认证方式；编辑时用来回显，避免总是显示「密码」。 */
  credential_kind?: 'password' | 'private_key';
  host_key_fingerprint: string | null;
  keepalive_secs: number;
  /** 用户意图：这条隧道是否应保持连接（持久化）。 */
  enabled: boolean;
  /** 最近一次建立会话失败的原因（运行时）。 */
  session_error?: string | null;
  /**
   * 还没有登录材料（导入分享包后的状态）。
   *
   * 这类连接不能启用、不能连接；编辑里填一次密码/私钥即恢复。
   * 由 Core 派生，不落盘。
   */
  needs_credential?: boolean;
}

export interface ServerList {
  items: ServerDto[];
}

export interface CreateServerRequest {
  name: string;
  host: string;
  port: number;
  enabled?: boolean;
  username: string;
  kind?: CredentialKind;
  /** 密码或私钥全文；只在创建/修改时传。 */
  secret?: string;
  passphrase?: string | null;
  credential_id?: string;
  host_key_fingerprint?: string | null;
  keepalive_secs?: number;
}

export interface PatchServerRequest {
  name?: string;
  host?: string;
  port?: number;
  credential_id?: string;
  keepalive_secs?: number;
  enabled?: boolean;
  username?: string;
  kind?: CredentialKind;
  secret?: string;
  passphrase?: string | null;
}

export interface SessionState {
  connected: boolean;
  /** 挂在这条会话上的转发数量（含未启动的）。 */
  attached_forwards: number;
}

export interface SessionClosed {
  /** 顺带被停掉的端口转发数量。 */
  stopped_forwards: number;
}

/** 保存前的试连请求：凭据只在本次调用里用，不落库。 */
export interface ProbeDraftRequest {
  host: string;
  port: number;
  username: string;
  credential_kind: CredentialKind;
  /** 密码，或私钥 PEM */
  secret: string;
  passphrase?: string | null;
  host_key_fingerprint?: string | null;
}

export interface ProbeServerReport {
  /** tcp / ssh / fingerprint / auth / ok */
  stage: string;
  fingerprint: string | null;
  message: string;
}

export interface HostKeyView {
  server_id: string;
  trusted_fingerprint: string | null;
  pending_fingerprint: string | null;
  pending_observed_at: string | null;
}

export interface AcceptHostKeyRequest {
  reject: boolean;
}

/**
 * 本地入口协议。
 *
 * 有 `domain` 时是真实入口行为：`https` + `remote_scheme=http` 走本地 TLS 终止，
 * `https` + `remote_scheme=https` 走透传；`domain` 为空时仍只影响 URL 与「打开本地页面」，
 * 转发永远是 TCP 字节管道。
 */
export type LocalScheme = 'tcp' | 'http' | 'https' | 'rdp' | 'ssh';

/** 远端实际协议。缺省（undefined）由 `scheme` 推导：https → https，其余 → http。 */
export type RemoteScheme = 'http' | 'https';

/* ───────────── 域名访问 / hosts / 网关证书 ───────────── */

export interface ManagedHostEntry {
  domain: string;
  address: string;
  tunnel_id: string;
}

export interface HostsConflict {
  domain: string;
  existing: string;
}

export interface ProxyOverrideStatus {
  desired: string[];
  managed: string[];
  pending: boolean;
  enabled: boolean | null;
  pac: boolean | null;
  last_error: string | null;
  last_synced_at: string | null;
}

export interface HostsStatus {
  path: string;
  managed: ManagedHostEntry[];
  /** 托管区块里还缺的条目，形如 `127.0.0.28 mypc.zly.m` */
  missing: string[];
  conflicts: HostsConflict[];
  needs_elevation: boolean;
  pending: boolean;
  elevated: boolean;
  last_error: string | null;
  last_synced_at: string | null;
  proxy: ProxyOverrideStatus;
}

export interface HostsSyncResult {
  written: boolean;
  needs_elevation: boolean;
  status: HostsStatus;
}

export interface ProxySyncResult {
  ok: boolean;
  code?: number | null;
  applied?: string[];
  message?: string | null;
  at: string;
  /** Agent 顺带探测到的系统代理开关。 */
  enabled?: boolean | null;
  /** Agent 顺带探测到的 PAC。 */
  pac?: boolean | null;
}

export interface GatewayCaStatus {
  dir: string;
  /** SHA-256（展示用）。 */
  thumbprint: string;
  /** SHA-1：`certutil -store Root` 只认这个。 */
  thumbprint_sha1: string;
  trusted: boolean | null;
  last_probed_at: string | null;
  last_error: string | null;
  restart_required: boolean;
}

export interface GatewayTrustResult {
  trusted: boolean;
  thumbprint: string;
  at: string;
}

export interface TunnelDto {
  id: string;
  name: string;
  server_id: string;
  local_host: string;
  local_port: number;
  remote_host: string;
  remote_port: number;
  auto_start: boolean;
  /** 备注：显示在名称后面，也参与搜索。 */
  note?: string;
  /** 点「打开本地页面」时补在端口后面的路径（仅 http/https）。空 = 根路径。 */
  open_path?: string;
  /** 老配置没有这个字段，后端会按 tcp 补。 */
  scheme?: LocalScheme;
  /** 访问域名；空 = 不使用域名访问。 */
  domain?: string;
  /** 域名入口自动分配的回环地址，由后端分配，前端只读。 */
  domain_host?: string;
  /** 远端实际协议；缺省由 scheme 推导。 */
  remote_scheme?: RemoteScheme | null;
  /** 自定义图标（http(s) / data:image）；空 = 按入口自动取 favicon。 */
  icon?: string;
  /** 自定义分组；空 = 未分组。 */
  group?: string;
  /** 是否出现在「应用」页。 */
  in_app?: boolean;
}

export interface TunnelStatus {
  id: string;
  state: TunnelState;
  active_connections: number;
  /** 累计发送/接收字节，速率由界面按时间窗口算。 */
  bytes_up?: number;
  bytes_down?: number;
  last_error: string | null;
  retry_in_secs: number | null;
}

export interface TunnelView {
  definition: TunnelDto;
  status: TunnelStatus;
}

export interface TunnelList {
  items: TunnelView[];
}

export interface CreateTunnelRequest {
  name: string;
  server_id: string;
  local_host?: string;
  local_port: number;
  remote_host?: string;
  remote_port: number;
  auto_start?: boolean;
  note?: string;
  open_path?: string;
  scheme?: LocalScheme;
  /** 访问域名；空 = 不用域名。 */
  domain?: string;
  /** 远端协议；不传 = 由 scheme 推导。 */
  remote_scheme?: RemoteScheme;
  /** 自定义图标；不传 = 自动取 favicon。 */
  icon?: string;
  /** 自定义分组；不传 = 未分组。 */
  group?: string;
  /** 是否出现在「应用」页；不传 = 显示。 */
  in_app?: boolean;
}

export interface PatchTunnelRequest {
  name?: string;
  server_id?: string;
  local_host?: string;
  local_port?: number;
  remote_host?: string;
  remote_port?: number;
  auto_start?: boolean;
  note?: string;
  open_path?: string;
  scheme?: LocalScheme;
  /** 访问域名；空串表示清空。 */
  domain?: string;
  /** 远端协议；空串或 `auto` 表示回到由 scheme 推导。 */
  remote_scheme?: RemoteScheme | 'auto' | '';
  /** 自定义图标；空串表示清空（回到自动取 favicon）。 */
  icon?: string;
  /** 自定义分组；空串表示清空（回到未分组）。 */
  group?: string;
  /** 是否出现在「应用」页。 */
  in_app?: boolean;
}

export interface CredentialSummary {
  id: string;
  /** 使用者起的名字；为空时回退显示用户名。 */
  name: string;
  kind: CredentialKind;
  username: string;
  has_passphrase: boolean;
}

export interface CredentialList {
  items: CredentialSummary[];
}

export interface CreateCredentialRequest {
  /** 留空则由服务端用 username 兜底。 */
  name: string;
  kind: CredentialKind;
  username: string;
  secret: string;
  passphrase?: string | null;
}

/* ───────────── 配置导入导出（profile） ───────────── */

export type PlannedAction =
  | 'ok'
  | 'exists'
  | 'name_taken'
  | 'port_taken'
  | 'domain_taken'
  | 'invalid';

export interface PlannedForward {
  index: number;
  name: string;
  action: PlannedAction;
  reason?: string | null;
  /** 端口/域名被谁占：界面显示「已被 xxx 占用」。 */
  occupied_by?: string | null;
}

export interface PlannedServer {
  index: number;
  name: string;
  action: PlannedAction;
  reason?: string | null;
  /** 重名时 Core 建议的新名字。 */
  suggested_name?: string | null;
  forwards: PlannedForward[];
}

export interface ProfilePlan {
  servers: PlannedServer[];
}

/** 用户确认后的一条转发。`action` 决定 Core 怎么落地。 */
export interface ProfileImportForward {
  name: string;
  local_host?: string;
  local_port: number;
  remote_host: string;
  remote_port: number;
  scheme?: string;
  domain?: string;
  remote_scheme?: RemoteScheme | null;
  open_path?: string;
  note?: string;
  group?: string;
  in_app?: boolean;
  auto_start?: boolean;
  /** 已从包里读出的图标（data URI）；空 = 不带。 */
  icon?: string;
  action: 'create' | 'overwrite' | 'skip';
  /** 覆盖时按此 id 找现有转发。 */
  target_id?: string | null;
}

export interface ProfileImportServer {
  source_id: string;
  name: string;
  host: string;
  port: number;
  username: string;
  credential_kind: CredentialKind;
  keepalive_secs?: number;
  host_key_fingerprint?: string | null;
  action: 'create' | 'overwrite' | 'skip';
  forwards: ProfileImportForward[];
}

export interface ProfileImportRequest {
  servers: ProfileImportServer[];
  /** 导入后保持停用；缺省由 Core 当作 true。 */
  keep_disabled?: boolean;
}

export interface ProfileImportFailure {
  name: string;
  reason: string;
}

export interface ProfileImportResult {
  /** 写入的连接条数（**新建 + 覆盖**）。 */
  created_servers: number;
  /** 写入的转发条数（**新建 + 覆盖**）。 */
  created_forwards: number;
  skipped: number;
  failed: ProfileImportFailure[];
}

export interface PatchCredentialRequest {
  /** 只改名时只传 name。 */
  name?: string;
  /** 省略 = 不改密钥材料。 */
  secret?: string;
  /** 仅当同时提供 secret 时生效。 */
  passphrase?: string | null;
}

export interface SetAutostartRequest {
  enabled: boolean;
}

export interface ShutdownAccepted {
  accepted: boolean;
  retry_after_ms: number;
}

export interface LogsResponse {
  lines: string[];
  truncated: boolean;
}

export interface ApiError {
  code: string;
  message: string;
  field?: string;
}

export interface ErrorBody {
  error: ApiError;
}

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
  /** 当前身份能否读到任务定义；false 时 enabled 取自上次设置结果，不是实测。 */
  task_readable: boolean;
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
  /**
   * `ssh` = 远端服务器；`local` = 本机（内置条目，恒已连接，没有会话）。
   *
   * 老版本核心不会带这个字段，缺省按 `ssh` 处理。
   */
  kind?: 'ssh' | 'local';
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
  /**
   * 应用页里的显示顺序（越小越靠前）。
   *
   * 只影响应用页排列，转发本身的运行与它无关；全为 0 时按配置原顺序排。
   */
  app_order?: number;
}

/* ───────────── 应用页布局（/app-groups） ───────────── */

/** 应用页的一个分组。成员归属仍由 `TunnelDto.group` 决定，这里只管顺序与空组。 */
export interface AppGroup {
  name: string;
  order: number;
}

export interface AppGroupList {
  items: AppGroup[];
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
  /** 分享者应用页里的排列序号。缺省 = 包没带布局，落地后追加到末尾。 */
  app_order?: number | null;
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
  /**
   * 分享者的应用页组清单（顺序即顺序）。
   *
   * Core 只在**本机没有这个组名**时追加，不覆盖本机已有的组顺序。
   */
  app_groups?: string[];
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

/* ───────────── 服务器管理（/servers/{id}/admin） ───────────── */

/** 管理页模块状态：可用 / 规划中 / 这台机器上不可用。 */
export type AdminModuleState = 'ready' | 'planned' | 'unavailable';

export interface AdminModule {
  id: string;
  title: string;
  state: AdminModuleState;
  note?: string;
}

export interface SystemInfo {
  hostname: string;
  os: string;
  kernel: string;
  arch: string;
  uptime_secs: number;
  /** 主出口 IPv4；取不到时是空串，界面就不显示这一行。 */
  host_ip?: string;
}

/**
 * 一块物理盘（`lsblk` 里 TYPE=disk 的那一层）。
 *
 * 之所以不按挂载点展示：Unraid 会把阵列里每个共享都挂一遍，
 * 同一块盘在 `df` 里能出现十几次，看着像装了一堆盘。
 */
export interface BlockDisk {
  name: string;
  size_bytes: number;
  /** 该盘所有已挂载分区的容量合计（kB）。0 = 未挂载或由阵列/ZFS 托管。 */
  total_kb: number;
  used_kb: number;
  /** 第一个挂载点，悬停时说明"挂在哪"。 */
  mount?: string;
  /** `lsblk` 报的文件系统类型（`ext4`、`zfs_member`…）。 */
  fstype?: string;
  /** 所属 ZFS 池名。空 = 不是池成员。 */
  pool?: string;
}

/** 一个 ZFS 池：成员盘把容量交给池，用量只能报在池上。 */
export interface ZfsPool {
  name: string;
  size_bytes: number;
  alloc_bytes: number;
}

/** 内存去向的细分：VM / 容器 / ZFS ARC。拿不到的子项是 0。 */
export interface MemBreakdown {
  vm_kb: number;
  container_kb: number;
  zfs_arc_kb: number;
}

export interface CpuStat {
  model: string;
  cores: number;
  usage_percent: number;
  user_percent: number;
  system_percent: number;
  iowait_percent: number;
}

export interface LoadStat {
  one: number;
  five: number;
  fifteen: number;
  running: number;
  total: number;
}

export interface MemoryStat {
  total_kb: number;
  available_kb: number;
  used_kb: number;
  buffers_kb: number;
  cached_kb: number;
  /** tmpfs / shmem：被算在 cached 里，但删不掉也回收不了。 */
  shmem_kb?: number;
  swap_total_kb: number;
  swap_used_kb: number;
}

/** 一块 GPU。驱动报不了的字段是 0（核显没独立显存、老驱动不给温度）。 */
export interface GpuStat {
  name: string;
  /** 0–100。 */
  usage_percent: number;
  memory_used_kb: number;
  memory_total_kb: number;
  /** 摄氏度；0 = 读不到。 */
  temperature_c: number;
}

export interface DiskUsage {
  filesystem: string;
  mount: string;
  total_kb: number;
  used_kb: number;
  available_kb: number;
  use_percent: number;
}

export interface NetInterface {
  name: string;
  rx_bytes: number;
  tx_bytes: number;
  /** 字节/秒。 */
  rx_rate: number;
  tx_rate: number;
}

export interface DiskIo {
  device: string;
  read_bytes: number;
  write_bytes: number;
  read_rate: number;
  write_rate: number;
}

export interface ProcessRow {
  pid: number;
  cpu_percent: number;
  mem_percent: number;
  rss_kb: number;
  command: string;
}

export interface ContainerRow {
  id: string;
  name: string;
  image: string;
  state: string;
  status: string;
}

export interface ContainerSummary {
  engine?: string;
  total: number;
  running: number;
  items: ContainerRow[];
}

// ── 容器管理（管理页模块）────────────────────────────────────────
// 与上面的 ContainerSummary 分开：那是概览卡片用的 5 个字段，
// 这里要端口、卷、网络、资源与创建时间。

export interface ContainerPort {
  host_ip: string;
  host_port: number;
  container_port: number;
  proto: string;
}

export interface ContainerMount {
  /** 宿主路径；匿名卷时是卷名。 */
  source: string;
  destination: string;
  mode: string;
}

export interface ContainerNetwork {
  name: string;
  ip: string;
}

export interface ContainerItem {
  id: string;
  name: string;
  image: string;
  state: string;
  status: string;
  health?: string;
  compose_project?: string;
  created_at: string;
  exit_code?: number;
  restarts: number;
  ports: ContainerPort[];
  mounts: ContainerMount[];
  networks: ContainerNetwork[];
  cpu_percent: number;
  mem_bytes: number;
  mem_limit_bytes: number;
  /** 累计收发字节（docker stats 给的是累计值，不是速率）。 */
  net_rx_bytes: number;
  net_tx_bytes: number;
}

export interface ContainerList {
  engine?: string;
  total: number;
  running: number;
  exited: number;
  paused: number;
  restarting: number;
  items: ContainerItem[];
  collected_at: string;
  elapsed_ms: number;
  degraded: string[];
}

/** 资源档（3s）：单独一条接口，不混进列表。 */
export interface ContainerStats {
  id: string;
  cpu_percent: number;
  mem_bytes: number;
  mem_limit_bytes: number;
  net_rx_bytes: number;
  net_tx_bytes: number;
}

export interface ContainerDetail {
  item: ContainerItem;
  command: string;
  restart_policy: string;
  env: ContainerEnv[];
}

export interface ContainerEnv {
  key: string;
  /** 敏感键已由核心打码。 */
  value: string;
  masked: boolean;
}

export interface ContainerLogs {
  lines: string[];
  truncated: boolean;
  next_since: string;
}

/** 只有启动 / 停止 / 重启。不做 pause：cgroup 冻结对在线服务是「挂起等超时」，比 stop 更难排查。 */
export type ContainerAction = 'start' | 'stop' | 'restart';

export interface ContainerActionResult {
  ok: boolean;
  message: string;
  state: string;
  image_removed: boolean;
}

export interface ServerOverview {
  collected_at: string;
  elapsed_ms: number;
  system: SystemInfo;
  cpu: CpuStat;
  load: LoadStat;
  memory: MemoryStat;
  /** 内存去向细分（VM / 容器 / ZFS ARC）；全 0 表示这台机器没这些东西。 */
  mem_breakdown: MemBreakdown;
  /** 没有 GPU 时为空数组；有 GPU 才在概览里多出一张卡。 */
  gpus: GpuStat[];
  disks: DiskUsage[];
  block_disks: BlockDisk[];
  /** ZFS 池清单；没装 ZFS 时为空。 */
  zfs_pools: ZfsPool[];
  network: NetInterface[];
  disk_io: DiskIo[];
  processes: ProcessRow[];
  containers: ContainerSummary;
  modules: AdminModule[];
  degraded: string[];
}

export interface ApiError {
  code: string;
  message: string;
  field?: string;
}

export interface ErrorBody {
  error: ApiError;
}

// 资源级 API 封装：views/models 只能从这里取数据，不直接碰 http/fetch。

import { http } from './client';
import type {
  AcceptHostKeyRequest,
  AppGroup,
  AppGroupList,
  AutostartStatus,
  CreateCredentialRequest,
  CreateServerRequest,
  CreateTunnelRequest,
  CredentialList,
  CredentialSummary,
  GatewayCaStatus,
  GatewayTrustResult,
  HostKeyView,
  HostsStatus,
  HostsSyncResult,
  LogsResponse,
  ProxySyncResult,
  PatchCredentialRequest,
  PatchServerRequest,
  PatchTunnelRequest,
  ProbeDraftRequest,
  ProbeServerReport,
  ServerEnabledResult,
  SessionClosed,
  SessionState,
  ServerDto,
  ServerList,
  ServerOverview,
  SetAutostartRequest,
  ShutdownAccepted,
  TunnelList,
  TunnelView,
  WhoamiResponse,
} from './types';

export const serversApi = {
  list: () => http.get<ServerList>('/servers'),
  create: (req: CreateServerRequest) => http.post<ServerDto>('/servers', req),
  patch: (id: string, req: PatchServerRequest) => http.patch<ServerDto>(`/servers/${id}`, req),
  /** `cascade` 时连同它下面的端口转发一起删（服务端一次写完，不逐条删）。 */
  remove: (id: string, cascade = false) =>
    http.del<void>(`/servers/${id}${cascade ? '?cascade=true' : ''}`),
  reorder: (ids: string[]) => http.put<void>('/servers/order', { ids }),
  hostKey: (id: string) => http.get<HostKeyView>(`/servers/${id}/hostkey`),
  probe: (id: string) => http.post<ProbeServerReport>(`/servers/${id}/probe`),
  probeDraft: (req: ProbeDraftRequest) =>
    http.post<ProbeServerReport>('/probe-draft', req),
  session: (id: string) => http.get<SessionState>(`/servers/${id}/session`),
  setEnabled: (id: string, enabled: boolean) =>
    http.post<ServerEnabledResult>(`/servers/${id}/enabled`, { enabled }),
  disconnectSession: (id: string) =>
    http.post<SessionClosed>(`/servers/${id}/session/disconnect`),
  acceptHostKey: (id: string, req: AcceptHostKeyRequest) =>
    http.post<ServerDto>(`/servers/${id}/hostkey/accept`, req),
};

/**
 * 概览采集的口径。
 *
 * 按刷新频率拆开：`live` 每秒（CPU/内存/流量）、`disk-io` 每 5 秒、`disks` 每 10 秒、
 * `full` 只在进页面与手动刷新时跑。拆开的理由是每秒跑 `df` 会在挂了网络盘时拖住脚本。
 */
export type AdminProbe = 'full' | 'live' | 'disk-io' | 'disks';

/** 服务器管理：只读采集。后续模块（进程/文件/终端…）沿用 `/servers/{id}/admin/` 前缀。 */
export const adminApi = {
  overview: (id: string, probe: AdminProbe = 'full') =>
    http.getSlow<ServerOverview>(
      `/servers/${id}/admin/overview${probe === 'full' ? '' : `?probe=${probe}`}`,
    ),
};

export const tunnelsApi = {
  list: () => http.get<TunnelList>('/tunnels'),
  create: (req: CreateTunnelRequest) => http.post<TunnelView>('/tunnels', req),
  patch: (id: string, req: PatchTunnelRequest) => http.patch<TunnelView>(`/tunnels/${id}`, req),
  remove: (id: string) => http.del<void>(`/tunnels/${id}`),
  start: (id: string) => http.post<TunnelView>(`/tunnels/${id}/start`),
  stop: (id: string) => http.post<TunnelView>(`/tunnels/${id}/stop`),
  /** 应用页磁贴顺序：只写 app_order，不碰任何转发的运行态。 */
  reorderApp: (ids: string[]) => http.put<void>('/tunnels/app-order', { ids }),
};

/**
 * 应用页布局：分组清单与磁贴顺序。
 *
 * 改名 / 删组由核心一次改完成员的 `group`，前端不做 N 次 PATCH——
 * 否则中途失败会留下"组名改了、成员还在旧组"的半截状态。
 */
export const appGroupsApi = {
  list: () => http.get<AppGroupList>('/app-groups'),
  /** 全量覆盖：新建、删除空组、调整组顺序。 */
  setAll: (items: AppGroup[]) => http.put<AppGroupList>('/app-groups', { items }),
  rename: (from: string, to: string) =>
    http.post<AppGroupList>('/app-groups/rename', { from, to }),
  remove: (name: string) => http.post<AppGroupList>('/app-groups/delete', { name }),
};

export const credentialsApi = {
  list: () => http.get<CredentialList>('/credentials'),
  create: (req: CreateCredentialRequest) => http.post<CredentialSummary>('/credentials', req),
  patch: (id: string, req: PatchCredentialRequest) =>
    http.patch<CredentialSummary>(`/credentials/${id}`, req),
  remove: (id: string) => http.del<void>(`/credentials/${id}`),
};

export const hostsApi = {
  status: () => http.get<HostsStatus>('/hosts'),
  // 409 = 需要提权，响应体里带着 `written/needs_elevation/status`，不是错误。
  sync: () => http.post<HostsSyncResult>('/hosts/sync', undefined, [409]),
  clear: () => http.post<HostsSyncResult>('/hosts/clear', undefined, [409]),
  proxyResult: (result: ProxySyncResult) => http.post<void>('/hosts/proxy-result', result),
  clearProxy: () => http.post<void>('/hosts/proxy-clear'),
};

export const gatewayApi = {
  status: () => http.get<GatewayCaStatus>('/gateway/ca'),
  reset: () => http.post<void>('/gateway/ca/reset'),
  trustResult: (result: GatewayTrustResult) =>
    http.post<void>('/gateway/ca/trust-result', result),
};

export const runtimeApi = {
  whoami: () => http.get<WhoamiResponse>('/whoami'),
  autostart: () => http.get<AutostartStatus>('/autostart'),
  setAutostart: (req: SetAutostartRequest) => http.post<AutostartStatus>('/autostart', req),
  shutdown: () => http.post<ShutdownAccepted>('/shutdown'),
  logs: (tail = 200) => http.get<LogsResponse>(`/logs?tail=${tail}`),
};

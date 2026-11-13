// 资源级 API 封装：views/models 只能从这里取数据，不直接碰 http/fetch。

import { http } from './client';
import type {
  AcceptHostKeyRequest,
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
  remove: (id: string) => http.del<void>(`/servers/${id}`),
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

export const tunnelsApi = {
  list: () => http.get<TunnelList>('/tunnels'),
  create: (req: CreateTunnelRequest) => http.post<TunnelView>('/tunnels', req),
  patch: (id: string, req: PatchTunnelRequest) => http.patch<TunnelView>(`/tunnels/${id}`, req),
  remove: (id: string) => http.del<void>(`/tunnels/${id}`),
  start: (id: string) => http.post<TunnelView>(`/tunnels/${id}/start`),
  stop: (id: string) => http.post<TunnelView>(`/tunnels/${id}/stop`),
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

// 服务器与凭据 store：两者一起管理，因为凭据只被服务器引用。

import { defineStore } from 'pinia';
import { computed, ref } from 'vue';
import { ApiFailure } from '../api/client';
import { mergeSessionStates, type SessionState } from './sessionState';
import { credentialsApi, serversApi } from '../api';
import type {
  CreateCredentialRequest,
  CreateServerRequest,
  CredentialSummary,
  HostKeyView,
  PatchCredentialRequest,
  ProbeDraftRequest,
  ProbeServerReport,
  PatchServerRequest,
  ServerDto,
} from '../api/types';

export const useServersStore = defineStore('servers', () => {
  const servers = ref<ServerDto[]>([]);
  const credentials = ref<CredentialSummary[]>([]);
  const lastError = ref('');
  /** 每条隧道的内存态主机密钥视图（待确认指纹只存在于核心内存里）。 */
  const hostKeys = ref<Record<string, HostKeyView>>({});
  /** 每条隧道的 SSH 会话是否连着（会话按隧道共享）。 */
  const sessions = ref<Record<string, boolean>>({});
  /**
   * 会话状态是否**查到了**。
   *
   * 查询失败（核心刚重启、token 轮换、暂时不可达）与"确实没连上"是两件事，
   * 之前用 `?? false` 一律当成未连接——于是界面会在一切正常时显示"未连接"，
   * 且没有任何提示。
   */
  const sessionKnown = ref<Record<string, boolean>>({});

  const credentialOptions = computed(() =>
    credentials.value.map((c) => ({
      value: c.id,
      label: `${c.name || c.username}（${c.username} · ${c.kind === 'password' ? '密码' : '私钥'}）`,
    })),
  );

  async function refresh(): Promise<void> {
    try {
      servers.value = (await serversApi.list()).items;
      credentials.value = (await credentialsApi.list()).items;
      // 待确认指纹只存在于核心内存，列表接口里没有，逐条取一次（条数很少）。
      const views = await Promise.all(
        servers.value.map((s) => serversApi.hostKey(s.id).catch(() => null)),
      );
      const next: Record<string, HostKeyView> = {};
      for (const view of views) {
        if (view) {
          next[view.server_id] = view;
        }
      }
      hostKeys.value = next;

      const sessionViews = await Promise.all(
        servers.value.map((s) => serversApi.session(s.id).catch(() => null)),
      );
      // 只更新查到的；查不到就保留上一次的值，别把"不知道"写成"没连上"。
      const previous: Record<string, SessionState> = {};
      for (const server of servers.value) {
        previous[server.id] = {
          connected: sessions.value[server.id] === true,
          known: sessionKnown.value[server.id] === true,
        };
      }
      const merged = mergeSessionStates(
        previous,
        servers.value.map((s) => s.id),
        sessionViews,
      );
      const sessionMap: Record<string, boolean> = {};
      const knownMap: Record<string, boolean> = {};
      for (const [id, state] of Object.entries(merged)) {
        sessionMap[id] = state.connected;
        knownMap[id] = state.known;
      }
      sessions.value = sessionMap;
      sessionKnown.value = knownMap;
      lastError.value = '';
    } catch (e) {
      lastError.value = e instanceof ApiFailure ? e.message : String(e);
    }
  }

  async function createServer(req: CreateServerRequest): Promise<boolean> {
    return guard(async () => {
      await serversApi.create(req);
    });
  }

  /** 试连一次（TCP → 握手 → 指纹 → 认证），返回卡在哪一步。 */
  async function probeServer(id: string): Promise<ProbeServerReport | null> {
    try {
      const report = await serversApi.probe(id);
      lastError.value = '';
      return report;
    } catch (e) {
      lastError.value = e instanceof ApiFailure ? e.message : String(e);
      return null;
    }
  }

  /** 保存前试连：拿表单里的值直接探，不落库。 */
  async function probeDraft(req: ProbeDraftRequest): Promise<ProbeServerReport | null> {
    try {
      const report = await serversApi.probeDraft(req);
      lastError.value = '';
      return report;
    } catch (e) {
      lastError.value = e instanceof ApiFailure ? e.message : String(e);
      return null;
    }
  }

  async function updateServer(id: string, req: PatchServerRequest): Promise<boolean> {
    return guard(async () => {
      await serversApi.patch(id, req);
    });
  }

  /** 重排隧道显示顺序；成功后重新拉列表，顺序以服务端为准。 */
  async function reorder(ids: string[]): Promise<boolean> {
    return guard(async () => {
      await serversApi.reorder(ids);
    });
  }

  /**
   * 删除连接。
   *
   * `cascade` 交给服务端一次做完：以前是前端 for 循环逐条删转发，40 条转发
   * 就会打 40 次请求、写 40 次盘、同步 40 次 hosts。
   */
  async function removeServer(id: string, cascade = false): Promise<boolean> {
    return guard(async () => {
      await serversApi.remove(id, cascade);
    });
  }

  async function createCredential(req: CreateCredentialRequest): Promise<boolean> {
    return guard(async () => {
      await credentialsApi.create(req);
    });
  }

  /** 局部更新：只传 name 就是改名；传 secret 才换密钥。 */
  async function updateCredential(id: string, req: PatchCredentialRequest): Promise<boolean> {
    return guard(async () => {
      await credentialsApi.patch(id, req);
    });
  }

  async function removeCredential(id: string): Promise<boolean> {
    return guard(async () => {
      await credentialsApi.remove(id);
    });
  }

  /** 首次连接后确认主机密钥（不在任何地方自动接受）。 */
  async function acceptHostKey(serverId: string, reject: boolean): Promise<boolean> {
    return guard(async () => {
      await serversApi.acceptHostKey(serverId, { reject });
    });
  }

  async function guard(action: () => Promise<void>): Promise<boolean> {
    // 与 tunnels store 同理：动作成功后那次刷新不能把失败信息抹掉。
    lastError.value = '';
    try {
      await action();
      await refresh();
      return true;
    } catch (e) {
      lastError.value = e instanceof ApiFailure ? e.message : String(e);
      return false;
    }
  }

  /** 这条隧道是否处于「启用」状态（端口转发依赖它）。 */
  function serverEnabled(id: string): boolean {
    return servers.value.find((s) => s.id === id)?.enabled ?? false;
  }

  function serverName(id: string): string {
    return servers.value.find((s) => s.id === id)?.name ?? id;
  }

  /**
   * 打开/关闭一条隧道。
   *
   * 开关只表示**用户意图**（由核心持久化）：连不上不会把开关弹回去，
   * 关闭时挂在上面的端口一起停。
   */
  async function setServerEnabled(id: string, enabled: boolean): Promise<number | null> {
    try {
      const result = await serversApi.setEnabled(id, enabled);
      lastError.value = '';
      await refresh();
      return result.affected_forwards;
    } catch (e) {
      lastError.value = e instanceof ApiFailure ? e.message : String(e);
      return null;
    }
  }

  /** 断开某条隧道的会话：挂在上面的端口全部停止。 */
  async function disconnectSession(id: string): Promise<number | null> {
    try {
      const result = await serversApi.disconnectSession(id);
      lastError.value = '';
      await refresh();
      return result.stopped_forwards;
    } catch (e) {
      lastError.value = e instanceof ApiFailure ? e.message : String(e);
      return null;
    }
  }

  function sessionConnected(id: string): boolean {
    return sessions.value[id] === true;
  }

  /** 会话状态是否查到了（false = 核心不可达/刚重启，不能断言"没连上"）。 */
  function sessionKnownOf(id: string): boolean {
    return sessionKnown.value[id] === true;
  }

  /** 核心离线时清空，避免展示已经过期的服务器/凭据。 */
  function reset(): void {
    servers.value = [];
    credentials.value = [];
    hostKeys.value = {};
    sessionKnown.value = {};
    lastError.value = '';
  }

  /** 刚握过手但还没确认的指纹；没有则返回 null（表示尚未连接过）。 */
  function pendingFingerprint(id: string): string | null {
    return hostKeys.value[id]?.pending_fingerprint ?? null;
  }

  return {
    servers,
    credentials,
    credentialOptions,
    hostKeys,
    sessions,
    sessionConnected,
    sessionKnownOf,
    setServerEnabled,
    disconnectSession,
    reset,
    lastError,
    pendingFingerprint,
    refresh,
    createServer,
    probeServer,
  probeDraft,
    updateServer,
    removeServer,
    reorder,
    createCredential,
    updateCredential,
    removeCredential,
    acceptHostKey,
    serverName,
    serverEnabled,
  };
});

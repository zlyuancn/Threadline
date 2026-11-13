// 运行态 store：实例信息、降级标记、自启开关。
// 规则：只有这里能决定"界面该显示什么状态"，views 只负责渲染。

import { defineStore } from 'pinia';
import { ref } from 'vue';
import { ApiFailure } from '../api/client';
import { runtimeApi } from '../api';
import type { AutostartStatus, Degradation, RunMode, WhoamiResponse } from '../api/types';

export type ConnectionState = 'unknown' | 'online' | 'offline';

/** 改自启开关的结果。`needsElevation` = 需要界面代弹一次 UAC。 */
export interface AutostartOutcome {
  ok: boolean;
  needsElevation?: boolean;
  message: string;
}

const RUN_MODE_LABEL: Record<RunMode, string> = {
  boot: '开机自启（系统身份）',
  manual: '手动启动（当前用户）',
  stand_in: '顶班（系统核心未起来）',
  fallback: '回退运行',
};

const DEGRADATION_LABEL: Record<Degradation, string> = {
  stand_in_core: '顶班核心：该由系统身份运行，当前是用户身份',
  fallback_run: '回退运行：无法注册开机自启',
  credential_acl_relaxed: '凭据文件权限未收紧',
  credential_unreadable: '凭据不可读，界面已缩减',
  local_port_lost: '本地端口在运行期丢失',
};

export const useRuntimeStore = defineStore('runtime', () => {
  const connection = ref<ConnectionState>('unknown');
  const whoami = ref<WhoamiResponse | null>(null);
  const autostart = ref<AutostartStatus | null>(null);
  const lastError = ref<string>('');

  async function refresh(): Promise<void> {
    try {
      // 连接态不在这里判定：托盘与面板共用 agent 探活线程推送的状态（markOnline/markOffline）。
      whoami.value = await runtimeApi.whoami();
      autostart.value = whoami.value.autostart;
      lastError.value = '';
    } catch (e) {
      connection.value = 'offline';
      lastError.value = e instanceof ApiFailure ? e.message : String(e);
    }
  }

  /**
   * 改自启开关。返回结构化结果：`needsElevation` 时由界面去弹一次 UAC，
   * 而不是让用户自己去开管理员终端。
   */
  async function setAutostart(enabled: boolean): Promise<AutostartOutcome> {
    try {
      autostart.value = await runtimeApi.setAutostart({ enabled });
      lastError.value = '';
      // 开关只改注册层，当前进程仍在跑——提醒用户生效时机。
      return {
        ok: true,
        message: enabled ? '已开启开机自启（下次开机生效）' : '已关闭开机自启（下次开机生效）',
      };
    } catch (e) {
      if (e instanceof ApiFailure) {
        lastError.value = e.message;
        return { ok: false, needsElevation: e.needsElevation, message: e.message };
      }
      const message = String(e);
      lastError.value = message;
      return { ok: false, message };
    }
  }

  /** 请求核心优雅退出（停隧道 → 写 stopped → 进程退出）。 */
  async function shutdown(): Promise<void> {
    await runtimeApi.shutdown();
  }

  /// 心跳（隧道轮询）成功/失败时同步连接态，避免每次都为"核心还活着吗"去拉 whoami。
  function markOnline(): void {
    connection.value = 'online';
    lastError.value = '';
  }

  function markOffline(message: string): void {
    connection.value = 'offline';
    lastError.value = message;
  }

  function modeLabel(mode?: RunMode): string {
    return mode ? RUN_MODE_LABEL[mode] : '未知';
  }

  function degradationLabels(): string[] {
    const list = whoami.value?.degradations ?? [];
    return list.map((d) => DEGRADATION_LABEL[d] ?? d);
  }

  return {
    connection,
    whoami,
    autostart,
    lastError,
    refresh,
    setAutostart,
    shutdown,
    markOnline,
    markOffline,
    modeLabel,
    degradationLabels,
  };
});

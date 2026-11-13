// 隧道 store：列表、启停、增删。

import { defineStore } from 'pinia';
import { computed, ref } from 'vue';
import { ApiFailure } from '../api/client';
import { useTrafficStore } from './traffic';
import { tunnelsApi } from '../api';
import type {
  CreateTunnelRequest,
  PatchTunnelRequest,
  TunnelState,
  TunnelView,
} from '../api/types';

const STATE_LABEL: Record<TunnelState, string> = {
  stopped: '已停止',
  connecting: '连接中',
  needs_trust: '等待确认主机密钥',
  running: '运行中',
  backoff: '重连中',
  error: '错误',
};

const STATE_TYPE: Record<TunnelState, 'info' | 'warning' | 'success' | 'danger'> = {
  stopped: 'info',
  connecting: 'warning',
  needs_trust: 'warning',
  running: 'success',
  backoff: 'warning',
  error: 'danger',
};

export const useTunnelsStore = defineStore('tunnels', () => {
  const items = ref<TunnelView[]>([]);
  const loading = ref(false);
  const lastError = ref('');

  const runningCount = computed(
    () => items.value.filter((v) => v.status.state === 'running').length,
  );

  /// 返回是否成功：调用方据此判断"核心还活着吗"，决定下一次轮询间隔。
  async function refresh(): Promise<boolean> {
    loading.value = true;
    try {
      items.value = (await tunnelsApi.list()).items;
      // 采样点跟着刷新走：速率是"两次刷新之间的增量"，不是后端算好的。
      useTrafficStore().sample(items.value);
      // 这里**不能**清 lastError：增删改之后紧跟着一次刷新，
      // 一清就把"端口冲突"之类的失败信息抹掉，界面于是照样报成功。
      return true;
    } catch (e) {
      lastError.value = e instanceof ApiFailure ? e.message : String(e);
      return false;
    } finally {
      loading.value = false;
    }
  }

  async function start(id: string): Promise<void> {
    await guard(() => tunnelsApi.start(id));
    await refresh();
  }

  async function stop(id: string): Promise<void> {
    await guard(() => tunnelsApi.stop(id));
    await refresh();
  }

  async function create(req: CreateTunnelRequest): Promise<void> {
    await guard(() => tunnelsApi.create(req));
    await refresh();
  }

  /** 改端口转发配置。运行中的转发会被核心停掉（配置变了就没法继续用旧端口）。 */
  async function update(id: string, req: PatchTunnelRequest): Promise<void> {
    await guard(async () => {
      await tunnelsApi.patch(id, req);
    });
  }

  async function remove(id: string): Promise<void> {
    await guard(() => tunnelsApi.remove(id));
    await refresh();
  }

  async function setAutoStart(id: string, autoStart: boolean): Promise<void> {
    await guard(() => tunnelsApi.patch(id, { auto_start: autoStart }));
    await refresh();
  }

  async function guard(action: () => Promise<unknown>): Promise<void> {
    // 每次动作开始先清空，避免把上一次的错误当成这一次的结果。
    lastError.value = '';
    try {
      await action();
    } catch (e) {
      lastError.value = e instanceof ApiFailure ? e.message : String(e);
    }
  }

  function stateLabel(state: TunnelState): string {
    return STATE_LABEL[state] ?? state;
  }

  function stateType(state: TunnelState): 'info' | 'warning' | 'success' | 'danger' {
    return STATE_TYPE[state] ?? 'info';
  }

  /** 删除/编辑的可用性由状态决定（界面规则集中在这里，不散落到 views）。 */
  /**
   * 核心没了就把表清空。
   *
   * 否则会留下"运行中"的假象：核心已被杀，界面还在显示绿色 Running，
   * 用户会以为转发还在工作。宁可空表，也不要撒谎。
   */
  function reset(): void {
    items.value = [];
    lastError.value = '';
  }

  /**
   * 还能不能删。
   *
   * 以前这里是"运行中/连接中不可删"，结果按钮直接变灰——点了没反应也没有解释，
   * 用户只能以为坏了。后端删除时会先停止再移除，所以这里不再拦，由调用方用
   * 确认框讲清"会先停止它"。
   */
  function canRemove(): boolean {
    return true;
  }

  return {
    items,
    loading,
    lastError,
    runningCount,
    refresh,
    start,
    stop,
    create,
    update,
    remove,
    setAutoStart,
    reset,
    stateLabel,
    stateType,
    canRemove,
  };
});

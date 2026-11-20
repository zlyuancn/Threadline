// 虚拟机管理页的状态与展示口径。
//
// 分工：核心负责「跑什么 virsh、怎么解析」（tl-core 的 server_admin::vms），
// 这里只管「当前看的是哪台、上一次结果是什么、累计值怎么变成速率」。
//
// 与容器管理的一处差异：虚拟机没有单独的 stats 档。`virsh domstats` 给的
// `cpu.time` / `net.N.rx.bytes` 都是累计值，两次列表采样就能算出 CPU% 与网速，
// 再多开一条接口只会多一次 SSH 往返。所以列表 4 秒一档，两个环都靠它喂。

import { defineStore } from 'pinia';
import { computed, ref } from 'vue';
import { ApiFailure } from '../api/client';
import { adminApi } from '../api';
import type { VmAction, VmDetail, VmItem, VmList } from '../api/types';
import { formatBytes } from './serverAdmin';

export type Density = 'compact' | 'detail';
export type Filter = 'all' | 'running' | 'stopped';

export const VM_ACTION_LABEL: Record<VmAction, string> = {
  start: '开机',
  shutdown: '关机',
  reboot: '重启',
  suspend: '暂停',
  resume: '继续',
  destroy: '强制断电',
};

/**
 * 当前状态可用的动作。**与核心 `vms.rs::allowed_actions` 一一对应**，
 * 改一处必须改另一处：这张表决定菜单给哪几项，核心那张表决定请求放不放行。
 */
const ACTIONS_BY_STATE: Record<string, VmAction[]> = {
  running: ['shutdown', 'reboot', 'suspend', 'destroy'],
  paused: ['resume', 'destroy'],
  'shut off': ['start'],
  crashed: ['destroy'],
  pmsuspended: ['destroy'],
};

export function allowedActions(state: string): VmAction[] {
  return ACTIONS_BY_STATE[state] ?? ['start'];
}

const STATE_LABEL: Record<string, string> = {
  running: '运行中',
  paused: '已暂停',
  'shut off': '已关机',
  crashed: '已崩溃',
  pmsuspended: '已挂起',
};

export function stateLabel(state: string): string {
  return STATE_LABEL[state] ?? state;
}

/** 状态点的语义色：和容器列表同一个口径。 */
export function stateTone(state: string): 'ok' | 'warn' | 'bad' | 'muted' {
  if (state === 'running') return 'ok';
  if (state === 'paused' || state === 'pmsuspended') return 'warn';
  if (state === 'crashed') return 'bad';
  return 'muted';
}

/** KiB → `15.2 GB`。0 表示没采到，调用方按 `—` 处理。 */
export function formatKib(kib: number, digits = 1): string {
  return formatBytes(kib * 1024, digits);
}

/** 环形里要拆成「数字 + 单位」两行才放得下。 */
export function splitKib(kib: number): [string, string] {
  const parts = formatKib(kib).split(' ');
  return [parts[0] ?? '—', parts[1] ?? ''];
}

/** 实占 / 分配，夹在 0–100（balloon 偶尔会给出比分配略大的 rss）。 */
export function memoryPercent(item: VmItem): number {
  if (!item.memory_kib || !item.memory_used_kib) return 0;
  return Math.max(0, Math.min(100, (item.memory_used_kib / item.memory_kib) * 100));
}

/** 只算真正的磁盘（光驱不算容量，它是个空槽）。 */
export function dataDisks(item: VmItem) {
  return item.disks.filter((disk) => disk.device !== 'cdrom');
}

export function diskTotalBytes(item: VmItem): number {
  return dataDisks(item).reduce((sum, disk) => sum + disk.capacity_bytes, 0);
}

const DENSITY_KEY = 'tl.vms.density';

function readDensity(): Density {
  try {
    return localStorage.getItem(DENSITY_KEY) === 'compact' ? 'compact' : 'detail';
  } catch {
    return 'detail';
  }
}

/** 一行资源：CPU 百分比（null = 还没第二拍）与网速。 */
export interface VmMetric {
  cpu: number | null;
  rx: number;
  tx: number;
  /** 这次差分覆盖多少秒，界面用来解释「4 秒平均」。 */
  seconds: number;
}

interface Sample {
  at: number;
  cpuNs: number;
  rx: number;
  tx: number;
}

export const useVmsStore = defineStore('vms', () => {
  const serverId = ref('');
  const list = ref<VmList | null>(null);
  const detail = ref<VmDetail | null>(null);
  const metrics = ref<Record<string, VmMetric>>({});
  const samples = ref<Record<string, Sample>>({});

  const loading = ref(false);
  const detailLoading = ref(false);
  const error = ref('');
  /** 后台轮询失败：不夺走已有数据，只在角落标一下。 */
  const lightError = ref('');
  const feedback = ref('');
  const feedbackIsError = ref(false);

  const filter = ref<Filter>('all');
  const query = ref('');
  const density = ref<Density>(readDensity());

  const items = computed<VmItem[]>(() => list.value?.items ?? []);

  const counts = computed(() => ({
    total: list.value?.total ?? 0,
    running: list.value?.running ?? 0,
    paused: list.value?.paused ?? 0,
    shutoff: list.value?.shutoff ?? 0,
    crashed: list.value?.crashed ?? 0,
  }));

  const visible = computed<VmItem[]>(() => {
    const needle = query.value.trim().toLowerCase();
    return items.value.filter((item) => {
      if (filter.value === 'running' && item.state !== 'running') return false;
      if (filter.value === 'stopped' && item.state === 'running') return false;
      if (!needle) return true;
      return (
        item.name.toLowerCase().includes(needle) ||
        item.uuid.toLowerCase().includes(needle) ||
        item.nets.some((net) => net.ip.includes(needle))
      );
    });
  });

  /** 有 virsh 但一台都没有 ≠ 没装 libvirt，两种空态文案不同。 */
  const hasEngine = computed(() => list.value?.engine != null);

  function metricFor(name: string): VmMetric {
    return metrics.value[name] ?? { cpu: null, rx: 0, tx: 0, seconds: 0 };
  }

  function setDensity(next: Density): void {
    density.value = next;
    try {
      localStorage.setItem(DENSITY_KEY, next);
    } catch {
      /* 无痕模式：只是不记住，不影响这次切换 */
    }
  }

  function showFeedback(message: string, isError = false): void {
    feedback.value = message;
    feedbackIsError.value = isError;
  }

  function clearFeedback(): void {
    feedback.value = '';
    feedbackIsError.value = false;
  }

  /**
   * 用「上一拍累计值」换这一拍的 CPU% 与网速。
   *
   * 采样窗口短于 1 秒就不算：动作之后会立刻补采一次（`runAction` 里），
   * 那一拍与上一拍可能只隔几十毫秒，拿它当分母会把百分比的噪声放大十倍。
   */
  function resample(rows: VmItem[]): void {
    const now = Date.now();
    const previous = samples.value;
    const nextSamples: Record<string, Sample> = {};
    const nextMetrics: Record<string, VmMetric> = {};

    for (const item of rows) {
      const sample: Sample = {
        at: now,
        cpuNs: item.cpu_time_ns,
        rx: item.net_rx_bytes,
        tx: item.net_tx_bytes,
      };
      const before = previous[item.name];
      if (before && now - before.at >= 1000) {
        const seconds = (now - before.at) / 1000;
        const cores = Math.max(1, item.vcpu);
        // cpu.time 是纳秒累计；除以 vCPU 数才是「占分配的百分比」，0–100 正好喂环。
        const busy = Math.max(0, sample.cpuNs - before.cpuNs) / 1e9;
        nextMetrics[item.name] = {
          cpu: Math.min(100, (busy / seconds / cores) * 100),
          rx: Math.max(0, sample.rx - before.rx) / seconds,
          tx: Math.max(0, sample.tx - before.tx) / seconds,
          seconds,
        };
      } else {
        // 第一拍（或窗口太短）：沿用上一次的读数，并且**不覆盖起点**——
        // 下一次就从那个起点重新算，窗口自然还是 4 秒。
        nextMetrics[item.name] = metrics.value[item.name] ?? { cpu: null, rx: 0, tx: 0, seconds: 0 };
      }
      nextSamples[item.name] = before && now - before.at < 1000 ? before : sample;
    }

    samples.value = nextSamples;
    metrics.value = nextMetrics;
  }

  async function load(id: string): Promise<boolean> {
    if (serverId.value !== id) {
      serverId.value = id;
      list.value = null;
      detail.value = null;
      samples.value = {};
      metrics.value = {};
      error.value = '';
      lightError.value = '';
    }
    const first = list.value == null;
    if (first) loading.value = true;
    try {
      const result = await adminApi.vms(id);
      list.value = result;
      resample(result.items);
      error.value = '';
      lightError.value = '';
      return true;
    } catch (e) {
      const message = e instanceof ApiFailure ? e.message : `${e}`;
      if (first) error.value = message;
      else lightError.value = message;
      return false;
    } finally {
      if (first) loading.value = false;
    }
  }

  async function loadDetail(name: string): Promise<void> {
    if (!serverId.value) return;
    detailLoading.value = true;
    try {
      detail.value = await adminApi.vmDetail(serverId.value, name);
    } catch (e) {
      detail.value = null;
      showFeedback(e instanceof ApiFailure ? e.message : `${e}`, true);
    } finally {
      detailLoading.value = false;
    }
  }

  function closeDetail(): void {
    detail.value = null;
  }

  async function runAction(item: VmItem, action: VmAction): Promise<boolean> {
    try {
      const result = await adminApi.vmAction(serverId.value, item.name, action);
      showFeedback(`${VM_ACTION_LABEL[action]}：${result.message}（当前 ${stateLabel(result.state)}）`);
      await load(serverId.value);
      if (detail.value?.item.name === item.name) await loadDetail(item.name);
      return true;
    } catch (e) {
      showFeedback(e instanceof ApiFailure ? e.message : `${e}`, true);
      return false;
    }
  }

  async function setAutostart(item: VmItem, enabled: boolean): Promise<boolean> {
    try {
      const result = await adminApi.vmAutostart(serverId.value, item.name, enabled);
      showFeedback(result.message);
      await load(serverId.value);
      if (detail.value?.item.name === item.name) await loadDetail(item.name);
      return true;
    } catch (e) {
      showFeedback(e instanceof ApiFailure ? e.message : `${e}`, true);
      return false;
    }
  }

  return {
    serverId,
    list,
    detail,
    metrics,
    loading,
    detailLoading,
    error,
    lightError,
    feedback,
    feedbackIsError,
    filter,
    query,
    density,
    items,
    counts,
    visible,
    hasEngine,
    metricFor,
    setDensity,
    showFeedback,
    clearFeedback,
    load,
    loadDetail,
    closeDetail,
    runAction,
    setAutostart,
  };
});

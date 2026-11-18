// 容器管理页的状态与展示口径。
//
// 分工：核心负责「跑什么脚本、怎么解析」（tl-core 的 server_admin::containers），
// 这里只管「当前看的是哪台、上一次结果是什么、数字怎么显示成人能读的样子」。
//
// 采集分三档，互不拖累：
//   列表 5s（ps + inspect）· 资源 3s（stats，仅抽屉打开时）· 日志 2s（仅跟随开启时）

import { defineStore } from 'pinia';
import { computed, ref } from 'vue';
import { ApiFailure } from '../api/client';
import { adminApi } from '../api';
import type {
  ContainerAction,
  ContainerDetail,
  ContainerItem,
  ContainerList,
  ContainerLogs,
  ContainerStats,
} from '../api/types';
import { formatBytes } from './serverAdmin';

export type Density = 'compact' | 'detail';
export type Filter = 'all' | 'running' | 'stopped';

export const ACTION_LABEL: Record<ContainerAction, string> = {
  start: '启动',
  stop: '停止',
  restart: '重启',
};

/**
 * 当前状态可用的动作。
 *
 * **与核心 `containers.rs::allowed_actions` 一一对应**，改一处必须改另一处：
 * 前端这张表决定菜单给哪几项，核心那张表决定请求放不放行——两张表不一致时，
 * 用户会看到「菜单里有、点了报错」。
 */
const ACTIONS_BY_STATE: Record<string, ContainerAction[]> = {
  running: ['restart', 'stop'],
  // 被命令行 `docker pause` 冻住的容器只给「停止」：本产品不做暂停，
  // 也就不留「继续」这个语义模糊的入口；停掉再启动是唯一出路。
  paused: ['stop'],
  restarting: ['stop'],
};

export function allowedActions(state: string): ContainerAction[] {
  return ACTIONS_BY_STATE[state] ?? ['start'];
}

const DENSITY_KEY = 'tl.containers.density';

function readDensity(): Density {
  try {
    return localStorage.getItem(DENSITY_KEY) === 'compact' ? 'compact' : 'detail';
  } catch {
    return 'detail';
  }
}

export const useContainersStore = defineStore('containers', () => {
  const serverId = ref('');
  const list = ref<ContainerList | null>(null);
  const stats = ref<Record<string, ContainerStats>>({});
  /**
   * 上一拍的累计字节与时刻。
   *
   * `docker stats` 只给累计收发（`net_rx_bytes` / `net_tx_bytes`），要看「现在多快」
   * 必须自己差分，而差分得有上一拍——核心侧是无状态的，这份历史只能放在前端。
   */
  const samples = ref<Record<string, { rx: number; tx: number; at: number }>>({});
  /**
   * 网络速率（字节/秒），附带这次差分覆盖了多少秒。
   *
   * 窗口必须一起带出去：列表 15 秒补一次 stats，抽屉开着时 3 秒一次，
   * 同样是「速率」，一个是 15 秒平均一个接近瞬时，不说明白会被当成同一件事。
   */
  const rates = ref<Record<string, { rx: number; tx: number; seconds: number }>>({});
  const detail = ref<ContainerDetail | null>(null);
  const logs = ref<ContainerLogs | null>(null);

  const loading = ref(false);
  const error = ref('');
  /** 后台轮询失败：不夺走已有数据，只在角落标一下。 */
  const lightError = ref('');
  /** 写操作的回执（成功绿、失败红），4 秒后由视图清掉。 */
  const feedback = ref('');
  const feedbackIsError = ref(false);

  const filter = ref<Filter>('all');
  const query = ref('');
  const density = ref<Density>(readDensity());

  /**
   * 按 id 找这一行最近一次的资源。
   *
   * 两档的 id 长度未必完全一致（`ps` 与 `stats` 都是短 id，但不同 docker 版本
   * 有过差异），所以先精确匹配再按前缀兜底——匹配不上就当作「还没采」。
   */
  function statsFor(id: string): ContainerStats | undefined {
    const direct = stats.value[id];
    if (direct) return direct;
    return Object.values(stats.value).find(
      (row) => row.id.startsWith(id) || id.startsWith(row.id),
    );
  }

  /** 列表项 + 最近一次资源：两档数据合起来才是完整一行。 */
  const items = computed<ContainerItem[]>(() => {
    const rows = list.value?.items ?? [];
    return rows.map((item) => {
      const s = statsFor(item.id);
      return s
        ? {
            ...item,
            cpu_percent: s.cpu_percent,
            mem_bytes: s.mem_bytes,
            mem_limit_bytes: s.mem_limit_bytes,
            net_rx_bytes: s.net_rx_bytes,
            net_tx_bytes: s.net_tx_bytes,
          }
        : item;
    });
  });

  const counts = computed(() => ({
    total: list.value?.total ?? 0,
    running: list.value?.running ?? 0,
    exited: list.value?.exited ?? 0,
    paused: list.value?.paused ?? 0,
  }));

  const visible = computed<ContainerItem[]>(() => {
    const needle = query.value.trim().toLowerCase();
    return items.value.filter((item) => {
      if (filter.value === 'running' && item.state !== 'running') return false;
      if (filter.value === 'stopped' && item.state === 'running') return false;
      if (!needle) return true;
      return (
        item.name.toLowerCase().includes(needle) ||
        item.image.toLowerCase().includes(needle) ||
        item.id.toLowerCase().includes(needle)
      );
    });
  });

  /** 有引擎但一个容器都没有 ≠ 没有引擎，两种空态文案不同。 */
  const hasEngine = computed(() => list.value?.engine != null);

  /**
   * 这一行有没有采到资源。
   *
   * 资源是单独一档（3s，只在抽屉打开时跑），没采到时 `cpu_percent` 是 0——
   * 直接显示会让人以为「这个容器 CPU 占用 0%」，而真相是「还没采」。
   */
  function hasStats(id: string): boolean {
    return statsFor(id) != null;
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

  async function load(id: string): Promise<boolean> {
    if (serverId.value !== id) {
      serverId.value = id;
      list.value = null;
      stats.value = {};
      // 换了服务器，上一台的累计值毫无意义——留着会让第一条速率串台
      samples.value = {};
      rates.value = {};
      detail.value = null;
      logs.value = null;
    }
    // 首次进入才占满屏；后续轮询不闪骨架。
    const first = list.value == null;
    if (first) loading.value = true;
    try {
      // 首屏把资源档一起拉。`stats --no-stream` 自带采样窗口，实测约 2 秒，
      // 但让它并着列表跑，用户看到的是「列表先出来、2 秒后数值自己填上」；
      // 不这么做的话 CPU / MEM / NET 三列会一直是「—」，整块看着像坏了。
      const [rows] = await Promise.all([
        adminApi.containers(id),
        first ? loadStats() : Promise.resolve(),
      ]);
      list.value = rows;
      error.value = '';
      lightError.value = '';
      return true;
    } catch (e) {
      const message = e instanceof ApiFailure ? e.message : String(e);
      if (first) error.value = message;
      else lightError.value = message;
      return false;
    } finally {
      if (first) loading.value = false;
    }
  }

  /** 资源档：失败不报错——列表照样能用，只是 CPU/内存显示 —。 */
  async function loadStats(): Promise<void> {
    if (!serverId.value) return;
    try {
      const now = Date.now();
      const rows = await adminApi.containerStats(serverId.value);
      const map: Record<string, ContainerStats> = {};
      const nextSamples: Record<string, { rx: number; tx: number; at: number }> = {};
      const nextRates: Record<string, { rx: number; tx: number; seconds: number }> = {};

      for (const row of rows) {
        map[row.id] = row;

        const prev = samples.value[row.id];
        const seconds = prev ? (now - prev.at) / 1000 : 0;
        if (prev && seconds >= 0.5) {
          const dRx = row.net_rx_bytes - prev.rx;
          const dTx = row.net_tx_bytes - prev.tx;
          // 容器重启会把累计值清零，负差值不能当速率（会变成负的「倒流」）
          nextRates[row.id] = {
            rx: dRx >= 0 ? dRx / seconds : 0,
            tx: dTx >= 0 ? dTx / seconds : 0,
            seconds,
          };
        } else if (rates.value[row.id]) {
          // 两次采样挨得太近（列表 15s 与抽屉 3s 撞在一起）：沿用上一拍，
          // 否则一除就抖成 0
          nextRates[row.id] = rates.value[row.id];
        }

        nextSamples[row.id] = { rx: row.net_rx_bytes, tx: row.net_tx_bytes, at: now };
      }

      stats.value = map;
      samples.value = nextSamples;
      rates.value = nextRates;
    } catch {
      /* 静默：stats 是锦上添花 */
    }
  }

  /** 这一行最近一次算出的网络速率；还没采够两拍就是 undefined。 */
  function rateFor(id: string): { rx: number; tx: number; seconds: number } | undefined {
    const direct = rates.value[id];
    if (direct) return direct;
    // 和 `statsFor` 同样的前缀兜底：`ps` 与 `stats` 的短 id 长度未必一致
    const key = Object.keys(rates.value).find(
      (k) => k.startsWith(id) || id.startsWith(k),
    );
    return key ? rates.value[key] : undefined;
  }

  async function loadDetail(cid: string): Promise<void> {
    if (!serverId.value) return;
    try {
      detail.value = await adminApi.containerDetail(serverId.value, cid);
    } catch (e) {
      detail.value = null;
      showFeedback(e instanceof ApiFailure ? e.message : String(e), true);
    }
  }

  async function loadLogs(cid: string, tail = 200, since?: string): Promise<void> {
    if (!serverId.value) return;
    try {
      const next = await adminApi.containerLogs(serverId.value, cid, tail, since);
      const prev = logs.value;

      // 跟随模式是 `--since` **增量**：返回的只有两次之间的新行。
      // 直接覆盖会把刚拉到的 200 行顶成最新一条（真机撞到：打开满屏，两秒后只剩一行）。
      if (!since || !prev || prev.lines.length === 0) {
        logs.value = next;
        return;
      }

      // `docker logs --since` 是 `>=`：上一批的最后一行每次都会被重新带回来。
      // 时间戳是定长的 RFC3339，直接按字典序比就能只留真正新增的。
      const lastTs = prev.lines[prev.lines.length - 1]?.split(/\s+/)[0] ?? '';
      const fresh = lastTs
        ? next.lines.filter((line) => (line.split(/\s+/)[0] ?? '') > lastTs)
        : next.lines;

      const merged = [...prev.lines, ...fresh];
      logs.value = {
        ...next,
        // 上限仍是 tail：长时间跟随不会把内存吃穿，滚动条也不会无限长
        lines: merged.length > tail ? merged.slice(merged.length - tail) : merged,
        // 任一段被截断都算截断，否则「只显示最后 200 行」的提示会中途消失
        truncated: prev.truncated || next.truncated,
      };
    } catch (e) {
      showFeedback(e instanceof ApiFailure ? e.message : String(e), true);
    }
  }

  /**
   * 写操作。
   *
   * 成功后**就地改状态**再等下一拍：否则点了停止，行还是「运行中」，
   * 用户会以为没生效而再点一次。
   */
  async function act(cid: string, action: ContainerAction): Promise<boolean> {
    if (!serverId.value) return false;
    try {
      const result = await adminApi.containerAction(serverId.value, cid, action);
      const row = list.value?.items.find((i) => i.name === cid || i.id === cid);
      if (row && result.state) {
        row.state = result.state;
        row.status = statusFor(result.state);
      }
      showFeedback(result.message || `已${ACTION_LABEL[action]}`);
      await load(serverId.value);
      return result.ok;
    } catch (e) {
      showFeedback(e instanceof ApiFailure ? e.message : String(e), true);
      return false;
    }
  }

  async function remove(cid: string, force: boolean, image: boolean): Promise<boolean> {
    if (!serverId.value) return false;
    try {
      const result = await adminApi.containerRemove(serverId.value, cid, force, image);
      showFeedback(result.message);
      if (detail.value?.item.name === cid) detail.value = null;
      await load(serverId.value);
      return result.ok;
    } catch (e) {
      showFeedback(e instanceof ApiFailure ? e.message : String(e), true);
      return false;
    }
  }

  function reset(): void {
    serverId.value = '';
    list.value = null;
    stats.value = {};
    samples.value = {};
    rates.value = {};
    detail.value = null;
    logs.value = null;
    loading.value = false;
    error.value = '';
    lightError.value = '';
    clearFeedback();
  }

  return {
    serverId,
    list,
    stats,
    detail,
    logs,
    loading,
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
    hasStats,
    rateFor,
    setDensity,
    showFeedback,
    clearFeedback,
    load,
    loadStats,
    loadDetail,
    loadLogs,
    act,
    remove,
    reset,
  };
});

/** 动作成功后的状态副行文案：真实值以核心回读为准，这里只是让行先跟上。 */
function statusFor(state: string): string {
  switch (state) {
    case 'running':
      return 'Up just now';
    case 'paused':
      return 'Up (Paused)';
    case 'restarting':
      return 'Restarting';
    default:
      return 'Exited (0) just now';
  }
}

// ── 展示口径 ──────────────────────────────────────────────

/** `ghcr.io/immich-app/immich-server:v1.119.0` → `{ repo: 'immich-app/immich-server', tag: ':v1.119.0' }`。 */
export function splitImage(image: string): { repo: string; tag: string } {
  const colon = image.lastIndexOf(':');
  const hasTag = colon > image.lastIndexOf('/');
  let repo = hasTag ? image.slice(0, colon) : image;
  const tag = hasTag ? image.slice(colon) : '';

  // 去掉 registry 前缀：十行里九行同一个源，留着只把 repo 挤成省略号。
  const parts = repo.split('/');
  if (
    parts.length > 1 &&
    (parts[0].includes('.') || parts[0].includes(':') || parts[0] === 'localhost')
  ) {
    repo = parts.slice(1).join('/');
  }
  return { repo, tag };
}

/** 主要视图只留最后一段：`immich-server:v1.119.0`。 */
export function shortImage(image: string): string {
  const { repo, tag } = splitImage(image);
  return `${repo.split('/').pop() ?? repo}${tag}`;
}

export function memText(item: ContainerItem): string {
  return item.mem_bytes > 0 ? formatBytes(item.mem_bytes) : '—';
}

export function memDetail(item: ContainerItem): string {
  if (item.mem_bytes <= 0) return '—';
  if (item.mem_limit_bytes <= 0) return formatBytes(item.mem_bytes);
  return `${formatBytes(item.mem_bytes)} / ${formatBytes(item.mem_limit_bytes)}`;
}

/** 内存占比（0–100）：没给上限时按 0 算，不画满格红条。 */
export function memPercent(item: ContainerItem): number {
  if (item.mem_bytes <= 0 || item.mem_limit_bytes <= 0) return 0;
  return Math.min(100, Math.round((item.mem_bytes / item.mem_limit_bytes) * 1000) / 10);
}

/** 阈值与其它面板一致：70 警告、90 危险。 */
export function levelClass(value: number): string {
  if (value >= 90) return 'is-bad';
  if (value >= 70) return 'is-warn';
  return 'is-ok';
}

/**
 * 收发的单位合并到末尾，省一半宽度。
 *
 * 注意这里画的是**累计字节**，不是速率：`docker stats` 只给累计值，
 * 要算速率得在模型里缓存上一拍，而核心侧是无状态的（见 `net_rx_bytes` 的注释）。
 * 所以函数名不叫 `ratePair`，免得后来人以为这里能看出「现在跑多快」。
 */
export function trafficPair(rx: number, tx: number): string {
  const max = Math.max(rx, tx);
  if (max < 1024) return `↑${Math.round(tx)} ↓${Math.round(rx)} B`;
  if (max < 1024 * 1024) {
    return `↑${(tx / 1024).toFixed(1)} ↓${(rx / 1024).toFixed(1)} KB`;
  }
  return `↑${(tx / 1024 / 1024).toFixed(1)} ↓${(rx / 1024 / 1024).toFixed(1)} MB`;
}

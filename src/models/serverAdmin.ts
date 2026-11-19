// 服务器管理页的状态与展示口径。
//
// 分工：核心负责"采什么、怎么算"（脚本与解析都在 tl-core 的 server_admin 模型里），
// 这里只负责"当前看的是哪台、上一次结果是什么、数字怎么显示成人能读的样子"。

import { defineStore } from 'pinia';
import { computed, ref } from 'vue';
import { ApiFailure } from '../api/client';
import { adminApi, type AdminProbe } from '../api';
import type { AdminModule, ServerOverview } from '../api/types';
import { formatRate } from './traffic';

export const useServerAdminStore = defineStore('serverAdmin', () => {
  /** 当前管理页对着哪条隧道。 */
  const serverId = ref('');
  const overview = ref<ServerOverview | null>(null);
  const loading = ref(false);
  const error = ref('');
  /** 自动刷新（轻量采集）最近一次的失败原因：不遮页面，只在角落提示。 */
  const lightError = ref('');

  /**
   * 导航清单。**只有核心给**：没采到之前没有清单，页面显示骨架，
   * 不在前端复制一份"假清单"——两处真相必然漂移。
   */
  const modules = computed<AdminModule[]>(() => overview.value?.modules ?? []);

  /** 采集失败的块（缺 docker、缺 /proc 等），页面顶部打一行提示。 */
  const degraded = computed<string[]>(() => overview.value?.degraded ?? []);

  /**
   * 采一次概览。返回是否成功——定时器据此决定下一次等多久。
   *
   * `probe` 决定采哪一块：`full` 覆盖全部（进页面、手动刷新），其余只更新对应字段。
   * 分开的意义是别用 10 秒级的数据去覆盖 1 秒级的数字，反之亦然。
   */
  async function load(id: string, probe: AdminProbe = 'full'): Promise<boolean> {
    // 换了隧道就把上一次的数据清掉：否则新机器上会闪一下旧机器的指标。
    if (serverId.value !== id) {
      serverId.value = id;
      overview.value = null;
    }
    const full = probe === 'full';
    if (full) {
      loading.value = true;
      error.value = '';
    }
    try {
      const next = await adminApi.overview(id, probe);
      const prev = overview.value;
      if (full || !prev) {
        overview.value = next;
      } else if (probe === 'live') {
        overview.value = {
          ...prev,
          collected_at: next.collected_at,
          elapsed_ms: next.elapsed_ms,
          // 系统信息基本不变，但首屏可能是 probe=live 抢到的：有值就用新的。
          system: next.system.hostname ? next.system : prev.system,
          cpu: next.cpu,
          memory: next.memory,
          // GPU 没有驱动时是空数组，但别把它当成"这次没采"而保留旧值：
          // 驱动卸载/重装后应该立刻消失，而不是挂着一块已经不存在的卡。
          gpus: next.gpus,
          network: next.network,
          // 磁盘 IO 与 CPU 同频（都读 /proc，一起采更省）
          disk_io: next.disk_io,
        };
      } else if (probe === 'disk-io') {
        overview.value = { ...prev, disk_io: next.disk_io };
      } else {
        // 磁盘容量、物理盘清单、ZFS 池、内存分源是同一次采集出来的，必须一起换。
        overview.value = {
          ...prev,
          disks: next.disks,
          block_disks: next.block_disks,
          zfs_pools: next.zfs_pools,
          mem_breakdown: next.mem_breakdown,
        };
      }
      lightError.value = '';
      return true;
    } catch (e) {
      const message = e instanceof ApiFailure ? e.message : String(e);
      if (full) {
        error.value = message;
      } else {
        // 后台刷新失败不夺走已有数据：页面继续显示上一份，角落标一下就够了。
        lightError.value = message;
      }
      return false;
    } finally {
      if (full) {
        loading.value = false;
      }
    }
  }

  /** 离开管理页：数据不留在内存里，下次进入重新采。 */
  function reset(): void {
    serverId.value = '';
    overview.value = null;
    loading.value = false;
    error.value = '';
    lightError.value = '';
  }

  return {
    serverId,
    overview,
    loading,
    error,
    lightError,
    modules,
    degraded,
    load,
    reset,
  };
});

/** 速率显示：核心已经给了字节/秒，这里只补一个 "0 B/s"（空串在表格里像缺数据）。 */
export function rateText(bytesPerSecond: number): string {
  return formatRate(bytesPerSecond) || '0 B/s';
}

const BYTE_UNITS = ['B', 'KB', 'MB', 'GB', 'TB', 'PB'];

/** 字节数 → 人能读的大小。`digits` 留给需要固定精度的场景（如磁盘）。 */
export function formatBytes(bytes: number, digits = 1): string {
  if (!Number.isFinite(bytes) || bytes <= 0) {
    return '0 B';
  }
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < BYTE_UNITS.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const fixed = unit === 0 ? 0 : digits;
  return `${value.toFixed(fixed)} ${BYTE_UNITS[unit]}`;
}

/** `/proc/meminfo` 口径是 kB，展示统一走字节换算。 */
export function formatKb(kb: number, digits = 1): string {
  return formatBytes(kb * 1024, digits);
}

/** 秒级运行时长 → `3 天 4 小时`。小于 1 小时时给到分钟。 */
/** 运行时长：精确到秒——概览每秒刷新，秒位会跟着走，看着才像"在跑"。 */
export function formatUptime(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds <= 0) {
    return '—';
  }
  const days = Math.floor(seconds / 86400);
  const hours = Math.floor((seconds % 86400) / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const secs = Math.floor(seconds % 60);
  if (days > 0) {
    return `${days} 天 ${hours} 小时 ${minutes} 分 ${secs} 秒`;
  }
  if (hours > 0) {
    return `${hours} 小时 ${minutes} 分 ${secs} 秒`;
  }
  return `${minutes} 分 ${secs} 秒`;
}

/** 百分比收敛到 0–100，并保留一位小数：`NaN` 会让进度条宽度变成无效值。 */
export function percent(value: number): number {
  if (!Number.isFinite(value) || value <= 0) {
    return 0;
  }
  return Math.min(100, Math.round(value * 10) / 10);
}

/**
 * 用量占比（0–100），入参是"已用 / 总量"，单位为任意一致单位。
 * 总量为 0 时返回 0，避免满格红条。
 */
export function usagePercent(used: number, total: number): number {
  if (!Number.isFinite(used) || !Number.isFinite(total) || total <= 0) {
    return 0;
  }
  return percent((used / total) * 100);
}

/** 进度条的语义色：越高越危险，与其它面板同一套阈值。 */
export function levelClass(value: number): string {
  if (value >= 90) {
    return 'is-bad';
  }
  if (value >= 70) {
    return 'is-warn';
  }
  return 'is-ok';
}

/** 采集时间 → `14:23:05`（只关心时分秒，跨天的情况在标题里另说）。 */
export function clockText(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) {
    return iso;
  }
  return date.toLocaleTimeString('zh-CN', { hour12: false });
}

/**
 * 页头切换器的可选项：剔除「本机」（没有会话，管理页对它没意义），
 * 再按名称或 host 过滤。关键词去空格、忽略大小写。
 *
 * 顺序不动：侧栏是用户自己排的序，管理页里换机器时也该是同一个顺序。
 */
export function pickableServers<T extends { name: string; host: string; kind?: string }>(
  servers: T[],
  keyword = '',
): T[] {
  const term = keyword.trim().toLowerCase();
  return servers
    .filter((server) => server.kind !== 'local')
    .filter(
      (server) =>
        !term ||
        server.name.toLowerCase().includes(term) ||
        server.host.toLowerCase().includes(term),
    );
}

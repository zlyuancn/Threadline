import { defineStore } from 'pinia';
import { computed, ref } from 'vue';
import type { TunnelView } from '../api/types';

/**
 * 速率显示。
 *
 * 后端只给累计字节（单调递增、重启转发时归零），速率必须由界面按时间窗口算：
 * 每次刷新采一次样，用「这次 - 上次」除以间隔。这样后端不需要维护定时器，
 * 也不需要把"每秒"这种展示口径写进协议。
 */
interface Sample {
  up: number;
  down: number;
  at: number;
}

/** 两次采样间隔太短时噪声很大（几百毫秒的抖动会放大成几倍速率）。 */
const MIN_INTERVAL_MS = 400;

export const useTrafficStore = defineStore('traffic', () => {
  const samples = ref<Record<string, Sample>>({});
  /** 每秒字节数，按转发 id。 */
  const rates = ref<Record<string, { up: number; down: number }>>({});
  /** 每个隧道下有哪些转发，用来汇总。 */
  const byServer = ref<Record<string, string[]>>({});

  function sample(items: TunnelView[]): void {
    const now = Date.now();
    const nextSamples: Record<string, Sample> = {};
    /**
     * 只保留本轮仍在列表里的转发。
     *
     * 之前这里从旧 `rates` 整体拷贝，转发的 key 就永远留着：删除或停止后
     * 那个速率不再更新，却仍然被 `totalSpeed` 累加——界面于是出现
     * 「0 条隧道正在运行，却挂着 ↑/↓ 速率」的怪象。
     */
    const nextRates: Record<string, { up: number; down: number }> = {};
    const owners: Record<string, string[]> = {};

    for (const item of items) {
      const id = item.definition.id;
      const up = item.status.bytes_up ?? 0;
      const down = item.status.bytes_down ?? 0;
      nextSamples[id] = { up, down, at: now };

      (owners[item.definition.server_id] ??= []).push(id);

      const previous = samples.value[id];
      if (!previous) {
        // 第一次见到：没有参照物，速率记 0，别把历史累计当成"刚刚跑了一整段"。
        nextRates[id] = { up: 0, down: 0 };
        continue;
      }
      const elapsed = now - previous.at;
      if (elapsed < MIN_INTERVAL_MS) {
        // 间隔太短算不准：沿用上一轮的速率，不要把它抹成 0 让数字闪一下。
        nextRates[id] = rates.value[id] ?? { up: 0, down: 0 };
        continue;
      }
      // 计数器归零（转发重启）：这一轮按 0 算，等下一轮再给数。
      const deltaUp = up >= previous.up ? up - previous.up : 0;
      const deltaDown = down >= previous.down ? down - previous.down : 0;
      nextRates[id] = {
        up: (deltaUp * 1000) / elapsed,
        down: (deltaDown * 1000) / elapsed,
      };
    }

    samples.value = nextSamples;
    rates.value = nextRates;
    byServer.value = owners;
  }

  function speedOf(id: string): { up: number; down: number } {
    return rates.value[id] ?? { up: 0, down: 0 };
  }

  /** 某条隧道下所有转发的合计速率。 */
  function tunnelSpeed(serverId: string): { up: number; down: number } {
    const ids = byServer.value[serverId] ?? [];
    return ids.reduce(
      (total, id) => {
        const rate = speedOf(id);
        return { up: total.up + rate.up, down: total.down + rate.down };
      },
      { up: 0, down: 0 },
    );
  }

  /** 所有隧道合计。 */
  const totalSpeed = computed(() => {
    return Object.values(rates.value).reduce(
      (total, rate) => ({ up: total.up + rate.up, down: total.down + rate.down }),
      { up: 0, down: 0 },
    );
  });

  return { sample, speedOf, tunnelSpeed, totalSpeed };
});

/**
 * 速率格式化：固定两位有效量级，避免「0.00 B/s」这种噪音。
 *
 * 全部为 0 时返回空串——界面上挂着一串 0 比不显示更吵。
 */
export function formatRate(bytesPerSecond: number): string {
  if (!Number.isFinite(bytesPerSecond) || bytesPerSecond < 1) {
    return '';
  }
  const units = ['B/s', 'KB/s', 'MB/s', 'GB/s'];
  let value = bytesPerSecond;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const digits = value >= 100 ? 0 : value >= 10 ? 1 : 2;
  return `${value.toFixed(digits)} ${units[unit]}`;
}

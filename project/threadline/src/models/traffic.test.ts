import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import type { TunnelView } from '../api/types';
import { useTrafficStore } from './traffic';

function view(id: string, serverId: string, up: number, down: number): TunnelView {
  return {
    definition: {
      id,
      name: id,
      server_id: serverId,
      local_host: '127.0.0.1',
      local_port: 18080,
      remote_host: '127.0.0.1',
      remote_port: 80,
      auto_start: false,
    },
    status: {
      id,
      state: 'running',
      active_connections: 1,
      bytes_up: up,
      bytes_down: down,
      last_error: null,
      retry_in_secs: null,
    },
  };
}

describe('traffic store', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2026-01-01T00:00:00Z'));
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('转发从列表里消失后，不再把它的旧速率算进合计', () => {
    // 用户报的现象：隧道全停了，侧栏仍挂着 ↑/↓ 速率。
    // 根因是旧实现用 {...rates} 承接上一轮，删掉的转发 key 永远留着。
    const store = useTrafficStore();

    store.sample([view('t1', 'srv', 0, 0)]);
    vi.advanceTimersByTime(1000);
    store.sample([view('t1', 'srv', 5000, 9000)]);
    expect(store.totalSpeed.down).toBeGreaterThan(0);

    // 转发被删/停：列表空了，合计必须立刻归零
    vi.advanceTimersByTime(1000);
    store.sample([]);
    expect(store.totalSpeed).toEqual({ up: 0, down: 0 });
  });

  it('两次采样间隔过短时沿用上一轮速率，不闪成 0', () => {
    const store = useTrafficStore();

    store.sample([view('t1', 'srv', 0, 0)]);
    vi.advanceTimersByTime(1000);
    store.sample([view('t1', 'srv', 1000, 2000)]);
    const first = store.totalSpeed.down;
    expect(first).toBe(2000);

    // 100ms 后就再采一次：增量算不准，但数字不该被抹掉
    vi.advanceTimersByTime(100);
    store.sample([view('t1', 'srv', 1100, 2200)]);
    expect(store.totalSpeed.down).toBe(first);
  });
});

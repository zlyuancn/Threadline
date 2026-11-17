import { createPinia, setActivePinia } from 'pinia';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { ApiFailure } from '../api/client';
import type { ContainerItem, ContainerList, ContainerStats } from '../api/types';

const api = vi.hoisted(() => ({
  containers: vi.fn(),
  containerStats: vi.fn(),
  containerDetail: vi.fn(),
  containerLogs: vi.fn(),
  containerAction: vi.fn(),
  containerRemove: vi.fn(),
}));

vi.mock('../api', () => ({ adminApi: api }));

const { useContainersStore } = await import('./containers');

function row(over: Partial<ContainerItem> = {}): ContainerItem {
  return {
    id: 'a1f93c2e7b10',
    name: 'immich_server',
    image: 'ghcr.io/immich-app/immich-server:v1.119.0',
    state: 'running',
    status: 'Up 3 days',
    created_at: '2026-11-03 21:04',
    restarts: 0,
    ports: [],
    mounts: [],
    networks: [],
    cpu_percent: 0,
    mem_bytes: 0,
    mem_limit_bytes: 0,
    net_rx_bytes: 0,
    net_tx_bytes: 0,
    ...over,
  };
}

function list(items: ContainerItem[]): ContainerList {
  return {
    engine: 'docker',
    total: items.length,
    running: items.filter((i) => i.state === 'running').length,
    exited: items.filter((i) => i.state === 'exited').length,
    paused: 0,
    restarting: 0,
    items,
    collected_at: '',
    elapsed_ms: 1,
    degraded: [],
  };
}

function stat(over: Partial<ContainerStats> = {}): ContainerStats {
  return {
    id: 'a1f93c2e7b10',
    cpu_percent: 2.5,
    mem_bytes: 1024 * 1024,
    mem_limit_bytes: 1024 * 1024 * 8,
    net_rx_bytes: 2048,
    net_tx_bytes: 1024,
    ...over,
  };
}

beforeEach(() => {
  setActivePinia(createPinia());
  for (const fn of Object.values(api)) fn.mockReset();
});

describe('列表与资源的合并', () => {
  it('first load pulls the stats pass together with the list', async () => {
    api.containers.mockResolvedValue(list([row(), row({ id: 'b2e04f1a9c33', name: 'redis' })]));
    api.containerStats.mockResolvedValue([stat()]);

    const store = useContainersStore();
    await store.load('s1');

    // 首屏就连带采一次：否则 CPU / MEM / NET 三列全是「—」，整块看着像坏了
    expect(store.hasStats('a1f93c2e7b10')).toBe(true);
    expect(store.hasStats('b2e04f1a9c33')).toBe(false);

    const immich = store.items.find((i) => i.name === 'immich_server');
    expect(immich?.cpu_percent).toBe(2.5);
    expect(immich?.mem_bytes).toBe(1024 * 1024);
  });

  it('does not re-run stats on every list poll', async () => {
    api.containers.mockResolvedValue(list([row()]));
    api.containerStats.mockResolvedValue([stat()]);

    const store = useContainersStore();
    await store.load('s1');
    expect(api.containerStats).toHaveBeenCalledTimes(1);

    // 列表 5 秒一拍，stats 要 2 秒：跟着每一拍跑会把会话占满，
    // 后续刷新由列表那边每 3 拍补一次。
    await store.load('s1');
    expect(api.containerStats).toHaveBeenCalledTimes(1);
  });

  it('falls back to a prefix match when the two passes disagree on id length', async () => {
    api.containers.mockResolvedValue(list([row({ id: 'a1f93c2e7b10' })]));
    api.containerStats.mockResolvedValue([stat({ id: 'a1f93c2e7b10ffff', cpu_percent: 9.9 })]);

    const store = useContainersStore();
    await store.load('s1');
    await store.loadStats();

    expect(store.hasStats('a1f93c2e7b10')).toBe(true);
    expect(store.items[0].cpu_percent).toBe(9.9);
  });

  it('keeps the list usable when the stats pass fails', async () => {
    api.containers.mockResolvedValue(list([row()]));
    api.containerStats.mockRejectedValue(new Error('stats 挂了'));

    const store = useContainersStore();
    await store.load('s1');
    await store.loadStats();

    expect(store.list?.total).toBe(1);
    expect(store.hasStats('a1f93c2e7b10')).toBe(false);
    // 资源失败是锦上添花的事，不该冒红条
    expect(store.feedback).toBe('');
  });
});

describe('日志跟随', () => {
  it('appends the incremental pass instead of replacing the tail', async () => {
    api.containers.mockResolvedValue(list([row()]));
    api.containerLogs
      .mockResolvedValueOnce({
        lines: ['2026-11-17T11:00:00.000000001Z a', '2026-11-17T11:00:00.000000002Z b'],
        truncated: false,
        next_since: '2026-11-17T11:00:00.000000002Z',
      })
      .mockResolvedValueOnce({
        // `docker logs --since` 是 `>=`：上一批的最后一行会被重复带回
        lines: ['2026-11-17T11:00:00.000000002Z b', '2026-11-17T11:00:00.000000003Z c'],
        truncated: false,
        next_since: '2026-11-17T11:00:00.000000003Z',
      });

    const store = useContainersStore();
    await store.load('s1');
    await store.loadLogs('immich_server', 200);
    expect(store.logs?.lines).toHaveLength(2);

    await store.loadLogs('immich_server', 200, store.logs?.next_since);

    // 不能覆盖（打开时那批还在），也不能重复（b 只出现一次）
    expect(store.logs?.lines).toEqual([
      '2026-11-17T11:00:00.000000001Z a',
      '2026-11-17T11:00:00.000000002Z b',
      '2026-11-17T11:00:00.000000003Z c',
    ]);
  });

  it('caps the buffer at `tail` while following', async () => {
    api.containers.mockResolvedValue(list([row()]));
    api.containerLogs.mockResolvedValueOnce({
      lines: ['2026-11-17T11:00:00.000000001Z a', '2026-11-17T11:00:00.000000002Z b'],
      truncated: true,
      next_since: '2026-11-17T11:00:00.000000002Z',
    });

    const store = useContainersStore();
    await store.load('s1');
    await store.loadLogs('immich_server', 2);

    api.containerLogs.mockResolvedValueOnce({
      lines: ['2026-11-17T11:00:00.000000002Z b', '2026-11-17T11:00:00.000000003Z c'],
      truncated: false,
      next_since: '2026-11-17T11:00:00.000000003Z',
    });
    await store.loadLogs('immich_server', 2, store.logs?.next_since);

    // 上限就是 tail：留着最后 2 行，且「已截断」的标记不会被后续批次抹掉
    expect(store.logs?.lines).toEqual([
      '2026-11-17T11:00:00.000000002Z b',
      '2026-11-17T11:00:00.000000003Z c',
    ]);
    expect(store.logs?.truncated).toBe(true);
  });
});

describe('筛选与搜索', () => {
  async function seed() {
    api.containers.mockResolvedValue(
      list([
        row(),
        row({ id: 'b2e04f1a9c33', name: 'redis', image: 'redis:7', state: 'exited' }),
        row({ id: 'c3d15a2b0e44', name: 'web', image: 'nginx:1.27', state: 'paused' }),
      ]),
    );
    const store = useContainersStore();
    await store.load('s1');
    return store;
  }

  it('splits running from stopped (paused counts as stopped)', async () => {
    const store = await seed();

    store.filter = 'running';
    expect(store.visible.map((i) => i.name)).toEqual(['immich_server']);

    store.filter = 'stopped';
    expect(store.visible.map((i) => i.name)).toEqual(['redis', 'web']);
  });

  it('searches name, image and id', async () => {
    const store = await seed();

    store.query = 'REDIS';
    expect(store.visible.map((i) => i.name)).toEqual(['redis']);

    store.query = 'nginx';
    expect(store.visible.map((i) => i.name)).toEqual(['web']);

    store.query = 'c3d15a';
    expect(store.visible.map((i) => i.name)).toEqual(['web']);
  });
});

describe('写操作', () => {
  it('updates the row in place so the UI does not look stuck', async () => {
    api.containers.mockResolvedValue(list([row({ state: 'running', status: 'Up 3 days' })]));
    api.containerAction.mockResolvedValue({
      ok: true,
      message: '已停止容器 immich_server',
      state: 'exited',
      image_removed: false,
    });

    const store = useContainersStore();
    await store.load('s1');

    const ok = await store.act('immich_server', 'stop');
    expect(ok).toBe(true);
    expect(api.containerAction).toHaveBeenCalledWith('s1', 'immich_server', 'stop');
    expect(store.feedback).toContain('已停止');
    expect(store.feedbackIsError).toBe(false);
  });

  it('surfaces the core message when an action is rejected', async () => {
    api.containers.mockResolvedValue(list([row()]));
    api.containerAction.mockRejectedValue(new ApiFailure(409, 'conflict', '容器当前状态不允许停止'));

    const store = useContainersStore();
    await store.load('s1');

    const ok = await store.act('immich_server', 'stop');
    expect(ok).toBe(false);
    expect(store.feedback).toBe('容器当前状态不允许停止');
    expect(store.feedbackIsError).toBe(true);
  });
});

describe('错误分档', () => {
  it('takes the screen on the first load, only a corner note afterwards', async () => {
    api.containers.mockRejectedValueOnce(new ApiFailure(500, 'boom', '采集失败'));

    const store = useContainersStore();
    await store.load('s1');
    expect(store.error).toBe('采集失败');
    expect(store.lightError).toBe('');

    // 已经有数据之后再失败：不能把已有列表换成整屏报错
    api.containers.mockResolvedValue(list([row()]));
    await store.load('s1');
    api.containers.mockRejectedValueOnce(new ApiFailure(500, 'boom', '又失败了'));
    await store.load('s1');

    expect(store.error).toBe('');
    expect(store.lightError).toBe('又失败了');
    expect(store.list?.total).toBe(1);
  });

  it('clears the board when switching to another server', async () => {
    api.containers.mockResolvedValue(list([row()]));
    const store = useContainersStore();
    await store.load('s1');
    expect(store.list).not.toBeNull();

    store.reset();
    expect(store.list).toBeNull();
    expect(store.serverId).toBe('');
  });
});

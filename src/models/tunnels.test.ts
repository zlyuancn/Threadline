import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';

// 目的不是测 HTTP，而是测"失败信息别被后续刷新抹掉"这条约束。
const createMock = vi.fn();
const listMock = vi.fn();

vi.mock('../api', () => ({
  tunnelsApi: {
    create: (...args: unknown[]) => createMock(...args),
    list: (...args: unknown[]) => listMock(...args),
    remove: vi.fn(),
    patch: vi.fn(),
    start: vi.fn(),
    stop: vi.fn(),
  },
}));

import { ApiFailure } from '../api/client';
import { useTunnelsStore } from './tunnels';

describe('tunnels store', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    createMock.mockReset();
    listMock.mockReset();
  });

  it('新建失败后，紧随其后的刷新不能把错误抹掉', async () => {
    // 这就是用户报的那个 bug：本地端口冲突 → create 抛错 → 紧接着 refresh() 成功，
    // 旧实现里 refresh 会清空 lastError，界面于是照样弹"端口已添加"。
    createMock.mockRejectedValue(
      new ApiFailure(409, 'invalid_reference', '本地端口 18080 已被转发「strata」占用，换一个端口'),
    );
    listMock.mockResolvedValue({ items: [] });

    const store = useTunnelsStore();
    await store.create({
      name: 'dup',
      server_id: 'srv',
      local_port: 18080,
      remote_port: 80,
    });

    expect(store.lastError).toContain('已被转发');
    expect(store.lastError).toContain('18080');
  });

  it('下一次动作开始时会清掉上一次的错误', async () => {
    createMock.mockRejectedValueOnce(new ApiFailure(409, 'invalid_reference', '端口冲突'));
    listMock.mockResolvedValue({ items: [] });

    const store = useTunnelsStore();
    await store.create({ name: 'a', server_id: 'srv', local_port: 1, remote_port: 1 });
    expect(store.lastError).toBe('端口冲突');

    // 第二次成功：错误应当消失，而不是一直挂着
    createMock.mockResolvedValueOnce({});
    await store.create({ name: 'b', server_id: 'srv', local_port: 2, remote_port: 2 });
    expect(store.lastError).toBe('');
  });
});

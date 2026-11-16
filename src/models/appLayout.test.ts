import { describe, expect, it } from 'vitest';
import { groupOf, groupOrder, insertAt, shiftItem, sortedIds, UNGROUPED } from './appLayout';
import type { AppGroup, TunnelView } from '../api/types';

function view(
  id: string,
  opts: { group?: string; app_order?: number; in_app?: boolean } = {},
): TunnelView {
  return {
    definition: {
      id,
      name: id,
      server_id: 'srv',
      local_host: '127.0.0.1',
      local_port: 1,
      remote_host: '127.0.0.1',
      remote_port: 1,
      auto_start: false,
      group: opts.group ?? '',
      in_app: opts.in_app ?? true,
      app_order: opts.app_order ?? 0,
    },
    status: {
      id,
      state: 'stopped',
      active_connections: 0,
      bytes_up: 0,
      bytes_down: 0,
      last_error: null,
      retry_in_secs: null,
    },
  } as unknown as TunnelView;
}

const groups = (...names: string[]): AppGroup[] => names.map((name, order) => ({ name, order }));

describe('groupOf', () => {
  it('空分组归到未分组', () => {
    expect(groupOf(view('a'))).toBe(UNGROUPED);
    expect(groupOf(view('a', { group: '   ' }))).toBe(UNGROUPED);
    expect(groupOf(view('a', { group: '工具' }))).toBe('工具');
  });
});

describe('sortedIds', () => {
  it('按 app_order 升序，没有 app_order 的保持原顺序', () => {
    const ids = sortedIds([view('a'), view('b', { app_order: 5 }), view('c', { app_order: 2 })]);
    expect(ids).toEqual(['a', 'c', 'b']);
  });

  it('同值保持原顺序（稳定）', () => {
    const ids = sortedIds([view('a'), view('b'), view('c')]);
    expect(ids).toEqual(['a', 'b', 'c']);
  });

  it('只看添加到应用的转发', () => {
    const ids = sortedIds([view('a'), view('b', { in_app: false }), view('c')]);
    expect(ids).toEqual(['a', 'c']);
  });
});

describe('insertAt', () => {
  const ids = ['a', 'b', 'c', 'd'];

  it('插到目标之前', () => {
    expect(insertAt(ids, 'd', 'b', false)).toEqual(['a', 'd', 'b', 'c']);
  });

  it('插到目标之后', () => {
    expect(insertAt(ids, 'a', 'b', true)).toEqual(['b', 'a', 'c', 'd']);
  });

  it('插到列表头部', () => {
    expect(insertAt(ids, 'c', 'a', false)).toEqual(['c', 'a', 'b', 'd']);
  });

  it('目标为空表示移到末尾', () => {
    expect(insertAt(ids, 'a', null, false)).toEqual(['b', 'c', 'd', 'a']);
  });

  it('目标不存在时也放到末尾（不丢条目）', () => {
    expect(insertAt(ids, 'a', 'zzz', false)).toEqual(['b', 'c', 'd', 'a']);
  });

  it('拖到自己身上时不会算两遍', () => {
    // 位置在"去掉自己"的序列里算，anchor 找不到就落到末尾
    const result = insertAt(ids, 'b', 'b', true);
    expect(result).toEqual(['a', 'c', 'd', 'b']);
    expect(result).toHaveLength(ids.length);
  });

  it('前后元素数量不变', () => {
    expect(insertAt(ids, 'c', 'a', false)).toHaveLength(ids.length);
  });
});

describe('shiftItem', () => {
  const ids = ['a', 'b', 'c', 'd'];

  it('上移一步', () => {
    expect(shiftItem(ids, 'c', -1)).toEqual(['a', 'c', 'b', 'd']);
  });

  it('下移一步', () => {
    expect(shiftItem(ids, 'b', 1)).toEqual(['a', 'c', 'b', 'd']);
  });

  it('移到最前 / 最后', () => {
    expect(shiftItem(ids, 'd', 'first')).toEqual(['d', 'a', 'b', 'c']);
    expect(shiftItem(ids, 'a', 'last')).toEqual(['b', 'c', 'd', 'a']);
  });

  it('已在边界时返回同一个数组（调用方据此跳过写入）', () => {
    expect(shiftItem(ids, 'a', -1)).toBe(ids);
    expect(shiftItem(ids, 'a', 'first')).toBe(ids);
    expect(shiftItem(ids, 'd', 1)).toBe(ids);
    expect(shiftItem(ids, 'd', 'last')).toBe(ids);
  });

  it('不存在时原样返回', () => {
    expect(shiftItem(ids, 'zzz', 1)).toBe(ids);
  });
});

describe('groupOrder', () => {
  const declared = groups('工具', 'ai工具');

  it('按清单顺序分组，未分组最后', () => {
    const rows = [
      view('u1', {}),
      view('b1', { group: 'ai工具' }),
      view('a1', { group: '工具' }),
    ];
    const result = groupOrder(rows, groupOf, declared, true);
    expect(result.map((b) => b.group)).toEqual(['工具', 'ai工具', UNGROUPED]);
  });

  it('保留空组（编辑模式需要能拖进去）', () => {
    const result = groupOrder([view('a1', { group: '工具' })], groupOf, groups('工具', '空闲'), true);
    const empty = result.find((b) => b.group === '空闲');
    expect(empty?.items).toEqual([]);
  });

  it('非编辑模式丢掉空组', () => {
    const result = groupOrder([view('a1', { group: '工具' })], groupOf, groups('工具', '空闲'), false);
    expect(result.map((b) => b.group)).toEqual(['工具']);
  });

  it('清单里没有但成员用到的组追加到末尾', () => {
    const rows = [view('a1', { group: '工具' }), view('x1', { group: '手工加的' })];
    const result = groupOrder(rows, groupOf, declared, true);
    expect(result.map((b) => b.group)).toEqual(['工具', 'ai工具', '手工加的']);
  });

  it('没有成员时不显示未分组', () => {
    const result = groupOrder([view('a1', { group: '工具' })], groupOf, declared, false);
    expect(result.some((b) => b.group === UNGROUPED)).toBe(false);
  });
});

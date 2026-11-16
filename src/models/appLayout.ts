// 应用页布局：分组清单（顺序 + 空组）与磁贴顺序。
//
// 边界：这里只算"怎么摆"，一律不碰转发本身。写入全交给核心
// （`appGroupsApi` / `tunnelsApi.reorderApp`），写完立刻回读——
// 布局错一次比慢一秒更难受，所以不做乐观更新。

import { defineStore } from 'pinia';
import { ref } from 'vue';
import { ApiFailure } from '../api/client';
import { appGroupsApi, tunnelsApi } from '../api';
import type { AppGroup, TunnelView } from '../api/types';

/** 隐含分组：转发 `group` 为空时归到这里，不可改名/删除。 */
export const UNGROUPED = '未分组';

export interface GroupBucket<T> {
  group: string;
  items: T[];
}

/** 一条转发所属的组（空 → 未分组）。 */
export function groupOf(view: TunnelView): string {
  return view.definition.group?.trim() || UNGROUPED;
}

/** 应用页可见磁贴的当前顺序：`app_order` 升序，同值保持配置顺序（稳定）。 */
export function sortedIds(views: TunnelView[]): string[] {
  return views
    .filter((view) => view.definition.in_app !== false)
    .map((view, index) => ({ view, index }))
    .sort(
      (a, b) =>
        (a.view.definition.app_order ?? 0) - (b.view.definition.app_order ?? 0) || a.index - b.index,
    )
    .map((item) => item.view.definition.id);
}

/**
 * 把 `moving` 插到 `anchorId` 的前面或后面；`anchorId` 为空 = 追加到末尾。
 *
 * 位置在**去掉自己之后**的序列里算——否则"拖到自己的右半边"会把自己算两遍。
 * `anchorId` 不在列表里（已删除/拖到了别处）时按末尾处理：宁可位置不准，
 * 也不要把它从布局里弄丢。
 */
export function insertAt(
  ids: string[],
  moving: string,
  anchorId: string | null,
  after: boolean,
): string[] {
  const rest = ids.filter((id) => id !== moving);
  const index = anchorId ? rest.indexOf(anchorId) : -1;
  const at = index < 0 ? rest.length : after ? index + 1 : index;
  return [...rest.slice(0, at), moving, ...rest.slice(at)];
}

/**
 * 按按钮挪动一项：`delta` 为 ±1 时上/下移一步，`'first'`/`'last'` 直接到位。
 *
 * 越界或本来就在目标位置时**原样返回同一个数组**（引用相同），调用方据此跳过写入——
 * 组已经在最前时点「移到最前」不该发一次没意义的请求。
 */
export function shiftItem(
  ids: string[],
  item: string,
  delta: number | 'first' | 'last',
): string[] {
  const index = ids.indexOf(item);
  if (index < 0) {
    return ids;
  }
  const target =
    delta === 'first' ? 0 : delta === 'last' ? ids.length - 1 : index + delta;
  if (target < 0 || target >= ids.length || target === index) {
    return ids;
  }
  const next = [...ids];
  next.splice(index, 1);
  next.splice(target, 0, item);
  return next;
}

/**
 * 按分组清单排序并归类。
 *
 * - 组顺序以核心给的清单为准（数组顺序即顺序）；清单里没有、但成员用到的组追加到末尾
 *   （手改配置后界面也能显示，不吞组）。
 * - `keepEmpty` 为真时保留空组（编辑模式下得能看见/拖进去），否则只留有空成员的组。
 * - 「未分组」永远最后。
 */
export function groupOrder<T>(
  items: T[],
  groupOfItem: (item: T) => string,
  declared: AppGroup[],
  keepEmpty = false,
): GroupBucket<T>[] {
  const buckets = new Map<string, T[]>();
  for (const item of items) {
    const name = groupOfItem(item);
    if (!buckets.has(name)) {
      buckets.set(name, []);
    }
    buckets.get(name)!.push(item);
  }

  const names: string[] = [];
  const push = (name: string) => {
    if (name && name !== UNGROUPED && !names.includes(name)) {
      names.push(name);
    }
  };
  for (const group of [...declared].sort((a, b) => a.order - b.order)) {
    push(group.name.trim());
  }
  for (const name of buckets.keys()) {
    push(name);
  }

  const result: GroupBucket<T>[] = names.map((name) => ({
    group: name,
    items: buckets.get(name) ?? [],
  }));
  const visible = keepEmpty ? result : result.filter((bucket) => bucket.items.length > 0);

  const ungrouped = buckets.get(UNGROUPED);
  if (ungrouped?.length) {
    visible.push({ group: UNGROUPED, items: ungrouped });
  }
  return visible;
}

export const useAppLayoutStore = defineStore('appLayout', () => {
  const groups = ref<AppGroup[]>([]);
  const loading = ref(false);
  const lastError = ref('');

  function failure(e: unknown): string {
    return e instanceof ApiFailure ? e.message : String(e);
  }

  async function refresh(): Promise<void> {
    loading.value = true;
    try {
      groups.value = (await appGroupsApi.list()).items;
      lastError.value = '';
    } catch (e) {
      lastError.value = failure(e);
    } finally {
      loading.value = false;
    }
  }

  /** 全量写回（新建空组、调整组顺序）。 */
  async function setAll(names: string[]): Promise<void> {
    groups.value = (await appGroupsApi.setAll(names.map((name, index) => ({ name, order: index }))))
      .items;
    lastError.value = '';
  }

  /** 新建一个空组（加到末尾）；已存在则直接返回。 */
  async function create(name: string): Promise<void> {
    const target = name.trim();
    if (!target || groups.value.some((g) => g.name === target)) {
      return;
    }
    await setAll([...groups.value.map((g) => g.name), target]);
  }

  async function rename(from: string, to: string): Promise<void> {
    groups.value = (await appGroupsApi.rename(from, to)).items;
    lastError.value = '';
  }

  async function remove(name: string): Promise<void> {
    groups.value = (await appGroupsApi.remove(name)).items;
    lastError.value = '';
  }

  /** 磁贴顺序：只写 `app_order`。 */
  async function reorder(ids: string[]): Promise<void> {
    await tunnelsApi.reorderApp(ids);
    lastError.value = '';
  }

  function reset(): void {
    groups.value = [];
    lastError.value = '';
  }

  return { groups, loading, lastError, refresh, setAll, create, rename, remove, reorder, reset };
});

import { beforeEach, describe, expect, it } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { useSelectionStore } from './selection';

describe('selection store', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  it('记录选中隧道', () => {
    const store = useSelectionStore();
    store.select('srv_1');
    expect(store.selectedId).toBe('srv_1');
    store.select(null);
    expect(store.selectedId).toBeNull();
  });

  it('新建请求是"待办"，消费者晚挂载也能补上', () => {
    // 这条就是那个 bug：从「所有转发」页点「新建连接」时，主区还没挂载，
    // 旧的 watch 收不到自增信号，界面只会跳到默认隧道。
    const store = useSelectionStore();
    expect(store.hasPendingCreate()).toBe(false);

    store.requestCreate();
    expect(store.hasPendingCreate()).toBe(true);

    store.markCreateHandled();
    expect(store.hasPendingCreate()).toBe(false);
  });

  it('多次请求只处理一半时仍然有待办', () => {
    const store = useSelectionStore();
    store.requestCreate();
    store.requestCreate();
    store.markCreateHandled();

    // 连着点两次、只开了一次窗：还有一次没兑现，不能算处理完
    expect(store.hasPendingCreate()).toBe(false);
    store.requestCreate();
    expect(store.hasPendingCreate()).toBe(true);
  });
});

import { defineStore } from 'pinia';
import { ref } from 'vue';

/**
 * 侧栏与主区共享的"当前选中隧道"，以及"要开新建弹窗"的待办信号。
 *
 * 隧道列表在侧栏（App.vue），详情在主区（HomeView.vue）。两个组件各自持有一份
 * 选中状态必然会不同步，所以放一份在 store 里。
 */
export const useSelectionStore = defineStore('selection', () => {
  const selectedId = ref<string | null>(null);

  /**
   * 「新建连接」按钮在侧栏，弹窗在主区。
   *
   * 这里记的是**待办**而不是"事件"：自增信号只在 HomeView 已经挂载时才收得到，
   * 而从「所有转发」页点按钮时主区还没挂载——信号就丢了，界面只会跳到某条隧道。
   * 所以改成"请求数 vs 已处理数"，主区挂载时先补一次。
   */
  const createRequests = ref(0);
  const createHandled = ref(0);

  function select(id: string | null) {
    selectedId.value = id;
  }

  function requestCreate() {
    createRequests.value += 1;
  }

  /** 有没有还没被处理的"开新建弹窗"请求。 */
  function hasPendingCreate(): boolean {
    return createRequests.value > createHandled.value;
  }

  function markCreateHandled() {
    createHandled.value = createRequests.value;
  }

  return {
    selectedId,
    createRequests,
    select,
    requestCreate,
    hasPendingCreate,
    markCreateHandled,
  };
});

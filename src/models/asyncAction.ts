// 按钮的"进行中"语义：加载态 + 防重复点击 + 超时。
//
// 为什么需要：这些动作会跨进程/跨网络（起核心要等好几秒、停服务要走 HTTP），
// 没有这层的话用户会连点，重复请求会互相打架（第二次 shutdown 会打到已经退出的核心）。

import { ref, type Ref } from 'vue';

/** 给任意 Promise 套一个上界，超时即以指定文案失败。 */
export async function withTimeout<T>(
  work: Promise<T>,
  ms: number,
  message: string,
): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      work,
      new Promise<never>((_resolve, reject) => {
        timer = setTimeout(() => reject(new Error(message)), ms);
      }),
    ]);
  } finally {
    if (timer !== undefined) {
      clearTimeout(timer);
    }
  }
}

export interface AsyncActionOptions {
  /** 上界毫秒数，默认 10 秒。 */
  timeoutMs?: number;
  timeoutMessage?: string;
}

export interface AsyncAction {
  /** 进行中：按钮据此转圈并禁止点击。 */
  pending: Ref<boolean>;
  /** 最近一次失败的原因（成功时清空）。 */
  error: Ref<string | null>;
  /** 执行；返回是否成功。**进行中再次调用会被直接忽略**，返回 false。 */
  run: () => Promise<boolean>;
}

export function useAsyncAction(
  work: () => Promise<void>,
  options: AsyncActionOptions = {},
): AsyncAction {
  const {
    timeoutMs = 10_000,
    timeoutMessage = '操作超时，请稍后重试',
  } = options;

  const pending = ref(false);
  const error = ref<string | null>(null);

  async function run(): Promise<boolean> {
    if (pending.value) {
      return false;
    }
    pending.value = true;
    error.value = null;
    try {
      await withTimeout(work(), timeoutMs, timeoutMessage);
      return true;
    } catch (e) {
      error.value = e instanceof Error ? e.message : String(e);
      return false;
    } finally {
      pending.value = false;
    }
  }

  return { pending, error, run };
}

import { describe, expect, it } from 'vitest';
import { useAsyncAction, withTimeout } from './asyncAction';

describe('withTimeout', () => {
  it('returns the value when the work finishes in time', async () => {
    await expect(withTimeout(Promise.resolve(7), 1000, '超时')).resolves.toBe(7);
  });

  it('rejects with the given message when the work hangs', async () => {
    const never = new Promise<number>(() => {});
    await expect(withTimeout(never, 50, '操作超时')).rejects.toThrow('操作超时');
  });

  it('propagates the original error instead of the timeout', async () => {
    const boom = Promise.reject(new Error('原始错误'));
    await expect(withTimeout(boom, 1000, '超时')).rejects.toThrow('原始错误');
  });
});

describe('useAsyncAction', () => {
  it('ignores repeated clicks while the first one is still pending', async () => {
    let calls = 0;
    let release = () => {};
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });

    const action = useAsyncAction(async () => {
      calls += 1;
      await gate;
    });

    const first = action.run();
    expect(action.pending.value).toBe(true);

    const second = await action.run();
    expect(second).toBe(false);
    expect(calls).toBe(1);

    release();
    await expect(first).resolves.toBe(true);
    expect(action.pending.value).toBe(false);
  });

  it('resets pending after a failure so the button becomes clickable again', async () => {
    const action = useAsyncAction(async () => {
      throw new Error('核心拒绝');
    });

    await expect(action.run()).resolves.toBe(false);
    expect(action.pending.value).toBe(false);
    expect(action.error.value).toBe('核心拒绝');

    // 还能再点
    await expect(action.run()).resolves.toBe(false);
  });

  it('fails with the timeout message when the work never finishes', async () => {
    const action = useAsyncAction(async () => new Promise<void>(() => {}), {
      timeoutMs: 50,
      timeoutMessage: '启动超时',
    });

    await expect(action.run()).resolves.toBe(false);
    expect(action.error.value).toBe('启动超时');
    expect(action.pending.value).toBe(false);
  });

  it('clears the previous error on a new attempt', async () => {
    let attempt = 0;
    const action = useAsyncAction(async () => {
      attempt += 1;
      if (attempt === 1) throw new Error('第一次失败');
    });

    await expect(action.run()).resolves.toBe(false);
    expect(action.error.value).toBe('第一次失败');

    await expect(action.run()).resolves.toBe(true);
    expect(action.error.value).toBeNull();
  });
});

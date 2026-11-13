import { describe, expect, it } from 'vitest';
import { mergeSessionStates } from './sessionState';

describe('mergeSessionStates', () => {
  it('查到了就采用新值', () => {
    const next = mergeSessionStates({}, ['a'], [{ connected: true }]);
    expect(next.a).toEqual({ connected: true, known: true });
  });

  it('查询失败时保留上一次的值，而不是写成未连接', () => {
    // 这是用户遇到的场景：核心重启后 token 轮换，会话查询失败，
    // 界面却显示"未连接"——因为失败被当成了 false。
    const previous = { a: { connected: true, known: true } };
    const next = mergeSessionStates(previous, ['a'], [null]);
    expect(next.a).toEqual({ connected: true, known: true });
  });

  it('第一次就查不到时标记为未知，而不是未连接', () => {
    const next = mergeSessionStates({}, ['a'], [null]);
    expect(next.a).toEqual({ connected: false, known: false });
  });

  it('多台隧道互不影响', () => {
    const previous = { b: { connected: true, known: true } };
    const next = mergeSessionStates(previous, ['a', 'b'], [{ connected: false }, null]);
    expect(next.a).toEqual({ connected: false, known: true });
    expect(next.b).toEqual({ connected: true, known: true });
  });
});

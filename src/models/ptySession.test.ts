import { describe, expect, it } from 'vitest';
import { parseServerFrame, reducePtyState } from './ptySession';

describe('parseServerFrame', () => {
  it('reads the hello frame and keeps the command', () => {
    expect(parseServerFrame('{"type":"hello","command":"bash -l","mode":"shell"}')).toEqual({
      type: 'hello',
      command: 'bash -l',
    });
  });

  it('defaults a missing command instead of failing', () => {
    expect(parseServerFrame('{"type":"hello"}')).toEqual({ type: 'hello', command: '' });
  });

  it('reads the exit code', () => {
    expect(parseServerFrame('{"type":"exit","code":127}')).toEqual({ type: 'exit', code: 127 });
    expect(parseServerFrame('{"type":"exit"}')).toEqual({ type: 'exit', code: 0 });
  });

  it('keeps the error message, with a fallback', () => {
    expect(parseServerFrame('{"type":"error","message":"隧道不存在：s1"}')).toEqual({
      type: 'error',
      message: '隧道不存在：s1',
    });
    expect(parseServerFrame('{"type":"error"}')).toEqual({
      type: 'error',
      message: '终端打开失败',
    });
  });

  // 非 JSON 是普通输出（有些程序会直接把 JSON 样子的文本打到屏幕上），不能吞掉
  it('returns null for plain output and unknown frames', () => {
    expect(parseServerFrame('total 12')).toBeNull();
    expect(parseServerFrame('{')).toBeNull();
    expect(parseServerFrame('null')).toBeNull();
    expect(parseServerFrame('{"type":"mystery"}')).toBeNull();
  });
});

describe('reducePtyState', () => {
  it('walks the happy path', () => {
    let state = reducePtyState('idle', 'connect');
    expect(state).toBe('connecting');

    state = reducePtyState(state, 'hello');
    expect(state).toBe('open');

    expect(reducePtyState(state, 'exit')).toBe('closed');
    expect(reducePtyState('open', 'close')).toBe('closed');
  });

  it('keeps the failure visible when the socket closes right after it', () => {
    const failed = reducePtyState('connecting', 'error');
    expect(failed).toBe('error');
    // 握手失败后浏览器一定会再触发 close：不能把原因改写成"已结束"
    expect(reducePtyState(failed, 'close')).toBe('error');
  });

  it('ignores stray events before connecting', () => {
    expect(reducePtyState('idle', 'close')).toBe('idle');
    expect(reducePtyState('idle', 'exit')).toBe('idle');
    expect(reducePtyState('idle', 'hello')).toBe('idle');
  });

  it('reconnects from a closed or failed session', () => {
    expect(reducePtyState('closed', 'connect')).toBe('connecting');
    expect(reducePtyState('error', 'connect')).toBe('connecting');
  });
});

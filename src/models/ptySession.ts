// 终端纯件：帧解析、状态归约与主题常量。
//
// 这里**不 import xterm**：`@xterm/addon-fit` 在 node 里加载就会抛
// `self is not defined`，而前端测试环境正是 node（无 jsdom）。xterm 实例的
// 创建放在 `ptyTerminal.ts`，只有浏览器/组件会碰它。
//
// 连接生命周期（connect / teardown、什么时候重连、不可见时要不要 fit）也留在
// 各自的面板里——容器控制台是抽屉（`v-if`，永远可见），服务器终端是模块
// （`v-show`，会被隐藏），两者对"不可见时怎么处理尺寸"的要求正好相反。

// `import type` 编译期就被擦除，不会把 xterm 带进 node 测试。
import type { Terminal } from '@xterm/xterm';

/** 与界面同一套深色：终端和卡片不该是两个色温。 */
export const PTY_THEME = {
  background: '#0a171a',
  foreground: '#c6d4d1',
  cursor: '#6ee7b7',
  cursorAccent: '#0a171a',
  selectionBackground: 'rgba(110, 231, 183, 0.24)',
  black: '#0a171a',
  red: '#f87171',
  green: '#6ee7b7',
  yellow: '#fbbf24',
  blue: '#60a5fa',
  magenta: '#c084fc',
  cyan: '#22d3ee',
  white: '#c6d4d1',
  brightBlack: '#4b6360',
  brightRed: '#fca5a5',
  brightGreen: '#a7f3d0',
  brightYellow: '#fcd34d',
  brightBlue: '#93c5fd',
  brightMagenta: '#d8b4fe',
  brightCyan: '#67e8f9',
  brightWhite: '#e8f1ef',
} as const;

export type PtyStatus = 'idle' | 'connecting' | 'open' | 'closed' | 'error';

export type PtyEvent = 'connect' | 'hello' | 'exit' | 'error' | 'close';

/** 服务端 → 客户端的文本帧。二进制帧是 PTY 原始输出，不经过这里。 */
export type ServerFrame =
  | { type: 'hello'; command: string }
  | { type: 'exit'; code: number }
  | { type: 'error'; message: string };

/**
 * 解析服务端文本帧。不是 JSON、或不认识的 type 都返回 null——
 * 调用方把 null 当普通输出写进终端，不要因此中断会话。
 */
export function parseServerFrame(raw: string): ServerFrame | null {
  let frame: unknown;
  try {
    frame = JSON.parse(raw);
  } catch {
    return null;
  }
  if (!frame || typeof frame !== 'object') {
    return null;
  }
  const value = frame as { type?: unknown; command?: unknown; code?: unknown; message?: unknown };
  if (value.type === 'hello') {
    return { type: 'hello', command: typeof value.command === 'string' ? value.command : '' };
  }
  if (value.type === 'exit') {
    return { type: 'exit', code: typeof value.code === 'number' ? value.code : 0 };
  }
  if (value.type === 'error') {
    return {
      type: 'error',
      message: typeof value.message === 'string' ? value.message : '终端打开失败',
    };
  }
  return null;
}

/**
 * 状态归约。规则只有两条值得记住：
 *
 * 1. `error` 优先：握手失败后浏览器紧接着会触发 close，若 close 把状态改成
 *    `closed`，用户就看不到"为什么打不开"，只剩一句"会话已结束"。
 * 2. `idle` 收到 `exit` / `close` 不变：还没开始连接时的杂散事件不该让状态乱跳。
 */
export function reducePtyState(state: PtyStatus, event: PtyEvent): PtyStatus {
  switch (event) {
    case 'connect':
      return 'connecting';
    case 'hello':
      return state === 'connecting' ? 'open' : state;
    case 'exit':
      return state === 'idle' ? state : 'closed';
    case 'error':
      return 'error';
    case 'close':
      return state === 'error' || state === 'idle' ? state : 'closed';
  }
}

/** xterm 实例与 fit。定义在 `ptyTerminal.ts`（会加载 xterm），这里只导出类型。 */
export interface TerminalHandle {
  term: Terminal;
  fit: () => void;
  dispose: () => void;
}

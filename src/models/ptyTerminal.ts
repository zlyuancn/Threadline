// xterm 实例创建：**只有这里 import xterm**。
//
// 为什么和 `ptySession.ts` 分开：`@xterm/addon-fit` 在 node 里加载就抛
// `self is not defined`，而前端测试跑在 node 环境。纯逻辑（帧解析、状态归约）
// 留在 `ptySession.ts` 才能被 vitest 覆盖；这个文件只由组件引用。

import { FitAddon } from '@xterm/addon-fit';
import { Terminal } from '@xterm/xterm';
import { PTY_THEME, type TerminalHandle } from './ptySession';

/** 建一个 xterm 实例并挂到 host 上。只在浏览器里调用。 */
export function createTerminal(
  host: HTMLElement,
  options: { fontSize?: number } = {},
): TerminalHandle {
  const term = new Terminal({
    cursorBlink: true,
    // 远端已经在输出里带 \r\n，这里再转换会平白多出一堆空行
    convertEol: false,
    scrollback: 2000,
    fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Consolas, "Courier New", monospace',
    fontSize: options.fontSize ?? 11.5,
    lineHeight: 1.3,
    theme: PTY_THEME,
  });
  const fitAddon = new FitAddon();
  term.loadAddon(fitAddon);
  term.open(host);

  return {
    term,
    /** 尺寸还没算出来时（抽屉过渡、隐藏中）fit 会抛，忽略即可。 */
    fit(): void {
      try {
        fitAddon.fit();
      } catch {
        /* 尺寸不可用 */
      }
    },
    dispose(): void {
      term.dispose();
    },
  };
}

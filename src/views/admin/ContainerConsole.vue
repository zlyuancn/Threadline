<script setup lang="ts">
// 容器控制台：xterm.js + 一条 WebSocket 直连远端 PTY。
//
// 帧协议在核心侧定稿（`interface/mod.rs` 的 ServerFrame / ClientFrame）：
//   服务端 → hello（JSON）/ 二进制原始字节 / exit / error
//   客户端 → input / resize / close
// 输出走二进制而不是 base64：多字节 UTF-8 会被切在包边界上，二进制帧配
// xterm 的 write(Uint8Array) 才不会乱码。

import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import '@xterm/xterm/css/xterm.css';
import { adminApi } from '../../api';
import type { ConsoleMode } from '../../api';

const props = defineProps<{ serverId: string; containerName: string; state: string }>();

const mode = ref<ConsoleMode>('exec');
/** 服务端回显的实际命令：「现在到底用的是哪一档」不该靠猜。 */
const command = ref('');
const status = ref<'idle' | 'connecting' | 'open' | 'closed' | 'error'>('idle');
const errorText = ref('');

const host = ref<HTMLDivElement | null>(null);

let term: Terminal | null = null;
let socket: WebSocket | null = null;
let observer: ResizeObserver | null = null;
/** 重连序号：`connect` 里有 await，防止旧的一轮把新的顶掉。 */
let seq = 0;

const MODE_NOTE: Record<ConsoleMode, string> = {
  exec: '默认用户 · bash 优先，没有就用 sh',
  root: 'root 用户 · 能装包改文件，权限更高',
  debug: '调试图 · 镜像里没 shell 时用，首次会拉 busybox',
};

const modeNote = computed(() => MODE_NOTE[mode.value]);
const canConnect = computed(() => props.state === 'running');

/** 与界面同一套深色：截图里终端和卡片不该是两个色温。 */
const THEME = {
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
};

function send(payload: unknown): void {
  const ws = socket;
  if (!ws || ws.readyState !== WebSocket.OPEN) return;
  ws.send(JSON.stringify(payload));
}

/** 把 PTY 输出塞进终端；对端关了就写一行提示，而不是留一个死屏。 */
function writeNotice(text: string): void {
  term?.write(`\r\n\u001b[2m${text}\u001b[0m\r\n`);
}

function teardown(): void {
  observer?.disconnect();
  observer = null;

  const ws = socket;
  socket = null;
  if (ws) {
    try {
      if (ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify({ type: 'close' }));
      ws.close();
    } catch {
      /* 已经断了 */
    }
  }

  term?.dispose();
  term = null;
}

async function connect(): Promise<void> {
  if (!canConnect.value) return;

  const ticket = ++seq;
  teardown();
  status.value = 'connecting';
  errorText.value = '';
  command.value = '';

  // host 是 v-if 控制的：不等到下一帧，容器还没尺寸，fit 会算出 0 列。
  await nextTick();
  if (ticket !== seq || !host.value) return;

  const instance = new Terminal({
    cursorBlink: true,
    // 远端已经在输出里带 \r\n，这里再转换会平白多出一堆空行
    convertEol: false,
    scrollback: 2000,
    fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Consolas, "Courier New", monospace',
    fontSize: 11.5,
    lineHeight: 1.3,
    theme: THEME,
  });
  const fitAddon = new FitAddon();
  instance.loadAddon(fitAddon);
  instance.open(host.value);

  term = instance;
  // 抽屉刚打开时宽度可能在过渡中，先 fit 一次拿个接近真实的尺寸
  try {
    fitAddon.fit();
  } catch {
    /* 尺寸还没算出来，用下面的兜底值 */
  }

  const cols = Math.max(20, instance.cols || 100);
  const rows = Math.max(5, instance.rows || 30);

  const ws = adminApi.openConsole(props.serverId, props.containerName, mode.value, cols, rows);
  ws.binaryType = 'arraybuffer';
  socket = ws;

  // 逐键发送（不是整行）：vim / htop / tab 补全都靠这个
  instance.onData((data) => send({ type: 'input', data }));
  // 终端尺寸变了要通知远端，否则全屏程序按旧列数排版
  instance.onResize((size) => send({ type: 'resize', cols: size.cols, rows: size.rows }));

  ws.onmessage = (event: MessageEvent) => {
    if (typeof event.data === 'string') {
      try {
        const frame = JSON.parse(event.data) as {
          type: string;
          command?: string;
          code?: number;
          message?: string;
        };
        if (frame.type === 'hello') {
          command.value = frame.command ?? '';
          status.value = 'open';
          instance.focus();
          return;
        }
        if (frame.type === 'exit') {
          status.value = 'closed';
          writeNotice(`session closed · exit ${frame.code ?? 0}`);
          return;
        }
        if (frame.type === 'error') {
          errorText.value = frame.message ?? '控制台打开失败';
          status.value = 'error';
          return;
        }
      } catch {
        /* 不是 JSON 就当普通输出 */
      }
      instance.write(event.data);
      return;
    }

    if (event.data instanceof ArrayBuffer) {
      instance.write(new Uint8Array(event.data));
    }
  };

  ws.onerror = () => {
    if (status.value !== 'error') {
      // 核心主动拒绝（会话被占、容器状态变了）会走 `error` 帧，带具体原因；
      // 能落到 onerror 就只剩「握手没成」一种可能，别再把两个原因混在一句话里。
      errorText.value = '连不上核心：应用可能刚重启，等两秒点「重开」；也可能是本地端口被别的程序占了';
      status.value = 'error';
    }
  };

  ws.onclose = () => {
    if (status.value === 'open' || status.value === 'connecting') status.value = 'closed';
  };

  observer = new ResizeObserver(() => {
    try {
      fitAddon.fit();
    } catch {
      /* 抽屉正在收起，忽略 */
    }
  });
  observer.observe(host.value);
}

function reopen(): void {
  void connect();
}

function switchMode(next: ConsoleMode): void {
  if (mode.value === next) return;
  mode.value = next;
  // 换档就是换命令：旧会话必须先关，否则远端会留一个没人管的 shell
  void connect();
}

function focus(): void {
  term?.focus();
}

watch(
  () => [props.containerName, props.state],
  () => {
    ++seq;
    teardown();
    status.value = 'idle';
    if (props.state === 'running') void connect();
  },
  { immediate: true },
);

onBeforeUnmount(() => {
  ++seq;
  teardown();
});

defineExpose({ reopen });
</script>

<template>
  <div class="cc">
    <div class="cc-bar">
      <div class="seg">
        <button :class="{ 'is-on': mode === 'exec' }" @click="switchMode('exec')">exec</button>
        <button :class="{ 'is-on': mode === 'root' }" @click="switchMode('root')">root</button>
        <button :class="{ 'is-on': mode === 'debug' }" @click="switchMode('debug')">调试图</button>
      </div>
      <span class="cc-note">{{ modeNote }}</span>
      <button class="btn is-xs" @click="reopen">重开</button>
    </div>

    <div v-if="!canConnect" class="cc-notice">
      容器现在没有在运行。先「启动」，等状态变成运行中再回到这一页。
    </div>

    <template v-else>
      <div ref="host" class="cc-term" @click="focus" />
      <div v-if="status === 'error'" class="cc-error">{{ errorText }}</div>
      <div v-else-if="status === 'closed'" class="cc-error">
        会话已结束。点「重开」重新进入。
      </div>

      <p class="hint">
        交互式终端（xterm）：支持全屏程序、Tab 补全与 Ctrl 组合键。会话不落盘，
        关闭抽屉即断开，远端的 <code>docker exec</code> 会一起退出。
        <span v-if="command" class="mono">当前命令：{{ command }}</span>
      </p>
    </template>
  </div>
</template>

<style scoped>
.cc {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.cc-bar {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}
.cc-note {
  font-size: 10px;
  color: var(--text-3);
  flex: 1;
  min-width: 120px;
}

.cc-notice {
  padding: 14px 12px;
  border: 1px solid var(--line);
  border-radius: 8px;
  font-size: 11.5px;
  color: var(--text-3);
}

/* xterm 自己管内部布局，外层只负责给一块有尺寸的区域 */
.cc-term {
  height: 380px;
  padding: 8px;
  border: 1px solid var(--line);
  border-radius: 8px;
  background: #0a171a;
  overflow: hidden;
}
.cc-term :deep(.xterm) {
  height: 100%;
}
.cc-term :deep(.xterm-viewport) {
  background: transparent !important;
}

.cc-error {
  padding: 7px 10px;
  border-radius: 8px;
  background: rgba(248, 113, 113, 0.12);
  color: var(--bad-text);
  font-size: 11px;
}
</style>

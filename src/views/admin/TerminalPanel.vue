<script setup lang="ts">
// 服务器终端：xterm.js + 一条 WebSocket 直连远端登录 shell。
//
// 与容器控制台（`ContainerConsole.vue`）的区别只有两点：
//   1. 这里是模块，父级用 `v-show` 保挂载，会被隐藏。隐藏时 `fit()` 会算出
//      2×1 并推给远端，所以观察器必须有可见性守卫，回到可见时补一次 fit + refresh；
//   2. 命令固定（登录 shell），没有档位切换。
//
// 帧协议与容器控制台完全一致，解析与状态归约来自 `models/ptySession.ts`。

import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import '@xterm/xterm/css/xterm.css';
import { adminApi } from '../../api';
import { createTerminal } from '../../models/ptyTerminal';
import {
  parseServerFrame,
  reducePtyState,
  type PtyEvent,
  type PtyStatus,
  type TerminalHandle,
} from '../../models/ptySession';

const props = defineProps<{
  serverId: string;
  /** 父级的模块切换结果。隐藏时不断会话，只跳过 fit。 */
  visible: boolean;
}>();

const status = ref<PtyStatus>('idle');
/** 核心回显的实际启动命令：现在用的是哪个 shell 不该靠猜。 */
const command = ref('');
const errorText = ref('');

const host = ref<HTMLDivElement | null>(null);

let handle: TerminalHandle | null = null;
let socket: WebSocket | null = null;
let observer: ResizeObserver | null = null;
/** 重连序号：`connect` 里有 await，防止旧的一轮把新的顶掉。 */
let seq = 0;

const statusText = computed(() => {
  switch (status.value) {
    case 'idle':
      return '未连接';
    case 'connecting':
      return '正在连接…';
    case 'open':
      return '已连接';
    case 'closed':
      return '会话已结束';
    case 'error':
      return '连接失败';
  }
});

function dispatch(event: PtyEvent): void {
  status.value = reducePtyState(status.value, event);
}

function send(payload: unknown): void {
  const ws = socket;
  if (!ws || ws.readyState !== WebSocket.OPEN) return;
  ws.send(JSON.stringify(payload));
}

/** 对端关了就写一行提示，而不是留一个死屏。 */
function writeNotice(text: string): void {
  handle?.term.write(`\r\n\u001b[2m${text}\u001b[0m\r\n`);
}

/** 隐藏时 `offsetParent` / 宽度都为 0，这时 fit 出来的尺寸是 2×1。 */
function isVisible(): boolean {
  const el = host.value;
  return !!el && el.offsetParent !== null && el.offsetWidth > 0;
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

  handle?.dispose();
  handle = null;
}

async function connect(): Promise<void> {
  const ticket = ++seq;
  teardown();
  errorText.value = '';
  command.value = '';
  dispatch('connect');

  // host 由 v-show 控制，通常已在 DOM 里；等一帧拿准确尺寸
  await nextTick();
  if (ticket !== seq || !host.value) return;

  const created = createTerminal(host.value);
  handle = created;
  created.fit();

  const cols = Math.max(20, created.term.cols || 100);
  const rows = Math.max(5, created.term.rows || 30);

  let ws: WebSocket;
  try {
    ws = await adminApi.openServerTerminal(props.serverId, cols, rows);
  } catch (error) {
    if (ticket !== seq) return;
    errorText.value = error instanceof Error ? error.message : String(error);
    dispatch('error');
    return;
  }
  if (ticket !== seq) {
    ws.close();
    return;
  }

  ws.binaryType = 'arraybuffer';
  socket = ws;

  // 逐键发送（不是整行）：vim / htop / tab 补全都靠这个
  created.term.onData((data) => send({ type: 'input', data }));
  // 终端尺寸变了要通知远端，否则全屏程序按旧列数排版
  created.term.onResize((size) => send({ type: 'resize', cols: size.cols, rows: size.rows }));

  ws.onmessage = (event: MessageEvent) => {
    if (typeof event.data === 'string') {
      const frame = parseServerFrame(event.data);
      if (frame) {
        if (frame.type === 'hello') {
          command.value = frame.command;
          dispatch('hello');
          created.term.focus();
          return;
        }
        if (frame.type === 'exit') {
          dispatch('exit');
          writeNotice(`会话已结束 · 退出码 ${frame.code}`);
          return;
        }
        errorText.value = frame.message;
        dispatch('error');
        return;
      }
      // 不是 JSON 就当普通输出
      created.term.write(event.data);
      return;
    }

    if (event.data instanceof ArrayBuffer) {
      created.term.write(new Uint8Array(event.data));
    }
  };

  ws.onerror = () => {
    if (status.value !== 'error') {
      // 核心主动拒绝（隧道不存在、会话建不起来）会走 `error` 帧并带原因；
      // 能落到 onerror 就只剩「握手没成」，别把两个原因混成一句话。
      errorText.value =
        '连不上核心：应用可能刚重启，等两秒点「重开」；也可能是本地端口被别的程序占了';
      dispatch('error');
    }
  };

  ws.onclose = () => dispatch('close');

  observer = new ResizeObserver(() => {
    if (!isVisible()) return;
    created.fit();
  });
  observer.observe(host.value);
}

function reopen(): void {
  void connect();
}

function clearView(): void {
  handle?.term.clear();
}

function focus(): void {
  handle?.term.focus();
}

/**
 * 从隐藏回到可见：容器尺寸变了但 ResizeObserver 在 display:none 期间给的是 0，
 * 这里显式补一次 fit，并把 xterm 的可见缓冲区重画一遍（隐藏期间写进去的输出）。
 */
watch(
  () => props.visible,
  async (visible) => {
    if (!visible) return;
    await nextTick();
    const current = handle;
    if (!current) return;
    current.fit();
    current.term.refresh(0, Math.max(0, current.term.rows - 1));
  },
);

onMounted(() => {
  void connect();
});

onBeforeUnmount(() => {
  ++seq;
  teardown();
});
</script>

<template>
  <div class="tp">
    <div class="tp-bar">
      <span class="tp-dot" :class="`is-${status}`" />
      <span class="tp-status">{{ statusText }}</span>
      <span class="tp-command mono" :title="command || '登录 shell'">{{ command || '登录 shell' }}</span>
      <span class="spacer" />
      <button class="btn is-xs" @click="clearView">清除</button>
      <button class="btn is-xs" @click="reopen">重开</button>
    </div>

    <div ref="host" class="tp-term" @click="focus" />

    <div v-if="status === 'error'" class="tp-error">{{ errorText }}</div>
    <p v-else-if="status === 'closed'" class="tp-notice">
      会话已结束。点「重开」重新进入这台服务器。
    </p>

    <p class="hint">
      交互式登录 shell（xterm）：支持全屏程序、Tab 补全与 Ctrl 组合键。会话不落盘；
      切到「概览」「容器」不会断开，返回隧道页即断开，远端的 shell 会一起退出。
    </p>
  </div>
</template>

<style scoped>
.tp {
  display: flex;
  flex-direction: column;
  gap: 8px;
  min-height: 0;
  flex: 1;
}

.tp-bar {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}
.tp-dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--text-3);
}
.tp-dot.is-open {
  background: var(--mint);
}
.tp-dot.is-connecting {
  background: #fbbf24;
}
.tp-dot.is-error {
  background: var(--bad-text);
}
.tp-status {
  font-size: 11px;
  color: var(--text-2);
}
.tp-command {
  font-size: 10px;
  color: var(--text-3);
  max-width: 52%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* xterm 自己管内部布局，外层只负责给一块有尺寸的区域 */
.tp-term {
  flex: 1;
  min-height: 320px;
  padding: 8px;
  border: 1px solid var(--line);
  border-radius: 8px;
  background: #0a171a;
  overflow: hidden;
}
.tp-term :deep(.xterm) {
  height: 100%;
}
.tp-term :deep(.xterm-viewport) {
  background: transparent !important;
}

.tp-error {
  padding: 7px 10px;
  border-radius: 8px;
  background: rgba(248, 113, 113, 0.12);
  color: var(--bad-text);
  font-size: 11px;
}
.tp-notice {
  font-size: 11px;
  color: var(--text-3);
}
</style>

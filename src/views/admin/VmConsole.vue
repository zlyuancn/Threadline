<script setup lang="ts">
// 虚拟机串口控制台：xterm.js + 一条 WebSocket 直连远端的 `virsh console`。
//
// 与 `ContainerConsole` 的差别只有一处：没有档位可切。串口就是串口，
// VNC 图形控制台不在本期（RFB 要另算前端体积）。
//
// 帧协议完全复用（`interface/mod.rs` 的 ServerFrame / ClientFrame）：
//   hello（JSON）/ 二进制原始输出 / exit / error；客户端发 input / resize / close。

import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue';
import { adminApi } from '../../api';
import { createTerminal } from '../../models/ptyTerminal';
import {
  parseServerFrame,
  reducePtyState,
  type PtyStatus,
  type TerminalHandle,
} from '../../models/ptySession';

const props = defineProps<{ serverId: string; name: string; state: string }>();

/** 服务端回显的实际命令：用户该知道连的是 `virsh console <name>`。 */
const command = ref('');
const status = ref<PtyStatus>('idle');
const errorText = ref('');

const host = ref<HTMLDivElement | null>(null);

let handle: TerminalHandle | null = null;
let socket: WebSocket | null = null;
let observer: ResizeObserver | null = null;
/** 重连序号：`connect` 里有 await，防止旧的一轮把新的顶掉。 */
let seq = 0;

/** 串口只在域跑着的时候有输出；暂停的域连上去只会挂着，不给这个入口。 */
const canConnect = computed(() => props.state === 'running');

function send(payload: unknown): void {
  const ws = socket;
  if (!ws || ws.readyState !== WebSocket.OPEN) return;
  ws.send(JSON.stringify(payload));
}

function writeNotice(text: string): void {
  handle?.term.write(`\r\n\u001b[2m${text}\u001b[0m\r\n`);
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
  if (!canConnect.value) return;

  const ticket = ++seq;
  teardown();
  status.value = 'connecting';
  errorText.value = '';
  command.value = '';

  // host 是 v-if 控制的：不等到下一帧，容器还没尺寸，fit 会算出 0 列。
  await nextTick();
  if (ticket !== seq || !host.value) return;

  const instance = createTerminal(host.value, { fontSize: 11.5 });
  handle = instance;
  try {
    instance.fit();
  } catch {
    /* 尺寸还没算出来，用下面的兜底值 */
  }

  const cols = Math.max(20, instance.term.cols || 100);
  const rows = Math.max(5, instance.term.rows || 30);

  // 打开失败（核心刚重启、本地端口被占）会抛在这里：写进状态，不要留一块白屏。
  let ws: WebSocket;
  try {
    ws = await adminApi.openVmConsole(props.serverId, props.name, cols, rows);
  } catch (error) {
    errorText.value = error instanceof Error ? error.message : `${error}`;
    status.value = 'error';
    return;
  }
  ws.binaryType = 'arraybuffer';
  socket = ws;

  instance.term.onData((data) => send({ type: 'input', data }));
  instance.term.onResize((size) => send({ type: 'resize', cols: size.cols, rows: size.rows }));

  ws.onmessage = (event: MessageEvent) => {
    if (typeof event.data === 'string') {
      const frame = parseServerFrame(event.data);
      if (frame?.type === 'hello') {
        command.value = frame.command;
        status.value = reducePtyState(status.value, 'hello');
        instance.term.focus();
        return;
      }
      if (frame?.type === 'exit') {
        status.value = reducePtyState(status.value, 'exit');
        writeNotice(`serial closed · exit ${frame.code}`);
        return;
      }
      if (frame?.type === 'error') {
        errorText.value = frame.message;
        status.value = reducePtyState(status.value, 'error');
        return;
      }
      instance.term.write(event.data);
      return;
    }

    if (event.data instanceof ArrayBuffer) {
      instance.term.write(new Uint8Array(event.data));
    }
  };

  ws.onerror = () => {
    if (status.value !== 'error') {
      // 核心主动拒绝（会话被占、域状态变了）会走 `error` 帧，带具体原因；
      // 能落到 onerror 就只剩「握手没成」一种可能。
      errorText.value = '连不上核心：应用可能刚重启，等两秒点「重开」；也可能是本地端口被别的程序占了';
      status.value = reducePtyState(status.value, 'error');
    }
  };

  ws.onclose = () => {
    status.value = reducePtyState(status.value, 'close');
  };

  observer = new ResizeObserver(() => {
    try {
      instance.fit();
    } catch {
      /* 抽屉正在收起，忽略 */
    }
  });
  observer.observe(host.value);
}

function reopen(): void {
  void connect();
}

function focus(): void {
  handle?.term.focus();
}

watch(
  () => [props.name, props.state],
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
  <div class="vc">
    <div class="vc-bar">
      <span class="vc-note">
        串口控制台 · <span class="mono">virsh console</span>
        <template v-if="command"> · 当前命令：<span class="mono">{{ command }}</span></template>
      </span>
      <button class="btn is-xs" @click="reopen">重开</button>
    </div>

    <div v-if="!canConnect" class="vc-notice">
      虚拟机现在没有在运行。先「开机」，等状态变成运行中再回到这一页。
    </div>

    <template v-else>
      <div ref="host" class="vc-term" @click="focus" />
      <div v-if="status === 'error'" class="vc-error">{{ errorText }}</div>
      <div v-else-if="status === 'closed'" class="vc-error">
        串口会话已结束。点「重开」重新进入。
      </div>

      <p class="hint">
        串口输出来自客户机自己：Linux 发行版要配 <code>console=ttyS0</code> 才有登录提示，
        Windows 默认没有串口输出（<code>virsh console</code> 连上空等）。会话不落盘，
        关闭抽屉即断开，远端的 <code>virsh console</code> 会一起退出。
      </p>
    </template>
  </div>
</template>

<style scoped>
.vc {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.vc-bar {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}
.vc-note {
  flex: 1;
  min-width: 140px;
  font-size: 10.5px;
  color: var(--text-3);
}

.vc-notice {
  padding: 14px 12px;
  border: 1px solid var(--line);
  border-radius: 8px;
  font-size: 11.5px;
  color: var(--text-3);
}

/* xterm 自己管内部布局，外层只负责给一块有尺寸的区域 */
.vc-term {
  height: 380px;
  padding: 8px;
  border: 1px solid var(--line);
  border-radius: 8px;
  background: #0a171a;
  overflow: hidden;
}
.vc-term :deep(.xterm) {
  height: 100%;
}
.vc-term :deep(.xterm-viewport) {
  background: transparent !important;
}

.vc-error {
  padding: 7px 10px;
  border-radius: 8px;
  background: rgba(248, 113, 113, 0.12);
  color: var(--bad-text);
  font-size: 11px;
}
</style>

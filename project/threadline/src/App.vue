<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue';
import { ElMessage } from 'element-plus';
import { useRuntimeStore } from './models/runtime';
import { useSelectionStore } from './models/selection';
import { useHotkeyStore } from './models/hotkey';
import { formatRate, useTrafficStore } from './models/traffic';
import { subscribeEvents, type EventStreamHandle } from './api/events';
import { hostsApi } from './api';
import type { HostsStatus } from './api/types';
import { useTunnelsStore } from './models/tunnels';
import { useServersStore } from './models/servers';
import {
  coreState,
  fixDataDirAcl,
  getDataDirStatus,
  isTauri,
  onCoreStateChange,
  refreshBootstrap,
  startCore,
  syncHostsElevated,
  syncLogonEntry,
  syncProxyOverride,
} from './api/native';
import { hasToken } from './api/client';
import { useAsyncAction } from './models/asyncAction';
import AppsView from './views/AppsView.vue';
import AllForwardsView from './views/AllForwardsView.vue';
import HomeView from './views/HomeView.vue';
import LogsView from './views/LogsView.vue';
import SettingsView from './views/SettingsView.vue';

type Tab = 'apps' | 'all' | 'forwards' | 'logs' | 'settings';

const tab = ref<Tab>('apps');
const runtime = useRuntimeStore();
const tunnels = useTunnelsStore();
const servers = useServersStore();

// 心跳只问"隧道状态变了吗、核心还活着吗"——`/tunnels` 是纯内存状态。
// `/whoami`（身份/运行方式/自启）只在启动、手动刷新、掉线恢复时取，
// 因为它内部要查任务计划（起进程），不该被当成心跳每 5 秒拉一次。
/**
 * 在线时的兜底轮询间隔。
 *
 * 状态变更由核心通过 SSE 主动推（实测 5ms 到界面），所以轮询只当"保险丝"：
 * 漏事件、或事件流被中间层掐断时，最多 5s 也能对上。
 */
const POLL_ONLINE_MS = 5000;
const POLL_OFFLINE_MS = 5000;
const POLL_HIDDEN_MS = 5000;

let timer: number | undefined;
let delay = POLL_ONLINE_MS;
let unlisten: (() => void) | undefined;

const connectionTag = computed(() => {
  if (runtime.connection === 'online') return { type: 'success' as const, text: '核心在线' };
  if (runtime.connection === 'offline') return { type: 'danger' as const, text: '核心不可达' };
  return { type: 'info' as const, text: '连接中' };
});

/** 侧栏底部的小圆点：颜色即状态，文字只做补充。 */
const selection = useSelectionStore();
const hotkey = useHotkeyStore();
const traffic = useTrafficStore();

/**
 * 单条隧道的速率；两端都为 0 时返回 null（模板据此不渲染）。
 *
 * 上行 / 下行分开给值，是为了让它们用不同颜色——同一个数字换个符号最容易看错。
 */
function speedOf(serverId: string): { up: string; down: string } | null {
  const speed = traffic.tunnelSpeed(serverId);
  const up = formatRate(speed.up);
  const down = formatRate(speed.down);
  if (!up && !down) {
    return null;
  }
  return { up: up || '0', down: down || '0' };
}

/** 全部隧道的合计速率，挂在侧栏底部——不占主区空间。 */
const totalRate = computed(() => {
  const up = formatRate(traffic.totalSpeed.up);
  const down = formatRate(traffic.totalSpeed.down);
  return up || down ? { up, down } : null;
});

/* ── 配置目录权限 ─────────────────────────────────────────────── */

const dataDir = ref<{ path: string; writable: boolean } | null>(null);
const fixingAcl = ref(false);

async function refreshDataDir() {
  dataDir.value = await getDataDirStatus();
}

async function repairAcl() {
  if (fixingAcl.value) {
    return;
  }
  fixingAcl.value = true;
  try {
    const message = await fixDataDirAcl();
    ElMessage.success(message);
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : String(e));
  } finally {
    fixingAcl.value = false;
    await refreshDataDir();
  }
}

/** 侧栏点隧道 = 切回转发页并选中它。 */
function openTunnel(id: string) {
  selection.select(id);
  tab.value = 'forwards';
}

/**
 * 「应用」「所有转发」里点编辑：只切到隧道页不够，用户还得自己找那条转发。
 * 带上转发 id 交给隧道页滚动定位并直接打开编辑弹窗；nonce 让重复点同一条也能重新触发。
 */
const focusForward = ref<{ id: string; nonce: number } | null>(null);

function openForwardEditor(serverId: string, forwardId: string) {
  selection.select(serverId);
  tab.value = 'forwards';
  focusForward.value = { id: forwardId, nonce: (focusForward.value?.nonce ?? 0) + 1 };
}

/** 「新建连接」在侧栏，弹窗在主区：走一次信号。 */
function newTunnel() {
  tab.value = 'forwards';
  selection.requestCreate();
}

/** 侧栏隧道卡片上的小圆点：和其他地方同一套三态语义。 */
function tunnelDot(id: string): string {
  if (servers.pendingFingerprint(id)) {
    return 'is-warn';
  }
  if (!servers.sessionKnownOf(id)) {
    return 'is-warn';
  }
  return servers.sessionConnected(id) ? 'is-on' : '';
}

function forwardCountOf(serverId: string): number {
  return tunnels.items.filter((item) => item.definition.server_id === serverId).length;
}

const runningCount = computed(
  () => servers.servers.filter((server) => servers.sessionConnected(server.id)).length,
);

function initial(name: string): string {
  return (name.trim()[0] ?? '?').toUpperCase();
}

// 开机自启只拉起核心；登录后的托盘靠这条 HKCU 启动项。
// 每次启动都同步一次，用户手动删掉注册表项也能自愈。
watch(
  () => runtime.autostart?.enabled,
  (enabled) => {
    if (enabled === undefined) {
      return;
    }
    syncLogonEntry(enabled).catch(() => {
      // 登录项写不进去不该打断主流程，设置页里会再暴露一次
    });
  },
  { immediate: true },
);

const coreDot = computed(() => {
  if (runtime.connection === 'online') return 'is-on';
  if (runtime.connection === 'offline') return 'is-bad';
  return 'is-unknown';
});


const degradations = computed(() => runtime.degradationLabels());

/* ── 域名访问：横幅与一键动作 ───────────────────────────────── */

const hosts = ref<HostsStatus | null>(null);
/** 代理例外正在自动同步：防止事件连发时并发写注册表。 */
const proxySyncing = ref(false);
/**
 * 上一次自动尝试过的 desired 集合。
 *
 * 失败后事件还会继续来（`pending` 仍为真），没有这个记录会变成无限重试 +
 * 无限弹错。同一集合只自动试一次，之后交给用户点「同步」。
 */
const lastProxyAttempt = ref('');

async function refreshHosts() {
  if (runtime.connection !== 'online') {
    hosts.value = null;
    return;
  }
  try {
    hosts.value = await hostsApi.status();
  } catch {
    hosts.value = null;
  }
  // 刷新完立刻尝试自动同步：建/删域名、切换域名后不需要用户再点一次。
  void autoSyncProxy();
}

type HostsAction = { text: string; actionLabel: string; action: () => void };

/** 横幅的触发源只有这一处；其余章节只引用（设计 §8）。 */
const hostsBanner = computed<HostsAction | null>(() => {
  const status = hosts.value;
  if (!status) {
    return null;
  }
  if (status.conflicts.length > 0) {
    return {
      text: `域名 ${status.conflicts[0].domain} 与 hosts 中已有条目冲突`,
      actionLabel: '查看',
      action: () => {
        tab.value = 'settings';
      },
    };
  }
  // 没有任何域名时，hosts 里即便还留着旧托管块也没必要提示「0 个域名待写入」，
  // 那种残留由「设置 → 域名访问」的清空动作处理。
  if (status.managed.length === 0) {
    return null;
  }
  if (status.pending || status.needs_elevation) {
    return {
      text: `${status.managed.length} 个域名待写入 hosts`,
      actionLabel: '写入',
      action: () => {
        void writeHosts();
      },
    };
  }
  if (status.proxy.pac) {
    return {
      text: '系统代理使用 PAC 脚本，域名的代理例外不生效',
      actionLabel: '查看',
      action: () => {
        tab.value = 'settings';
      },
    };
  }
  if (status.proxy.enabled && status.proxy.pending) {
    return {
      text: '系统代理已开启，域名的代理例外未同步',
      actionLabel: '同步',
      action: () => {
        void syncProxy();
      },
    };
  }
  return null;
});

async function writeHosts() {
  try {
    const result = await hostsApi.sync();
    if (result.written) {
      ElMessage.success('已写入 hosts');
    } else if (result.needs_elevation) {
      await syncHostsElevated();
      ElMessage.success('已写入 hosts');
    } else {
      ElMessage.warning(result.status.last_error ?? '写入未完成');
    }
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : String(e));
  } finally {
    await refreshHosts();
  }
}

async function syncProxy() {
  const status = hosts.value;
  if (!status) {
    return;
  }
  try {
    const report = await syncProxyOverride(status.proxy.desired, status.proxy.managed);
    await hostsApi.proxyResult({
      ok: report.ok,
      code: report.ok ? 0 : 1,
      applied: report.applied,
      message: report.message ?? undefined,
      at: new Date().toISOString(),
      enabled: report.enabled,
      pac: report.pac,
    });
    if (report.ok) {
      ElMessage.success('代理例外已同步');
    } else {
      ElMessage.error(report.message ?? '代理例外同步失败');
    }
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : String(e));
  } finally {
    await refreshHosts();
  }
}

/**
 * 使用过程中的自动同步（设计 §3.3）。
 *
 * 代理例外写在 HKCU，Agent 不需要提权，所以这类变更**不该让用户再点一次**：
 * 新建域名、删除域名、改域名都会让 `desired` 变化，这里直接落盘。
 *
 * 只在「系统代理已开启且不是 PAC」时做；PAC 模式下例外本来就不生效。
 */
async function autoSyncProxy() {
  const status = hosts.value;
  if (!status || proxySyncing.value) {
    return;
  }
  const { proxy } = status;
  // 两件事都走这同一条路径：
  // - `pending`：域名集合变了，需要 reconcile；
  // - `enabled === null`：Core 重启后还没拿到系统代理状态（它不落盘），借这次调用探测一次。
  if ((!proxy.pending && proxy.enabled !== null) || proxy.pac) {
    return;
  }
  // 系统代理明确关着、也没有待同步的域名：不写注册表。
  if (proxy.enabled === false && !proxy.pending) {
    return;
  }
  const key = proxy.desired.join(';');
  if (lastProxyAttempt.value === key) {
    return;
  }
  lastProxyAttempt.value = key;
  proxySyncing.value = true;
  try {
    const report = await syncProxyOverride(proxy.desired, proxy.managed);
    await hostsApi.proxyResult({
      ok: report.ok,
      code: report.ok ? 0 : 1,
      applied: report.applied,
      message: report.message ?? undefined,
      at: new Date().toISOString(),
      enabled: report.enabled,
      pac: report.pac,
    });
    if (!report.ok) {
      ElMessage.error(report.message ?? '代理例外同步失败');
    }
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : String(e));
  } finally {
    proxySyncing.value = false;
    // 回报后重新拉一次：pending 归零，横幅与设置页同时更新。
    await refreshHosts();
  }
}

async function refreshAll() {
  await runtime.refresh();
  if (runtime.connection === 'online') {
    await Promise.all([tunnels.refresh(), servers.refresh(), refreshHosts()]);
  }
}

async function stopService(): Promise<void> {
  await runtime.shutdown();
  // 核心正在退出，立刻探一次，让按钮尽快翻转成「启动服务」。
  await pollNow();
}

async function startService(): Promise<void> {
  await startCore();
  // 核心起来需要一两秒：每轮先重新注入（新实例=新 token），再探。
  for (let i = 0; i < 12; i += 1) {
    await new Promise((resolve) => window.setTimeout(resolve, 800));
    await refreshBootstrap();
    if (await tunnels.refresh()) {
      runtime.markOnline();
      await refreshAll();
      return;
    }
  }
  throw new Error('启动超时：核心没有在预期时间内就绪');
}

// 启停共用同一把"锁"：进行中时按钮转圈、再点直接忽略。
const serviceAction = useAsyncAction(
  async () => {
    if (runtime.connection === 'online') {
      await stopService();
      ElMessage.success('已停止服务（不影响开机自启，重启电脑后会照常启动）');
    } else {
      await startService();
      ElMessage.success('服务已启动');
    }
  },
  { timeoutMs: 30_000, timeoutMessage: '启动超时：核心没有在预期时间内就绪' },
);
const servicePending = serviceAction.pending;

async function toggleService() {
  const ok = await serviceAction.run();
  if (!ok && serviceAction.error.value) {
    ElMessage.error(serviceAction.error.value);
  }
}

const refreshAction = useAsyncAction(refreshAll, { timeoutMs: 15_000 });
const refreshPending = refreshAction.pending;

async function manualRefresh() {
  const ok = await refreshAction.run();
  if (!ok && refreshAction.error.value) {
    ElMessage.error(refreshAction.error.value);
  }
}

/** true = 连接态由 agent 的探活线程推送（托盘与面板共用一个判断）。 */
const stateDriven = ref(false);

async function poll() {
  // 核心可能在外部被拉起（用户双击 tl.exe）：没有令牌时先尝试重新注入，再探。
  if (isTauri() && !hasToken()) {
    await refreshBootstrap();
  }
  const ok = await tunnels.refresh();
  // 顺带对齐一次：事件流被掐断时，代理例外仍能靠心跳自动补齐。
  void refreshHosts();
  // 非 Tauri 环境（浏览器调试）没有推送源，退化为自己判断。
  if (!stateDriven.value) {
    if (ok) {
      runtime.markOnline();
    } else {
      runtime.markOffline(tunnels.lastError || '核心不可达');
    }
  }
  const hidden = document.visibilityState === 'hidden';
  delay = ok ? (hidden ? POLL_HIDDEN_MS : POLL_ONLINE_MS) : POLL_OFFLINE_MS;
  timer = window.setTimeout(poll, delay);
}

/* ── 实时推送 ───────────────────────────────────────────────── */

let eventStream: EventStreamHandle | null = null;
let pushRefresh: number | null = null;

/**
 * 事件可能连发（比如一条转发从 connecting → running）。
 * 用 120ms 的合并窗口把一串事件收敛成一次拉取，避免把核心打出一串重复请求。
 */
function schedulePushRefresh(): void {
  if (pushRefresh !== null) {
    return;
  }
  pushRefresh = window.setTimeout(() => {
    pushRefresh = null;
    void tunnels.refresh();
    // 会话状态（连上/断开）也可能在同一时刻变，低频补一次即可
    void servers.refresh();
    // hosts / 代理 / 证书状态也在同一条事件流上——不重拉的话横幅不会消失。
    void refreshHosts();
  }, 120);
}

function startEventStream(): void {
  if (eventStream) {
    return;
  }
  eventStream = subscribeEvents(() => {
    runtime.markOnline();
    schedulePushRefresh();
  });
}

// 从后台切回前台时立刻补一次，不用干等下一个 tick。
document.addEventListener('visibilitychange', () => {
  if (document.visibilityState === 'visible' && !timer) {
    void pollNow();
  }
});

async function pollNow() {
  if (timer) window.clearTimeout(timer);
  await poll();
}

onMounted(async () => {
  await hotkey.load();
  await refreshDataDir();

  // 先取一次初始状态（避免只依赖事件时的时序空窗）。
  const initial = await coreState();
  if (initial !== null) {
    stateDriven.value = true;
    if (initial) {
      runtime.markOnline();
    } else {
      runtime.markOffline('核心未运行');
    }
    unlisten = await onCoreStateChange(async (alive) => {
      if (alive) {
        runtime.markOnline();
        await refreshAll();
      } else {
        runtime.markOffline('核心未运行');
        // 核心不在了，表里的"运行中"就是假的，清掉免得误导。
        tunnels.reset();
        servers.reset();
      }
    });
  }

  await refreshAll();
  // 事件流在建好初始数据之后再订阅：先有底图，再谈增量。
  startEventStream();
  timer = window.setTimeout(poll, delay);
});

onUnmounted(() => {
  if (timer) window.clearTimeout(timer);
  if (pushRefresh !== null) window.clearTimeout(pushRefresh);
  eventStream?.close();
  eventStream = null;
  unlisten?.();
});
</script>

<template>
  <el-container class="app-shell">
    <aside class="rail">
      <div class="brand">
        <img src="/logo.svg" alt="" />
        <div class="brand-text">
          <div class="brand-name">THREADLINE</div>
          <div class="brand-sub">SSH 端口转发</div>
        </div>
      </div>

      <!-- 隧道列表：主对象放最上面 -->
      <div class="rail-section">
        <span class="rail-section-k">SSH 连接</span>
        <span class="rail-section-n">{{ servers.servers.length }}</span>
      </div>

      <button class="new-conn" @click="newTunnel">
        <span class="new-conn-icon">+</span>
        新建连接
      </button>

      <div class="conn-list">
        <button
          v-for="server in servers.servers"
          :key="server.id"
          class="conn-item"
          :class="{ 'is-active': tab === 'forwards' && selection.selectedId === server.id }"
          @click="openTunnel(server.id)"
        >
          <span class="conn-avatar">{{ initial(server.name) }}</span>
          <span class="conn-body">
            <span class="conn-name">
              {{ server.name }}
              <span class="dot" :class="tunnelDot(server.id)" />
            </span>
            <span class="conn-host mono">{{ server.username }}@{{ server.host }}</span>
            <span v-if="speedOf(server.id)" class="conn-speed">
              <span class="rate-up">↑ {{ speedOf(server.id)?.up }}</span>
              <span class="rate-down">↓ {{ speedOf(server.id)?.down }}</span>
            </span>
          </span>
          <span class="conn-count">{{ forwardCountOf(server.id) }}</span>
        </button>

        <div v-if="!servers.servers.length" class="rail-empty">
          还没有连接。点上面「新建连接」添加。
        </div>
      </div>

      <span class="spacer" />

      <!-- 状态 + 快捷键：原来的"停止服务"大按钮太容易误触，已移到设置页 -->
      <div class="rail-status">
        <span class="dot" :class="runningCount > 0 ? 'is-on' : ''" />
        {{ runningCount }} 条隧道正在运行
      </div>

      <!-- 核心状态块：插在隧道计数与快捷键之间，刷新按钮与版本同行、合计速率占第二行 -->
      <div class="rail-foot">
        <span class="rail-core">
          <span class="dot" :class="coreDot" />
          {{ connectionTag.text }}
        </span>
        <button
          class="rail-refresh"
          title="刷新状态"
          :disabled="refreshPending"
          @click="manualRefresh"
        >
          <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M13 8a5 5 0 1 1-1.6-3.7"/><path d="M13 2.5V5h-2.5"/></svg>
        </button>
        <span class="rail-ver">v0.1.0 · 仅本机</span>
        <span v-if="totalRate" class="rail-rate" title="所有隧道的合计速率">
          <span class="rate-up">↑ {{ totalRate.up }}</span>
          <span class="rate-down">↓ {{ totalRate.down }}</span>
        </span>
      </div>

      <div class="rail-shortcut" :title="`在任何窗口按 ${hotkey.spec} 呼出面板（设置页可改）`">
        <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"><rect x="1.5" y="3.5" width="13" height="9" rx="1.6"/><path d="M4 6.5h.01M6.5 6.5h.01M9 6.5h.01M11.5 6.5h.01M4.5 9.5h7"/></svg>
        快速呼出
        <kbd v-for="key in hotkey.keys" :key="key">{{ key }}</kbd>
      </div>

      <!-- 设置类入口放底部，不再和隧道抢位置 -->
      <!-- 竖排 tab：一行一个入口，不再挤成两个按钮 -->
      <nav class="rail-tabs">
        <button class="rail-tab" :class="{ 'is-active': tab === 'apps' }" @click="tab = 'apps'">
          应用
        </button>
        <button class="rail-tab" :class="{ 'is-active': tab === 'all' }" @click="tab = 'all'">
          所有转发
        </button>
        <button class="rail-tab" :class="{ 'is-active': tab === 'logs' }" @click="tab = 'logs'">
          日志
        </button>
        <button
          class="rail-tab"
          :class="{ 'is-active': tab === 'settings' }"
          @click="tab = 'settings'"
        >
          设置
        </button>
      </nav>
    </aside>

    <el-container class="app-main">
      <el-main class="page-wrap">
        <div v-if="dataDir && !dataDir.writable" class="notice">
          <div class="offline-banner">
            <span>
              配置目录当前<strong>只读</strong>：新建或修改隧道会保存失败（{{
                dataDir.path
              }}）
            </span>
            <button class="btn is-primary" :disabled="fixingAcl" @click="repairAcl">
              {{ fixingAcl ? '等待授权…' : '修复权限' }}
            </button>
          </div>
        </div>

        <div v-if="hostsBanner" class="notice">
          <div class="offline-banner">
            <span>{{ hostsBanner.text }}</span>
            <button class="btn is-primary" @click="hostsBanner.action()">
              {{ hostsBanner.actionLabel }}
            </button>
          </div>
        </div>

        <div v-if="degradations.length" class="notice">
          <span v-for="label in degradations" :key="label" class="pill is-warn">{{ label }}</span>
        </div>

        <div v-if="runtime.connection === 'offline'" class="notice">
          <div class="offline-banner">
            <span>
              核心未运行{{ runtime.lastError ? `：${runtime.lastError}` : '' }}
            </span>
            <button class="btn is-primary" :disabled="servicePending" @click="toggleService">
              {{ servicePending ? '正在启动…' : '启动服务' }}
            </button>
          </div>
        </div>

        <AppsView v-if="tab === 'apps'" @edit-forward="openForwardEditor" />
        <AllForwardsView
          v-else-if="tab === 'all'"
          @open-tunnel="openTunnel"
          @edit-forward="openForwardEditor"
        />
        <HomeView
          v-else-if="tab === 'forwards'"
          :focus-forward="focusForward"
          @focus-handled="focusForward = null"
        />
        <div v-else class="padded">
          <LogsView v-if="tab === 'logs'" />
          <SettingsView v-else />
        </div>
      </el-main>
    </el-container>
  </el-container>
</template>

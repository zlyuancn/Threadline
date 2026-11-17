<script setup lang="ts">
// 容器管理面板：列表（两档密度）+ 右侧抽屉（详情 / 日志 / 环境）+ 「⋯」菜单与确认框。
//
// 这里只做呈现与调度：脚本、解析、动作白名单、状态机全在核心（tl-core 的
// server_admin::containers 与 business::admin）。前端再写一套规则，就会出现
// 「菜单能点、后端 409」这种两边不一致的毛病。

import { computed, defineAsyncComponent, nextTick, onMounted, onUnmounted, reactive, ref, watch } from 'vue';
import {
  ACTION_LABEL,
  allowedActions,
  levelClass,
  memDetail,
  memPercent,
  memText,
  netShort,
  trafficPair,
  shortImage,
  splitImage,
  useContainersStore,
} from '../../models/containers';
import type { ContainerAction, ContainerItem } from '../../api/types';

// xterm 约 74 kB（gzip）：只有真的打开「控制台」页签才该下载它，
// 不该让从不进容器的人也在首屏等这一坨。
const ContainerConsole = defineAsyncComponent(() => import('./ContainerConsole.vue'));

const props = defineProps<{ serverId: string }>();

const store = useContainersStore();
const tab = ref<'detail' | 'logs' | 'console' | 'env'>('detail');
const follow = ref(true);

/** 抽屉当前对着哪个容器（按名字定位：动作接口用的也是名字）。 */
const current = ref<ContainerItem | null>(null);

/** 「⋯」菜单：位置按打开那一刻算，fixed 定位避免被列表容器裁掉。 */
const menu = reactive<{ open: boolean; item: ContainerItem | null; left: number; top: number }>({
  open: false,
  item: null,
  left: 0,
  top: 0,
});

/** 菜单 DOM：定位前要量它的真实高度（写死估值会在最后几行溢出屏幕）。 */
const menuEl = ref<HTMLElement | null>(null);

/** 确认框：动作与删除共用一套。 */
const confirm = reactive<{
  open: boolean;
  action: ContainerAction | 'remove' | null;
  item: ContainerItem | null;
  busy: boolean;
}>({ open: false, action: null, item: null, busy: false });

const removeImage = ref(false);

const countsText = computed(() => {
  const c = store.counts;
  const parts = [`${c.total} 个`];
  if (c.running) parts.push(`${c.running} 运行`);
  if (c.paused) parts.push(`${c.paused} 暂停`);
  if (c.exited) parts.push(`${c.exited} 退出`);
  return parts.join(' · ');
});

const engineText = computed(() => (store.list?.engine ? `${store.list.engine}` : '未检测到引擎'));

/* ── 轮询 ──────────────────────────────────────────────── */

let listTimer: number | undefined;
let statsTimer: number | undefined;
let logsTimer: number | undefined;

/** 窗口不可见时不采：后台标签页刷 5 秒一次的远端命令没有意义。 */
function shouldPoll(): boolean {
  return Boolean(props.serverId) && !document.hidden;
}

function startList(): void {
  window.clearInterval(listTimer);
  let tick = 0;
  listTimer = window.setInterval(() => {
    if (!shouldPoll()) return;
    void store.load(props.serverId);
    // 资源每 3 拍（15 秒）补一次。`stats` 要 2 秒，跟着 5 秒档跑太重；
    // 但完全不刷的话，用户过几分钟回来看到的还是打开时那个快照。
    // 抽屉开着时跳过——那边已经是 3 秒一档。
    tick += 1;
    if (tick % 3 === 0 && !statsTimer) void store.loadStats();
  }, 5000);
}

function stopStats(): void {
  window.clearInterval(statsTimer);
  statsTimer = undefined;
}

function stopLogs(): void {
  window.clearInterval(logsTimer);
  logsTimer = undefined;
}

/**
 * 抽屉打开时资源 3 秒一档。
 *
 * 列表那边也有一份：首屏采一次、之后 15 秒补一次——否则 CPU / MEM / NET
 * 三列在用户眼里永远是空的，那比数字稍微旧一点糟糕得多。
 */
function startStats(): void {
  stopStats();
  void store.loadStats();
  statsTimer = window.setInterval(() => {
    if (shouldPoll()) void store.loadStats();
  }, 3000);
}

function startLogs(): void {
  stopLogs();
  void store.loadLogs(nameOf(current.value), 200);
  logsTimer = window.setInterval(() => {
    if (!shouldPoll() || !follow.value) return;
    // 跟随用 `--since` 增量拉，不整段重拉
    void store.loadLogs(nameOf(current.value), 200, store.logs?.next_since || undefined);
  }, 2000);
}

/** 日志区：跟随模式下来新行了要看到底，但不能把正在上翻的人拽回来。 */
const logsEl = ref<HTMLElement | null>(null);
watch(
  () => store.logs?.lines.length,
  async (_len, prevLen) => {
    const el = logsEl.value;
    if (!follow.value || !el) return;
    // 首次载入直接到底：用户打开日志要看的是最新那几行，不是 200 行前的开头。
    // 之后在 DOM 更新**前**判断是否原本就贴着底部——往上翻着看历史时不该被拽回来。
    const stick = !prevLen || el.scrollHeight - el.scrollTop - el.clientHeight < 60;
    await nextTick();
    if (stick) el.scrollTop = el.scrollHeight;
  },
);

function nameOf(item: ContainerItem | null): string {
  return item?.name ?? '';
}

/* ── 交互 ──────────────────────────────────────────────── */

function openDrawer(
  item: ContainerItem,
  next: 'detail' | 'logs' | 'console' | 'env' = 'detail',
): void {
  closeMenu();
  current.value = item;
  tab.value = next;
  store.detail = null;
  store.logs = null;
  void store.loadDetail(item.name);
  startStats();
  if (next === 'logs') startLogs();
}

function closeDrawer(): void {
  current.value = null;
  store.detail = null;
  store.logs = null;
  stopStats();
  stopLogs();
}

function changeTab(next: 'detail' | 'logs' | 'console' | 'env'): void {
  tab.value = next;
  if (next === 'logs') startLogs();
  else stopLogs();
}

/** 菜单开着的那一行要看得出来——菜单浮在下面几行上，不标出来就对不上是哪一行的。 */
function isMenuRow(item: ContainerItem): boolean {
  return menu.open && menu.item?.id === item.id;
}

async function openMenu(event: MouseEvent, item: ContainerItem): Promise<void> {
  const el = event.currentTarget as HTMLElement;
  const rect = el.getBoundingClientRect();
  menu.item = item;
  menu.open = true;

  // 先渲染再量真实高度。原来写死 250 是估的，实际约 280——最后一行打开时
  // 判断「下方放得下」会误判，菜单直接溢出到屏幕外。
  await nextTick();
  const width = menuEl.value?.offsetWidth ?? 168;
  const height = menuEl.value?.offsetHeight ?? 280;
  const below = rect.bottom + 6;

  menu.left = Math.max(12, Math.min(window.innerWidth - width - 12, rect.right - width));
  menu.top =
    below + height > window.innerHeight - 12
      ? Math.max(12, rect.top - height - 6)
      : below;
}

function closeMenu(): void {
  menu.open = false;
  menu.item = null;
}

function ask(action: ContainerAction | 'remove', item: ContainerItem): void {
  closeMenu();
  confirm.open = true;
  confirm.action = action;
  confirm.item = item;
  confirm.busy = false;
  removeImage.value = false;
}

async function runConfirm(): Promise<void> {
  const item = confirm.item;
  const action = confirm.action;
  if (!item || !action) return;

  confirm.busy = true;
  const ok =
    action === 'remove'
      ? await store.remove(item.name, item.state !== 'exited', removeImage.value)
      : await store.act(item.name, action);
  confirm.busy = false;
  confirm.open = false;

  if (ok && action === 'remove' && current.value?.name === item.name) closeDrawer();
}

/** 回执 4 秒后自动消失：否则它就一直挂在那儿像报错。 */
let flashTimer: number | undefined;
watch(
  () => store.feedback,
  (text) => {
    window.clearTimeout(flashTimer);
    if (text) flashTimer = window.setTimeout(() => store.clearFeedback(), 4200);
  },
);

/* ── 生命周期 ──────────────────────────────────────────── */

onMounted(async () => {
  await store.load(props.serverId);
  startList();
});

watch(
  () => props.serverId,
  async (id) => {
    if (!id) return;
    closeDrawer();
    await store.load(id);
    startList();
  },
);

onUnmounted(() => {
  window.clearInterval(listTimer);
  stopStats();
  stopLogs();
  window.clearTimeout(flashTimer);
  store.reset();
});

/* ── 展示口径 ──────────────────────────────────────────── */

function dotClass(item: ContainerItem): string {
  if (item.health === 'unhealthy') return 'is-bad';
  if (item.health === 'starting') return 'is-warn';
  switch (item.state) {
    case 'running':
      return 'is-ok';
    case 'paused':
    case 'restarting':
      return 'is-warn';
    default:
      return 'is-idle';
  }
}

/**
 * 这一行到底有没有资源数据。
 *
 * 资源是单独一档（3s，只在抽屉打开时跑）。没采到时 `cpu_percent` 是 0，
 * 直接渲染会写成「0.0%」——那是「没采」，不是「没占用」。
 */
function liveStats(item: ContainerItem): boolean {
  return item.state !== 'exited' && store.hasStats(item.id);
}

function statusClass(item: ContainerItem): string {
  if (item.health === 'unhealthy') return 'is-bad';
  if (item.state === 'restarting' || item.state === 'paused') return 'is-warn';
  return '';
}

function menuActions(item: ContainerItem): ContainerAction[] {
  return allowedActions(item.state);
}

function canConsole(item: ContainerItem): boolean {
  return item.state === 'running';
}

/**
 * 网络：**IP 在前**。
 *
 * 列宽固定，「网络名 IP」的顺序下名字一长就把 IP 挤没了（`syncthing_default 172.2…`），
 * 而 IP 才是判断「这个容器在哪个网段、能不能被谁访问」的关键，名字放后面被截掉无所谓。
 */
function netText(item: ContainerItem): string {
  return item.networks.length
    ? item.networks.map((n) => `${n.ip} ${n.name}`).join(' / ')
    : '—';
}

/**
 * 端口 / 卷超出显示上限时的汇总。
 *
 * 这一块的行数直接决定整行高度：syncthing 这种 4 端口 2 卷的容器全展开是 6 行、
 * 行高 130px，一屏只看得下 4 个（设计目标是 75px / 一屏 6–7 个）。
 * 所以端口和卷各限 2 条，剩下的折成一行，完整列表在抽屉里。
 */
function overflowText(item: ContainerItem): string {
  const parts: string[] = [];
  if (item.ports.length > 2) parts.push(`+${item.ports.length - 2} 端口`);
  if (item.mounts.length > 2) parts.push(`+${item.mounts.length - 2} 卷`);
  return parts.join(' · ');
}

const detailEnv = computed(() => store.detail?.env ?? []);

const confirmTitle = computed(() => {
  if (confirm.action === 'remove') return '删除容器？';
  return confirm.action ? `${ACTION_LABEL[confirm.action]}容器？` : '';
});

const confirmOk = computed(() => {
  if (confirm.action === 'remove') return '删除';
  return confirm.action ? ACTION_LABEL[confirm.action] : '确定';
});

const confirmDanger = computed(() => confirm.action === 'remove' || confirm.action === 'stop');
</script>

<template>
  <div class="ct">
    <!-- 工具条：引擎与统计在左，搜索与两档切换在右 -->
    <div class="ct-bar">
      <span class="ct-engine mono">{{ engineText }}</span>
      <span class="ct-counts">{{ countsText }}</span>
      <span class="ct-grow" />
      <input v-model="store.query" class="ct-search" placeholder="搜索名称 / 镜像 / ID" />
      <div class="seg">
        <button :class="{ 'is-on': store.filter === 'all' }" @click="store.filter = 'all'">全部</button>
        <button :class="{ 'is-on': store.filter === 'running' }" @click="store.filter = 'running'">
          运行中
        </button>
        <button :class="{ 'is-on': store.filter === 'stopped' }" @click="store.filter = 'stopped'">
          已停止
        </button>
      </div>
      <!-- 密度只影响呈现，不触发任何请求 -->
      <div class="seg" title="主要：状态 · 名称 · 入口 · 资源；详细：再加网络、卷、创建时间">
        <button :class="{ 'is-on': store.density === 'compact' }" @click="store.setDensity('compact')">
          主要
        </button>
        <button :class="{ 'is-on': store.density === 'detail' }" @click="store.setDensity('detail')">
          详细
        </button>
      </div>
    </div>

    <div v-if="store.feedback" class="ct-flash" :class="{ 'is-bad': store.feedbackIsError }">
      {{ store.feedback }}
    </div>
    <div v-if="store.lightError" class="ct-flash is-bad">刷新失败：{{ store.lightError }}</div>

    <div v-if="store.loading" class="admin-loading">正在采集容器列表…</div>

    <div v-else-if="store.error" class="admin-error">
      <div class="admin-error-title">容器列表采集失败</div>
      <div class="admin-error-body">{{ store.error }}</div>
      <p class="hint">
        常见原因：当前 SSH 用户不在 <code>docker</code> 组（会报 permission denied）、
        docker 守护进程没启动。
      </p>
    </div>

    <div v-else-if="!store.hasEngine" class="admin-empty">
      <h4>这台机器上没有容器引擎</h4>
      <p>没检测到 <code>docker</code>。装上之后回到这一页会自动出现容器列表。</p>
    </div>

    <div v-else-if="!store.visible.length" class="admin-empty">
      <h4>没有匹配的容器</h4>
      <p>
        共 {{ store.counts.total }} 个容器，当前筛选与搜索条件下一个都没匹配上。
      </p>
    </div>

    <!-- ── 列表 ─────────────────────────────────────── -->
    <div v-else class="ct-list" :class="{ 'is-compact': store.density === 'compact' }">
      <div class="ct-thead">
        <span />
        <span>容器</span>
        <span v-if="store.density === 'detail'">镜像 · 网络</span>
        <span v-else>镜像</span>
        <span>{{ store.density === 'detail' ? '端口 · 卷' : '入口' }}</span>
        <!-- 主要视图的数值是右对齐的，表头不跟着右对齐就会和数据错开半个字 -->
        <span :class="{ 'is-right': store.density === 'compact' }">
          {{ store.density === 'detail' ? '资源' : 'CPU' }}
        </span>
        <span v-if="store.density === 'compact'" class="is-right">内存</span>
        <span v-else />
      </div>

      <!-- 详细：四块，块内纵向堆叠、块间左对齐 -->
      <template v-if="store.density === 'detail'">
        <div
          v-for="item in store.visible"
          :key="item.id"
          class="ct-row"
          :class="{ 'is-menu-open': isMenuRow(item) }"
          @click="openDrawer(item)"
        >
          <span class="ct-dot" :class="dotClass(item)" />
          <div class="ct-cell">
            <span class="ct-n">{{ item.name }}</span>
            <span class="ct-s" :class="statusClass(item)">{{ item.status }}</span>
            <span class="ct-id mono">{{ item.id }}<template v-if="item.compose_project"> · {{ item.compose_project }}</template></span>
          </div>
          <div class="ct-cell">
            <span class="ct-img mono" :title="item.image">
              <span class="ct-repo">{{ splitImage(item.image).repo }}</span>
              <span class="ct-tag">{{ splitImage(item.image).tag }}</span>
            </span>
            <span class="ct-line mono" :title="netText(item)">网络 <b>{{ netText(item) }}</b></span>
            <span class="ct-line">创建 <b>{{ item.created_at }}</b></span>
          </div>
          <div class="ct-cell">
            <template v-if="item.ports.length">
              <span
                v-for="port in item.ports.slice(0, 2)"
                :key="`${port.host_ip}:${port.host_port}:${port.proto}`"
                class="ct-line mono"
              >
                <span class="ct-ip">{{ port.host_ip }}</span>:<b>{{ port.host_port }}</b>→{{ port.container_port }}/{{ port.proto }}
              </span>
            </template>
            <span v-else class="ct-line">没有端口映射</span>
            <span
              v-for="mount in item.mounts.slice(0, 2)"
              :key="mount.destination"
              class="ct-line mono"
              :title="`${mount.source} → ${mount.destination}`"
            >
              卷 <b>{{ mount.source }} → {{ mount.destination }}{{ mount.mode === 'ro' ? ':ro' : '' }}</b>
            </span>
            <!-- 这一列的行数决定整行高度：全展开能把行撑到 130px，一屏只看得到 4 个 -->
            <span v-if="overflowText(item)" class="ct-line ct-more-n">{{ overflowText(item) }}</span>
          </div>
          <div class="ct-cell ct-res-cell" :title="liveStats(item) ? '' : '资源正在采集（docker stats 自带采样窗口，约 2 秒）'">
            <span class="ct-res"><i>CPU</i><b class="mono">{{ liveStats(item) ? `${item.cpu_percent.toFixed(1)}%` : '—' }}</b></span>
            <span class="ct-res"><i>MEM</i><b class="mono">{{ liveStats(item) ? memText(item) : '—' }}</b></span>
            <span class="ct-res">
              <i>NET</i>
              <b
                class="mono"
                :title="liveStats(item) ? `累计收发 ${trafficPair(item.net_rx_bytes, item.net_tx_bytes)}` : ''"
              >{{ liveStats(item) ? netShort(item.net_rx_bytes) : '—' }}</b>
            </span>
            <span class="ct-res"><i>重启</i><b class="mono" :class="{ 'is-warn': item.restarts > 0 }">{{ item.restarts }} 次</b></span>
          </div>
          <button class="ct-more" title="操作" @click.stop="openMenu($event, item)">⋯</button>
        </div>
      </template>

      <!-- 主要：单行 7 列 -->
      <template v-else>
        <div
          v-for="item in store.visible"
          :key="item.id"
          class="ct-row"
          :class="{ 'is-menu-open': isMenuRow(item) }"
          @click="openDrawer(item)"
        >
          <span class="ct-dot" :class="dotClass(item)" />
          <div class="ct-cell">
            <span class="ct-n">{{ item.name }}</span>
            <span class="ct-s" :class="statusClass(item)">{{ item.status }}</span>
          </div>
          <span class="ct-img mono" :title="item.image">{{ shortImage(item.image) }}</span>
          <span class="ct-line mono">
            <template v-if="item.ports.length">
              {{ item.ports[0].host_ip === '0.0.0.0' ? '' : `${item.ports[0].host_ip}:` }}{{ item.ports[0].host_port }}
              <span v-if="item.ports.length > 1" class="ct-more-n">+{{ item.ports.length - 1 }}</span>
            </template>
            <span v-else class="ct-muted">—</span>
          </span>
          <span class="ct-num mono" :title="liveStats(item) ? '' : '资源正在采集（约 2 秒）'">{{ liveStats(item) ? `${item.cpu_percent.toFixed(1)}%` : '—' }}</span>
          <span class="ct-num mono">{{ liveStats(item) ? memText(item) : '—' }}</span>
          <button class="ct-more" title="操作" @click.stop="openMenu($event, item)">⋯</button>
        </div>
      </template>
    </div>

    <!-- ── 「⋯」菜单 ─────────────────────────────────── -->
    <Teleport to="body">
      <div
        v-if="menu.open && menu.item"
        ref="menuEl"
        class="ct-menu"
        :style="{ left: `${menu.left}px`, top: `${menu.top}px` }"
        @click.stop
      >
        <div class="ct-menu-k">{{ menu.item.name }}</div>
        <button
          v-for="action in menuActions(menu.item)"
          :key="action"
          class="ct-menu-item"
          :class="{ 'is-danger': action === 'stop' }"
          @click="ask(action, menu.item)"
        >
          {{ ACTION_LABEL[action] }}
        </button>
        <div class="ct-menu-sep" />
        <!-- exec 只能进运行中的容器：暂停 / 重启中 / 已停止都不给这一项，免得点了只看到报错 -->
        <button
          v-if="canConsole(menu.item)"
          class="ct-menu-item"
          @click="openDrawer(menu.item, 'console')"
        >
          进入控制台
        </button>
        <button class="ct-menu-item" @click="openDrawer(menu.item, 'logs')">查看日志</button>
        <button class="ct-menu-item" @click="openDrawer(menu.item, 'detail')">详情</button>
        <div class="ct-menu-sep" />
        <button class="ct-menu-item is-danger" @click="ask('remove', menu.item)">删除容器</button>
      </div>
    </Teleport>

    <!-- ── 抽屉 ─────────────────────────────────────── -->
    <Teleport to="body">
      <div v-if="current" class="ct-mask" @click.self="closeDrawer">
        <aside class="ct-drawer">
          <header class="ct-drawer-head">
            <div>
              <div class="ct-drawer-title">{{ current.name }}</div>
              <div class="ct-drawer-sub mono">{{ current.image }}</div>
            </div>
            <button class="icon-btn" title="关闭" @click="closeDrawer">✕</button>
          </header>

          <div class="ct-tabs">
            <button :class="{ 'is-on': tab === 'detail' }" @click="changeTab('detail')">详情</button>
            <button :class="{ 'is-on': tab === 'logs' }" @click="changeTab('logs')">日志</button>
            <button :class="{ 'is-on': tab === 'console' }" @click="changeTab('console')">
              控制台
            </button>
            <button :class="{ 'is-on': tab === 'env' }" @click="changeTab('env')">环境</button>
          </div>

          <div class="ct-drawer-body">
            <!-- 详情 -->
            <template v-if="tab === 'detail'">
              <div class="ct-sec-k">运行</div>
              <dl class="ct-kv">
                <div><dt>状态</dt><dd>{{ current.status }}</dd></div>
                <div><dt>容器 ID</dt><dd class="mono">{{ current.id }}</dd></div>
                <div v-if="current.compose_project"><dt>compose</dt><dd>{{ current.compose_project }}</dd></div>
                <div><dt>创建</dt><dd class="mono">{{ current.created_at }}</dd></div>
                <div><dt>重启</dt><dd>{{ current.restarts }} 次</dd></div>
                <div><dt>健康检查</dt><dd>{{ current.health || '未配置' }}</dd></div>
                <div v-if="store.detail"><dt>重启策略</dt><dd>{{ store.detail.restart_policy || '—' }}</dd></div>
                <div v-if="store.detail"><dt>命令</dt><dd class="mono">{{ store.detail.command || '—' }}</dd></div>
                <div v-if="current.exit_code !== undefined"><dt>退出码</dt><dd class="mono">{{ current.exit_code }}</dd></div>
              </dl>

              <div class="ct-sec-k">资源</div>
              <template v-if="liveStats(current)">
                <div class="ct-res-row">
                  <span class="mono">CPU {{ current.cpu_percent.toFixed(1) }}%</span>
                  <span class="ct-res-bar"><i :class="levelClass(current.cpu_percent)" :style="{ width: `${Math.min(100, current.cpu_percent)}%` }" /></span>
                </div>
                <div class="ct-res-row">
                  <span class="mono">内存 {{ memDetail(current) }}</span>
                  <span class="ct-res-bar"><i :class="levelClass(memPercent(current))" :style="{ width: `${memPercent(current)}%` }" /></span>
                </div>
                <div class="ct-res-row">
                  <span class="mono">收发（累计）{{ trafficPair(current.net_rx_bytes, current.net_tx_bytes) }}</span>
                </div>
              </template>
              <div v-else class="ct-muted">
                正在采集资源（约 1–2 秒，<code>docker stats --no-stream</code> 自带采样窗口）…
              </div>

              <div class="ct-sec-k">端口映射</div>
              <table v-if="current.ports.length" class="admin-table">
                <thead><tr><th>宿主 IP</th><th>端口</th><th>容器</th></tr></thead>
                <tbody>
                  <tr v-for="port in current.ports" :key="`${port.host_ip}:${port.host_port}`">
                    <td class="mono">{{ port.host_ip }}</td>
                    <td class="mono">{{ port.host_port }}</td>
                    <td class="mono">{{ port.container_port }}/{{ port.proto }}</td>
                  </tr>
                </tbody>
              </table>
              <div v-else class="ct-muted">没有映射端口</div>

              <div class="ct-sec-k">卷映射</div>
              <table v-if="current.mounts.length" class="admin-table">
                <thead><tr><th>宿主路径</th><th>容器路径</th><th>模式</th></tr></thead>
                <tbody>
                  <tr v-for="mount in current.mounts" :key="mount.destination">
                    <td class="mono">{{ mount.source }}</td>
                    <td class="mono">{{ mount.destination }}</td>
                    <td class="mono">{{ mount.mode }}</td>
                  </tr>
                </tbody>
              </table>
              <div v-else class="ct-muted">没有卷映射</div>

              <div class="ct-sec-k">网络</div>
              <table v-if="current.networks.length" class="admin-table">
                <thead><tr><th>网络</th><th>容器 IP</th></tr></thead>
                <tbody>
                  <tr v-for="net in current.networks" :key="net.name">
                    <td>{{ net.name }}</td>
                    <td class="mono">{{ net.ip }}</td>
                  </tr>
                </tbody>
              </table>
              <div v-else class="ct-muted">没有网络信息</div>

              <p class="hint">抽屉打开时资源 3 秒刷新（<code>docker stats --no-stream</code>），关掉即停；列表上每 15 秒补一次。</p>
            </template>

            <!-- 日志 -->
            <template v-else-if="tab === 'logs'">
              <label class="ct-follow">
                <input v-model="follow" type="checkbox" />
                跟随（2s，按 <code>--since</code> 增量拉取）
              </label>
              <pre v-if="store.logs?.lines.length" ref="logsEl" class="ct-logs">{{ store.logs.lines.join('\n') }}</pre>
              <div v-else class="ct-muted">没有输出（容器可能没有启动过，或日志已被轮转）</div>
              <p class="hint">
                只显示最后 200 行{{ store.logs?.truncated ? '（已截断）' : '' }}；需要更多请用终端模块。
              </p>
            </template>

            <!-- 控制台：一条 WebSocket 直连 PTY -->
            <template v-else-if="tab === 'console'">
              <ContainerConsole
                :server-id="props.serverId"
                :container-name="current.name"
                :state="current.state"
              />
            </template>

            <!-- 环境 -->
            <template v-else>
              <table v-if="detailEnv.length" class="admin-table">
                <thead><tr><th>变量</th><th>值</th></tr></thead>
                <tbody>
                  <tr v-for="env in detailEnv" :key="env.key">
                    <td class="mono">{{ env.key }}</td>
                    <td class="mono">
                      {{ env.value }}
                      <span v-if="env.masked" class="pill is-warn">已打码</span>
                    </td>
                  </tr>
                </tbody>
              </table>
              <div v-else class="ct-muted">正在读取环境变量，或该容器没有环境变量。</div>
              <p class="hint">密码、令牌这类键由核心打码后下发，前端拿不到明文。</p>
            </template>
          </div>
        </aside>
      </div>
    </Teleport>

    <!-- ── 确认框 ───────────────────────────────────── -->
    <Teleport to="body">
      <div v-if="confirm.open && confirm.item" class="ct-mask is-center" @click.self="confirm.open = false">
        <div class="ct-modal">
          <div class="ct-modal-title">{{ confirmTitle }}</div>
          <div class="ct-modal-body">
            <template v-if="confirm.action === 'remove'">
              容器 <code>{{ confirm.item.name }}</code>
              <template v-if="confirm.item.state === 'running'">
                正在运行，删除会先强制停止（<code>docker rm -f</code>）。
              </template>
              <template v-else>会被删除（<code>docker rm</code>）。</template>

              <label class="ct-check">
                <input v-model="removeImage" type="checkbox" />
                <span>同时删除镜像 <b class="mono">{{ shortImage(confirm.item.image) }}</b>（<code>docker rmi</code>）</span>
              </label>
              <div class="ct-warn">
                容器会被删除，<b>数据卷不会删除</b>；勾选后镜像也一并删除。此操作不可撤销。
              </div>
            </template>
            <template v-else>
              将对 <code>{{ confirm.item.name }}</code> 执行
              <code>docker {{ confirm.action }}</code>。
            </template>
          </div>
          <div class="ct-modal-foot">
            <button class="btn" :disabled="confirm.busy" @click="confirm.open = false">取消</button>
            <button
              class="btn"
              :class="confirmDanger ? 'is-danger' : 'is-primary'"
              :disabled="confirm.busy"
              @click="runConfirm"
            >
              {{ confirm.busy ? '执行中…' : confirmOk }}
            </button>
          </div>
        </div>
      </div>
    </Teleport>
  </div>
</template>

<style scoped>
.ct {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

/* 工具条 */
.ct-bar {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
}
.ct-engine {
  font-size: 11px;
  color: var(--text-2);
}
.ct-counts {
  font-size: 11px;
  color: var(--text-3);
}
.ct-grow {
  flex: 1;
}
.ct-search {
  width: 190px;
  height: 27px;
  padding: 0 9px;
  /* 和分段控件同底色：两个控件并排，一个半透明一个实色会显得脏 */
  border: 1px solid var(--line-strong);
  border-radius: 8px;
  background: var(--card-solid);
  color: var(--text);
  font-size: 11.5px;
}
.ct-search:focus {
  outline: none;
  border-color: var(--line-strong);
}

.ct-flash {
  padding: 7px 10px;
  border: 1px solid var(--line-strong);
  border-radius: 8px;
  font-size: 11.5px;
  color: var(--text-2);
}
.ct-flash.is-bad {
  border-color: var(--bad-text);
  color: var(--bad-text);
}

/* 列表 */
.ct-list {
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.ct-thead,
.ct-row {
  display: grid;
  /* 端口·卷那列要放 `/root/docker/xxx/config → /var/xxx` 这种成对路径，
     给少了就只剩省略号；镜像名通常十几字符就够，让出来的宽度给它。
     资源列收到 130px：`↓103.0 MB` 这类值最长也就这么宽。 */
  grid-template-columns: 16px minmax(150px, 1fr) minmax(140px, 0.95fr) minmax(240px, 1.75fr) 130px 30px;
  gap: 10px;
  align-items: start;
}
.ct-thead {
  padding: 0 12px 2px;
  font-size: 10px;
  font-weight: 600;
  letter-spacing: 1.1px;
  /* 表头是小字，用 --text-3 在浅色主题下对比度只有 3:1 出头，看着发虚 */
  color: var(--text-2);
}
.ct-row {
  padding: 10px 12px;
  border: 1px solid var(--line);
  border-radius: 10px;
  background: var(--card);
  cursor: pointer;
}
.ct-row:hover {
  border-color: var(--line-strong);
}
/* 菜单浮在下面几行上面，行本身不标出来，就对不上这个菜单是哪一行的 */
.ct-row.is-menu-open {
  border-color: var(--line-strong);
  box-shadow: 0 0 0 1px var(--line-strong);
}
.ct-thead .is-right {
  text-align: right;
}

.ct-cell {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

.ct-dot {
  width: 7px;
  height: 7px;
  margin-top: 5px;
  border-radius: 50%;
  background: var(--text-3);
}
.ct-dot.is-ok {
  background: var(--mint);
}
.ct-dot.is-warn {
  background: #fbbf24;
}
.ct-dot.is-bad {
  background: var(--bad-text);
}

.ct-n {
  font-size: 12.5px;
  font-weight: 600;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.ct-s {
  font-size: 10.5px;
  color: var(--text-3);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.ct-s.is-warn {
  color: #fbbf24;
}
.ct-s.is-bad {
  color: var(--bad-text);
}
.ct-id {
  font-size: 9.5px;
  color: var(--text-3);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.ct-img {
  display: flex;
  min-width: 0;
  font-size: 11px;
  color: var(--text-2);
}
.ct-repo {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
/* tag 一定要看得见：repo 可以省略，版本号不能 */
.ct-tag {
  flex-shrink: 0;
  color: var(--cyan);
  opacity: 0.9;
}

.ct-line {
  font-size: 10.5px;
  color: var(--text-3);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.ct-line b {
  color: var(--text-2);
  font-weight: 500;
}
.ct-ip {
  opacity: 0.85;
}
.ct-muted {
  font-size: 11px;
  color: var(--text-3);
}

.ct-res-cell {
  gap: 2px;
}
.ct-res {
  display: grid;
  grid-template-columns: 26px 1fr;
  gap: 6px;
  align-items: baseline;
}
.ct-res i {
  font-size: 9px;
  font-style: normal;
  letter-spacing: 0.5px;
  color: var(--text-3);
}
.ct-res b {
  font-size: 10.5px;
  font-weight: 500;
  color: var(--text-2);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.ct-res b.is-warn {
  color: #fbbf24;
}

.ct-more {
  width: 27px;
  height: 25px;
  padding: 0;
  border: 1px solid var(--line);
  border-radius: 7px;
  background: transparent;
  color: var(--text-2);
  font-size: 15px;
  line-height: 1;
  cursor: pointer;
}
.ct-more:hover {
  background: var(--card);
  border-color: var(--line-strong);
  color: var(--text);
}

/* 主要视图：单行 7 列 */
.ct-list.is-compact .ct-thead,
.ct-list.is-compact .ct-row {
  /* 容器名短、镜像名长：把比例让给镜像那列，否则容器列空出一大片、
     镜像（`opentelemetry-collector-contrib` 这种）还是被截断。 */
  grid-template-columns: 16px minmax(140px, 0.8fr) minmax(220px, 1.5fr) 128px 68px 88px 30px;
  align-items: center;
}
.ct-list.is-compact .ct-row {
  padding: 8px 12px;
}
.ct-num {
  font-size: 11px;
  color: var(--text-2);
  text-align: right;
  font-variant-numeric: tabular-nums;
}
.ct-more-n {
  margin-left: 4px;
  font-size: 9.5px;
  color: var(--text-3);
}

/* 菜单 */
.ct-menu {
  position: fixed;
  z-index: 40;
  min-width: 168px;
  padding: 5px;
  border: 1px solid var(--line-strong);
  border-radius: 10px;
  /* 跟随主题：`--panel` 从来没有定义过，一直回退到写死的深色 #102529。
     在浅色主题下就变成「深底 + 深字」（`--text` 是 #2f2920），整张菜单看不见。
     `--card-solid` 是每个主题都定义好、且不透明的面板底色。 */
  background: var(--card-solid);
  box-shadow: 0 16px 40px rgba(0, 0, 0, 0.28);
}
.ct-menu-k {
  padding: 4px 9px 7px;
  font-size: 11px;
  font-weight: 600;
  letter-spacing: 0.3px;
  color: var(--text-2);
  max-width: 180px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.ct-menu-item {
  display: block;
  width: 100%;
  padding: 7px 9px;
  border: 0;
  border-radius: 7px;
  background: transparent;
  /* 用 --text 而不是 --text-2：菜单项偏灰时整块看着像禁用了 */
  color: var(--text);
  font-size: 12px;
  text-align: left;
  cursor: pointer;
}
.ct-menu-item:hover:not(:disabled) {
  background: var(--tint-strong);
}
.ct-menu-item:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}
.ct-menu-item.is-danger {
  color: var(--bad-text);
}
.ct-menu-item.is-danger:hover:not(:disabled) {
  background: rgba(248, 113, 113, 0.14);
}
.ct-menu-sep {
  height: 1px;
  margin: 6px 4px;
  background: var(--line-strong);
}

/* 抽屉 */
.ct-mask {
  position: fixed;
  inset: 0;
  z-index: 45;
  display: flex;
  justify-content: flex-end;
  background: var(--mask);
}
.ct-mask.is-center {
  align-items: center;
  justify-content: center;
}
.ct-drawer {
  display: flex;
  flex-direction: column;
  width: 460px;
  max-width: 92vw;
  height: 100%;
  border-left: 1px solid var(--line-strong);
  background: var(--card-solid);
}
.ct-drawer-head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 10px;
  padding: 14px 16px 10px;
  border-bottom: 1px solid var(--line);
}
.ct-drawer-title {
  font-size: 14px;
  font-weight: 600;
}
.ct-drawer-sub {
  margin-top: 3px;
  font-size: 10.5px;
  color: var(--text-3);
  word-break: break-all;
}
.ct-tabs {
  display: flex;
  gap: 4px;
  padding: 8px 12px;
  border-bottom: 1px solid var(--line);
}
.ct-tabs button {
  padding: 5px 10px;
  border: 1px solid transparent;
  border-radius: 7px;
  background: transparent;
  color: var(--text-3);
  font-size: 11.5px;
  cursor: pointer;
}
.ct-tabs button.is-on {
  border-color: var(--line-strong);
  background: var(--card);
  color: var(--text);
}
.ct-drawer-body {
  flex: 1;
  overflow-y: auto;
  padding: 12px 16px 20px;
}

.ct-sec-k {
  margin: 14px 0 7px;
  font-size: 10px;
  font-weight: 600;
  letter-spacing: 1px;
  color: var(--text-3);
}
.ct-sec-k:first-child {
  margin-top: 0;
}

.ct-kv {
  display: grid;
  gap: 5px;
  margin: 0;
}
.ct-kv > div {
  display: grid;
  grid-template-columns: 96px 1fr;
  gap: 10px;
  font-size: 11.5px;
}
.ct-kv dt {
  color: var(--text-3);
}
.ct-kv dd {
  margin: 0;
  color: var(--text-2);
  word-break: break-all;
}

.ct-res-row {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-bottom: 6px;
  font-size: 11px;
  color: var(--text-2);
}
/* 抽屉里的资源进度条。**必须叫 ct-res-bar**：原先复用了 `.ct-bar`，
 * 而工具条也叫 `.ct-bar`——CSS 是层叠的，两条规则合并到同一个类上，
 * 工具条于是吃到了 `height: 4px` / `background: var(--line)` / `overflow: hidden`，
 * 整条变成一条灰蓝色块，把搜索框和按钮顶得七零八落。 */
.ct-res-bar {
  flex: 1;
  height: 4px;
  border-radius: 3px;
  background: var(--line);
  overflow: hidden;
}
.ct-res-bar i {
  display: block;
  height: 100%;
  background: var(--mint);
}
.ct-res-bar i.is-warn {
  background: #fbbf24;
}
.ct-res-bar i.is-bad {
  background: var(--bad-text);
}

.ct-follow {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-bottom: 8px;
  font-size: 11px;
  color: var(--text-2);
  cursor: pointer;
}
.ct-logs {
  margin: 0;
  padding: 10px 12px;
  max-height: 480px;
  overflow: auto;
  border: 1px solid var(--line);
  border-radius: 8px;
  background: rgba(0, 0, 0, 0.28);
  font-size: 11px;
  line-height: 1.6;
  white-space: pre-wrap;
  word-break: break-all;
}

/* 确认框 */
.ct-modal {
  width: 420px;
  max-width: 92vw;
  padding: 16px;
  border: 1px solid var(--line-strong);
  border-radius: 12px;
  background: var(--card-solid);
}
.ct-modal-title {
  font-size: 13.5px;
  font-weight: 600;
  margin-bottom: 10px;
}
.ct-modal-body {
  font-size: 11.5px;
  line-height: 1.7;
  color: var(--text-2);
}
.ct-check {
  display: flex;
  align-items: flex-start;
  gap: 8px;
  margin-top: 10px;
  padding: 8px 10px;
  border: 1px solid var(--line);
  border-radius: 8px;
  cursor: pointer;
}
.ct-check input {
  margin-top: 2px;
}
.ct-warn {
  margin-top: 10px;
  padding: 8px 10px;
  border-radius: 8px;
  background: rgba(248, 113, 113, 0.12);
  color: var(--bad-text);
  font-size: 11px;
}
.ct-modal-foot {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 14px;
}
</style>

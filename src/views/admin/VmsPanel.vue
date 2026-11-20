<script setup lang="ts">
// 虚拟机管理面板：列表（两档密度）+ 右侧抽屉（详情 / 磁盘 / 网络 / 串口控制台）
// + 「⋯」菜单与确认框。
//
// 只做呈现与调度：virsh 脚本、解析、动作白名单、状态机全在核心
// （tl-core 的 server_admin::vms 与 business::admin）。前端再写一套规则，
// 就会出现「菜单能点、后端 409」这种两边不一致的毛病。

import {
  computed,
  defineAsyncComponent,
  nextTick,
  onMounted,
  onUnmounted,
  reactive,
  ref,
  watch,
} from 'vue';
import RingGauge from '../../components/RingGauge.vue';
import {
  allowedActions,
  dataDisks,
  diskTotalBytes,
  formatKib,
  memoryPercent,
  splitKib,
  stateLabel,
  stateTone,
  useVmsStore,
  VM_ACTION_LABEL,
} from '../../models/vms';
import { formatRate } from '../../models/traffic';
import type { VmAction, VmItem } from '../../api/types';

// xterm 约 74 kB（gzip）：只有真的打开「串口控制台」页签才该下载它。
const VmConsole = defineAsyncComponent(() => import('./VmConsole.vue'));

const props = defineProps<{ serverId: string }>();

const store = useVmsStore();
const tab = ref<'detail' | 'disks' | 'nets' | 'console'>('detail');

/** 抽屉当前对着哪台（按名字定位：动作与详情接口用的也是名字）。 */
const current = ref<VmItem | null>(null);

/** 「⋯」菜单：位置按打开那一刻算，fixed 定位避免被列表容器裁掉。 */
const menu = reactive<{ open: boolean; item: VmItem | null; left: number; top: number }>({
  open: false,
  item: null,
  left: 0,
  top: 0,
});
const menuEl = ref<HTMLElement | null>(null);

/** 确认框：只有优雅关机与强制断电需要，其余动作点了就执行。 */
const confirm = reactive<{
  open: boolean;
  action: 'shutdown' | 'destroy' | null;
  item: VmItem | null;
  busy: boolean;
}>({ open: false, action: null, item: null, busy: false });
/** 强制断电必须勾选这一条才能点「断电」。 */
const confirmed = ref(false);

const countsText = computed(() => {
  const c = store.counts;
  const parts = [`${c.total} 台`];
  if (c.running) parts.push(`${c.running} 运行`);
  if (c.paused) parts.push(`${c.paused} 暂停`);
  if (c.shutoff) parts.push(`${c.shutoff} 关机`);
  if (c.crashed) parts.push(`${c.crashed} 崩溃`);
  return parts.join(' · ');
});

const engineText = computed(() =>
  store.list?.engine ? `libvirt ${store.list.engine}` : '未检测到 libvirt',
);

/** 抽屉里的数据：详情还没回来时先用列表里那份，别让面板空着。 */
const shown = computed<VmItem | null>(() => store.detail?.item ?? current.value);

/* ── 轮询 ──────────────────────────────────────────────── */

let listTimer: number | undefined;
let detailTimer: number | undefined;

/** 窗口不可见时不采：后台标签页每 4 秒刷一次远端 virsh 没有意义。 */
function shouldPoll(): boolean {
  return Boolean(props.serverId) && !document.hidden;
}

function startList(): void {
  window.clearInterval(listTimer);
  listTimer = window.setInterval(() => {
    if (!shouldPoll()) return;
    void store.load(props.serverId);
  }, 4000);
}

function startDetail(): void {
  stopDetail();
  detailTimer = window.setInterval(() => {
    if (shouldPoll() && current.value) void store.loadDetail(current.value.name);
  }, 6000);
}

function stopDetail(): void {
  window.clearInterval(detailTimer);
  detailTimer = undefined;
}

/* ── 交互 ──────────────────────────────────────────────── */

function openDrawer(item: VmItem, next: 'detail' | 'disks' | 'nets' | 'console' = 'detail'): void {
  closeMenu();
  current.value = item;
  tab.value = next;
  void store.loadDetail(item.name);
  startDetail();
}

function closeDrawer(): void {
  current.value = null;
  store.closeDetail();
  stopDetail();
}

function changeTab(next: 'detail' | 'disks' | 'nets' | 'console'): void {
  tab.value = next;
}

function isMenuRow(item: VmItem): boolean {
  return menu.open && menu.item?.name === item.name;
}

async function openMenu(event: MouseEvent, item: VmItem): Promise<void> {
  const el = event.currentTarget as HTMLElement;
  const rect = el.getBoundingClientRect();
  menu.item = item;
  menu.open = true;

  // 先渲染再量真实高度：写死估值会在最后一行打开时算出「下方放得下」，
  // 菜单直接溢出到屏幕外。
  await nextTick();
  const width = menuEl.value?.offsetWidth ?? 172;
  const height = menuEl.value?.offsetHeight ?? 260;
  const below = rect.bottom + 6;

  menu.left = Math.max(12, Math.min(window.innerWidth - width - 12, rect.right - width));
  menu.top = below + height > window.innerHeight - 12 ? Math.max(12, rect.top - height - 6) : below;
}

function closeMenu(): void {
  menu.open = false;
  menu.item = null;
}

/** 菜单项：先按状态白名单，再补控制台 / 自启 / 详情。 */
function menuActions(item: VmItem): VmAction[] {
  return allowedActions(item.state);
}

/**
 * 串口只在域跑着时给：paused 是 qemu 被冻住，`virsh console` 连上去只会挂在那里，
 * 没有输出也没有提示——给这个入口等于给一个死胡同。
 */
function canConsole(item: VmItem): boolean {
  return item.state === 'running';
}

async function run(item: VmItem, action: VmAction): Promise<void> {
  closeMenu();
  // 关机是异步的、断电可能丢数据：这两项先问一句。
  if (action === 'shutdown' || action === 'destroy') {
    confirm.open = true;
    confirm.action = action;
    confirm.item = item;
    confirm.busy = false;
    confirmed.value = false;
    return;
  }
  await store.runAction(item, action);
}

async function runConfirm(): Promise<void> {
  const item = confirm.item;
  const action = confirm.action;
  if (!item || !action) return;
  confirm.busy = true;
  const ok = await store.runAction(item, action);
  confirm.busy = false;
  confirm.open = false;
  if (ok && tab.value === 'console' && action === 'shutdown') closeDrawer();
}

async function toggleAutostart(item: VmItem): Promise<void> {
  closeMenu();
  await store.setAutostart(item, !item.autostart);
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

/** 列表刷新后，抽屉里那份跟着换（动作改的是同一个域）。 */
watch(
  () => store.list,
  () => {
    if (!current.value) return;
    const fresh = store.items.find((item) => item.name === current.value?.name);
    if (fresh) current.value = fresh;
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
  stopDetail();
  window.clearTimeout(flashTimer);
});

/* ── 展示口径 ──────────────────────────────────────────── */

function dotClass(item: VmItem): string {
  const tone = stateTone(item.state);
  return tone === 'ok' ? 'is-ok' : tone === 'warn' ? 'is-warn' : tone === 'bad' ? 'is-bad' : 'is-idle';
}

function statusClass(item: VmItem): string {
  const tone = stateTone(item.state);
  return tone === 'warn' ? 'is-warn' : tone === 'bad' ? 'is-bad' : '';
}

/** 环里的 CPU：null = 还没第二拍，显示 `—` 而不是 0%。 */
function cpuValue(item: VmItem): number {
  return store.metricFor(item.name).cpu ?? 0;
}

function cpuText(item: VmItem): string {
  const value = store.metricFor(item.name).cpu;
  return item.state === 'running' && value != null ? `${Math.round(value)}%` : '—';
}

function memValue(item: VmItem): number {
  return memoryPercent(item);
}

/**
 * 展示用的内存实占。
 *
 * 核心给的是 qemu 进程的 RSS（`balloon.rss`），含 qemu 自身开销：4 GB 的域
 * 实测到 4.2 GB 很常见。界面上写「4.2 / 4.0 GB」会被当成算错，所以超过分配量
 * 时按分配量封顶；原始值仍留在悬停提示里。
 */
function memUsedKib(item: VmItem): number {
  if (!item.memory_used_kib) return 0;
  return item.memory_kib
    ? Math.min(item.memory_used_kib, item.memory_kib)
    : item.memory_used_kib;
}

function memNumber(item: VmItem): string {
  const used = memUsedKib(item);
  return used ? splitKib(used)[0] : '—';
}

function memUnit(item: VmItem): string {
  const used = memUsedKib(item);
  return used ? splitKib(used)[1] : '';
}

/** 详细视图的 MEM 行：用量 + 占分配的比例。 */
function memLine(item: VmItem): string {
  const used = memUsedKib(item);
  if (!used || !item.memory_kib) return '—';
  const pct = memoryPercent(item);
  return `${formatKib(used)} · ${pct < 0.1 ? '<0.1' : pct.toFixed(0)}%`;
}

/** 与环形同一套阈值：≥70 黄、≥90 红。 */
function memTone(item: VmItem): string {
  const pct = memoryPercent(item);
  return pct >= 90 ? 'is-bad' : pct >= 70 ? 'is-warn' : '';
}

function cpuLine(item: VmItem): string {
  const value = store.metricFor(item.name).cpu;
  return item.state === 'running' && value != null ? `${value.toFixed(1)}%` : '—';
}

/** 网速：速率是 4 秒平均，第一拍没有上一拍可比，显示 `—`。 */
function netLine(item: VmItem): string {
  // 关机的域网卡计数不会动，写「空闲」会被读成「在跑但没流量」。
  if (item.state !== 'running' && item.state !== 'paused') return '—';
  const metric = store.metricFor(item.name);
  if (!metric.seconds) return '—';
  const rx = formatRate(metric.rx);
  const tx = formatRate(metric.tx);
  if (!rx && !tx) return '空闲';
  return [rx ? `↓ ${rx}` : '', tx ? `↑ ${tx}` : ''].filter(Boolean).join(' ');
}

function netTitle(item: VmItem): string {
  const metric = store.metricFor(item.name);
  if (!metric.seconds) return '需要两次采样才能算速率（列表 4 秒一档）';
  return `最近 ${metric.seconds.toFixed(1)} 秒的平均收发`;
}

/** 磁盘列：`vda 120 GB · qcow2`，最多两行，多了写 `+N`。 */
function diskLines(item: VmItem): string[] {
  return dataDisks(item)
    .slice(0, 2)
    .map((disk) => {
      const size = disk.capacity_bytes ? formatKib(disk.capacity_bytes / 1024) : '—';
      return `${disk.target} ${size}${disk.format ? ` · ${disk.format}` : ''}`;
    });
}

function diskSummary(item: VmItem): string {
  const count = dataDisks(item).length;
  if (!count) return '无磁盘';
  return `${count} 块 · ${formatKib(diskTotalBytes(item) / 1024)}`;
}

function cdromCount(item: VmItem): number {
  return item.disks.length - dataDisks(item).length;
}

function netSource(net: { source: string; kind: string }): string {
  return net.source || net.kind || '—';
}

/** IP 没有租约时写黄字，而不是留白：留白会被当成「没采到」。 */
function netIp(net: { ip: string }): string {
  return net.ip || '无租约';
}

function shortId(item: VmItem): string {
  const id = item.id != null ? `#${item.id}` : '未运行';
  const uuid = item.uuid.slice(0, 8);
  return uuid ? `${id} · ${uuid}` : id;
}

function confirmTitle(): string {
  return confirm.action === 'destroy' ? '强制断电' : '优雅关机';
}

function confirmDanger(): boolean {
  return confirm.action === 'destroy';
}

/** 关机按钮：断电必须勾选；优雅关机不必。 */
function confirmDisabled(): boolean {
  return confirm.busy || (confirm.action === 'destroy' && !confirmed.value);
}
</script>

<template>
  <div class="vm">
    <!-- 工具条：引擎与统计在左，搜索与两档切换在右 -->
    <div class="vm-bar">
      <span class="vm-engine mono">{{ engineText }}</span>
      <span class="vm-counts">{{ countsText }}</span>
      <span class="vm-grow" />
      <input v-model="store.query" class="vm-search" placeholder="搜索域名 / UUID / IP" />
      <div class="seg">
        <button :class="{ 'is-on': store.filter === 'all' }" @click="store.filter = 'all'">全部</button>
        <button :class="{ 'is-on': store.filter === 'running' }" @click="store.filter = 'running'">
          运行中
        </button>
        <button :class="{ 'is-on': store.filter === 'stopped' }" @click="store.filter = 'stopped'">
          已关机
        </button>
      </div>
      <!-- 密度只影响呈现，不触发任何请求 -->
      <div class="seg" title="主要：状态 · 名称 · 规格 · 磁盘 · 资源环；详细：再加网络与运行细节">
        <button :class="{ 'is-on': store.density === 'compact' }" @click="store.setDensity('compact')">
          主要
        </button>
        <button :class="{ 'is-on': store.density === 'detail' }" @click="store.setDensity('detail')">
          详细
        </button>
      </div>
    </div>

    <div v-if="store.feedback" class="vm-flash" :class="{ 'is-bad': store.feedbackIsError }">
      {{ store.feedback }}
    </div>
    <div v-if="store.lightError" class="vm-flash is-bad">刷新失败：{{ store.lightError }}</div>

    <div v-if="store.loading" class="admin-loading">正在采集虚拟机列表…</div>

    <div v-else-if="store.error" class="admin-error">
      <div class="admin-error-title">虚拟机列表采集失败</div>
      <div class="admin-error-body">{{ store.error }}</div>
      <p class="hint">
        常见原因：当前 SSH 用户不在 <code>libvirt</code> 组（会报 failed to connect / permission
        denied）。在服务器上执行 <code>sudo usermod -aG libvirt $USER</code>，重新登录后再试。
      </p>
    </div>

    <div v-else-if="!store.hasEngine" class="admin-empty">
      <h4>这台机器上没有 libvirt</h4>
      <p>
        没检测到 <code>virsh</code>。Debian / Ubuntu 装 <code>qemu-system-x86 libvirt-daemon-system
        libvirt-clients</code>；装上之后回到这一页会自动出现虚拟机列表。
      </p>
    </div>

    <div v-else-if="!store.visible.length" class="admin-empty">
      <h4>没有匹配的虚拟机</h4>
      <p>共 {{ store.counts.total }} 台虚拟机，当前筛选与搜索条件下一个都没匹配上。</p>
    </div>

    <!-- ── 列表 ─────────────────────────────────────── -->
    <div v-else class="vm-list" :class="{ 'is-compact': store.density === 'compact' }">
      <div class="vm-thead">
        <span />
        <span>虚拟机</span>
        <span>{{ store.density === 'detail' ? '规格' : '规格 · 磁盘' }}</span>
        <span v-if="store.density === 'detail'">磁盘</span>
        <span v-if="store.density === 'detail'">网络</span>
        <span :class="{ 'is-right': store.density === 'compact' }">
          {{ store.density === 'detail' ? '资源' : 'CPU' }}
        </span>
        <span v-if="store.density === 'compact'" class="is-right">内存</span>
        <span />
      </div>

      <!-- 详细：五块，块内纵向堆叠 -->
      <template v-if="store.density === 'detail'">
        <div
          v-for="item in store.visible"
          :key="item.name"
          class="vm-row"
          :class="{ 'is-menu-open': isMenuRow(item) }"
          @click="openDrawer(item)"
        >
          <span class="vm-dot" :class="dotClass(item)" />
          <div class="vm-cell">
            <span class="vm-n">
              {{ item.name }}
              <span class="vm-chip" :class="{ 'is-on': item.autostart }">
                开机自启<b v-if="item.autostart" class="vm-ck">✓</b>
              </span>
            </span>
            <span class="vm-s" :class="statusClass(item)">{{ stateLabel(item.state) }}</span>
            <span class="vm-id mono">{{ shortId(item) }}</span>
          </div>
          <div class="vm-cell">
            <span class="vm-line"><b>{{ item.vcpu }}</b> vCPU</span>
            <span class="vm-line">内存 <b>{{ formatKib(item.memory_kib) }}</b></span>
            <span class="vm-line">
              {{ item.os_type || '—' }} · {{ item.persistent ? '持久化' : '临时域' }}
            </span>
          </div>
          <div class="vm-cell">
            <span v-for="line in diskLines(item)" :key="line" class="vm-line mono">{{ line }}</span>
            <span v-if="!diskLines(item).length" class="vm-line">无磁盘</span>
            <span v-if="cdromCount(item)" class="vm-line">光驱 {{ cdromCount(item) }} 个</span>
          </div>
          <div class="vm-cell">
            <span v-for="net in item.nets.slice(0, 2)" :key="net.target" class="vm-line mono">
              {{ netSource(net) }} · <b :class="{ 'is-warn': !net.ip }">{{ netIp(net) }}</b>
            </span>
            <span v-if="!item.nets.length" class="vm-line">没有网卡</span>
            <span v-if="item.nets.length > 2" class="vm-line">+{{ item.nets.length - 2 }} 块网卡</span>
          </div>
          <div class="vm-cell vm-res-cell" :title="netTitle(item)">
            <span class="vm-res"><i>CPU</i><b class="mono">{{ cpuLine(item) }}</b></span>
            <span class="vm-res">
              <i>MEM</i>
              <b class="mono" :class="memTone(item)">{{ memLine(item) }}</b>
            </span>
            <span class="vm-res"><i>NET</i><b class="mono">{{ netLine(item) }}</b></span>
          </div>
          <button class="vm-more" title="操作" @click.stop="openMenu($event, item)">⋯</button>
        </div>
      </template>

      <!-- 主要：单行 7 列 -->
      <template v-else>
        <div
          v-for="item in store.visible"
          :key="item.name"
          class="vm-row"
          :class="{ 'is-menu-open': isMenuRow(item) }"
          @click="openDrawer(item)"
        >
          <span class="vm-dot" :class="dotClass(item)" />
          <div class="vm-cell">
            <span class="vm-n">
              {{ item.name }}
              <span class="vm-chip" :class="{ 'is-on': item.autostart }">
                开机自启<b v-if="item.autostart" class="vm-ck">✓</b>
              </span>
            </span>
            <span class="vm-s" :class="statusClass(item)">{{ stateLabel(item.state) }}</span>
          </div>
          <div class="vm-cell">
            <span class="vm-line"><b>{{ item.vcpu }}</b> vCPU · {{ formatKib(item.memory_kib) }}</span>
            <span class="vm-line">磁盘 {{ diskSummary(item) }}</span>
          </div>
          <div class="vm-metric" :title="`CPU 占用 · ${item.vcpu} 个 vCPU 为 100%`">
            <RingGauge :value="cpuValue(item)" :size="44" :text="cpuText(item)" />
          </div>
          <div
            class="vm-metric"
            :title="`内存 ${memLine(item)}（分配 ${formatKib(item.memory_kib)}；宿主 RSS，含 qemu 开销）`"
          >
            <RingGauge
              :value="memValue(item)"
              :size="44"
              :text="memNumber(item)"
              :sub="memUnit(item)"
            />
          </div>
          <button class="vm-more" title="操作" @click.stop="openMenu($event, item)">⋯</button>
        </div>
      </template>
    </div>

    <!-- ── 「⋯」菜单 ─────────────────────────────────── -->
    <Teleport to="body">
      <div
        v-if="menu.open && menu.item"
        ref="menuEl"
        class="vm-menu"
        :style="{ left: `${menu.left}px`, top: `${menu.top}px` }"
        @click.stop
      >
        <div class="vm-menu-k">{{ menu.item.name }}</div>
        <button
          v-for="action in menuActions(menu.item)"
          :key="action"
          class="vm-menu-item"
          :class="{ 'is-danger': action === 'destroy' }"
          @click="run(menu.item, action)"
        >
          {{ VM_ACTION_LABEL[action] }}
        </button>
        <div class="vm-menu-sep" />
        <!-- 自启是状态不是动作：开着才打勾，点一下切换 -->
        <button class="vm-menu-item" @click="toggleAutostart(menu.item)">
          开机自启
          <span class="vm-menu-ck">{{ menu.item.autostart ? '✓' : '' }}</span>
        </button>
        <button
          v-if="canConsole(menu.item)"
          class="vm-menu-item"
          @click="openDrawer(menu.item, 'console')"
        >
          串口控制台
        </button>
        <button class="vm-menu-item" @click="openDrawer(menu.item, 'detail')">详情</button>
      </div>
    </Teleport>

    <!-- ── 抽屉 ─────────────────────────────────────── -->
    <Teleport to="body">
      <div v-if="current" class="vm-mask" @click.self="closeDrawer">
        <aside class="vm-drawer">
          <header class="vm-drawer-head">
            <div>
              <div class="vm-drawer-title">{{ current.name }}</div>
              <div class="vm-drawer-sub mono">{{ current.uuid }}</div>
            </div>
            <button class="icon-btn" title="关闭" @click="closeDrawer">✕</button>
          </header>

          <div class="vm-tabs">
            <button :class="{ 'is-on': tab === 'detail' }" @click="changeTab('detail')">详情</button>
            <button :class="{ 'is-on': tab === 'disks' }" @click="changeTab('disks')">磁盘</button>
            <button :class="{ 'is-on': tab === 'nets' }" @click="changeTab('nets')">网络</button>
            <button :class="{ 'is-on': tab === 'console' }" @click="changeTab('console')">
              串口控制台
            </button>
          </div>

          <div class="vm-drawer-body">
            <!-- 详情 -->
            <template v-if="tab === 'detail'">
              <div class="vm-sec-k">运行状态</div>
              <dl class="vm-kv">
                <div><dt>状态</dt><dd>{{ stateLabel(current.state) }}</dd></div>
                <div><dt>域 ID</dt><dd class="mono">{{ current.id ?? '—' }}</dd></div>
                <div><dt>域 UUID</dt><dd class="mono">{{ current.uuid || '—' }}</dd></div>
                <div><dt>开机自启</dt><dd :class="{ 'is-on': current.autostart }">{{ current.autostart ? '✓ 开启' : '关闭' }}</dd></div>
                <div><dt>持久化</dt><dd>{{ current.persistent ? '是' : '否（临时域）' }}</dd></div>
                <div><dt>OS 类型</dt><dd>{{ current.os_type || '—' }}</dd></div>
                <div v-if="store.detail"><dt>机型</dt><dd class="mono">{{ store.detail.machine || '—' }}</dd></div>
                <div v-if="store.detail"><dt>固件</dt><dd>{{ store.detail.firmware || '—' }}</dd></div>
              </dl>

              <div class="vm-sec-k">资源</div>
              <div class="vm-rings">
                <div class="vm-ring">
                  <RingGauge :value="cpuValue(current)" :size="84" :text="cpuText(current)" />
                  <div class="k">CPU 负载</div>
                  <div class="s">{{ current.vcpu }} 个 vCPU 为 100%</div>
                </div>
                <div class="vm-ring">
                  <RingGauge
                    :value="memValue(current)"
                    :size="84"
                    :text="memNumber(current)"
                    :sub="memUnit(current)"
                  />
                  <div class="k">内存占用</div>
                  <div class="s">
                    分配 {{ formatKib(current.memory_kib) }} · 已用
                    {{ current.memory_used_kib ? `${memoryPercent(current).toFixed(0)}%` : '—' }}
                    （宿主 RSS）
                  </div>
                </div>
              </div>

              <div class="vm-sec-k">配置</div>
              <dl class="vm-kv">
                <div><dt>vCPU</dt><dd>{{ current.vcpu }}</dd></div>
                <div><dt>内存分配</dt><dd>{{ formatKib(current.memory_kib) }}</dd></div>
                <div><dt>磁盘</dt><dd>{{ diskSummary(current) }}</dd></div>
                <div><dt>网卡</dt><dd>{{ current.nets.length }} 块</dd></div>
              </dl>

              <p class="hint vm-boundary">
                本模块只做只读采集与生命周期动作；创建 / 删除虚拟机、改配置、快照、迁移都不在这里做。
                串口控制台是唯一的交互入口，VNC 图形控制台不在本期。
              </p>
            </template>

            <!-- 磁盘 -->
            <template v-else-if="tab === 'disks'">
              <div v-if="!shown" class="admin-loading">正在读取磁盘…</div>
              <template v-else>
                <div v-for="disk in shown.disks" :key="disk.target" class="vm-disk">
                  <div class="vm-disk-head">
                    <span class="mono vm-disk-t">{{ disk.target }}</span>
                    <span class="vm-line">{{ disk.device === 'cdrom' ? '光驱' : '磁盘' }}</span>
                    <span class="vm-grow" />
                    <span class="mono vm-line">{{ disk.format || '—' }}</span>
                  </div>
                  <div class="vm-line mono" :title="disk.source">{{ disk.source || '（没有介质）' }}</div>
                  <div class="vm-line">
                    容量 <b>{{ disk.capacity_bytes ? formatKib(disk.capacity_bytes / 1024) : '—' }}</b>
                    · 实占 <b>{{ disk.allocation_bytes ? formatKib(disk.allocation_bytes / 1024) : '—' }}</b>
                  </div>
                </div>
                <div v-if="!shown.disks.length" class="admin-empty">
                  <h4>没有磁盘</h4>
                  <p>这台域没有挂任何块设备。</p>
                </div>
                <p class="hint">
                  qcow2 的「实占」小于「容量」是正常的：稀疏文件只在写了数据的地方分配空间，
                  删掉客户机里的文件不会让它自动缩小（要缩得在关机后 `qemu-img resize`）。
                </p>
              </template>
            </template>

            <!-- 网络 -->
            <template v-else-if="tab === 'nets'">
              <div v-for="net in current.nets" :key="net.target" class="vm-net">
                <div class="vm-disk-head">
                  <span class="mono vm-disk-t">{{ net.target }}</span>
                  <span class="vm-line">{{ net.kind }}</span>
                  <span class="vm-grow" />
                  <span class="mono vm-line">{{ net.model || '—' }}</span>
                </div>
                <div class="vm-line mono">桥接/网络 <b>{{ netSource(net) }}</b></div>
                <div class="vm-line mono">MAC <b>{{ net.mac || '—' }}</b></div>
                <div class="vm-line">
                  IP
                  <b :class="{ 'is-warn': !net.ip }">{{ netIp(net) }}</b>
                  <span class="vm-muted">（来自 DHCP 租约，静态 IP 与 guest agent 未接入）</span>
                </div>
              </div>
              <div v-if="!current.nets.length" class="admin-empty">
                <h4>没有网卡</h4>
                <p>这台域没有挂任何网络接口。</p>
              </div>
              <div v-if="store.detail?.display" class="vm-kv vm-display">
                <div><dt>图形显示</dt><dd class="mono">{{ store.detail.display }}</dd></div>
              </div>
              <p class="hint">
                这里只列网卡与地址，不做端口转发或防火墙。VNC 图形控制台不在本期
                （要另做一个 RFB 客户端）；需要图形界面时用宿主上的 virt-manager 或 VNC 客户端。
              </p>
            </template>

            <!-- 串口控制台 -->
            <template v-else>
              <VmConsole
                v-if="current"
                :server-id="props.serverId"
                :name="current.name"
                :state="current.state"
              />
            </template>
          </div>
        </aside>
      </div>
    </Teleport>

    <!-- ── 确认框 ───────────────────────────────────── -->
    <Teleport to="body">
      <div v-if="confirm.open && confirm.item" class="vm-mask is-center">
        <div class="vm-modal">
          <div class="vm-modal-title">{{ confirmTitle() }} · {{ confirm.item.name }}</div>
          <div class="vm-modal-body">
            <template v-if="confirm.action === 'shutdown'">
              向客户机发送 ACPI 关机请求（相当于按一下电源键）。装了 <code>acpid</code> /
              <code>qemu-guest-agent</code> 的系统会正常关机，没装的会毫无反应。
            </template>
            <template v-else>
              相当于直接拔电源：qemu 进程立刻结束，客户机里未落盘的数据可能丢失、
              文件系统可能损坏。只有在优雅关机无效时才用它。
            </template>
            <div v-if="confirm.action === 'shutdown'" class="vm-modal-note">
              发送后 60 秒还没关机，可以再回来用「强制断电」。
            </div>
          </div>

          <label v-if="confirmDanger()" class="vm-check">
            <input v-model="confirmed" type="checkbox" />
            <span>我知道这会中断客户机，可能损坏未落盘的数据</span>
          </label>

          <div class="vm-modal-foot">
            <button class="btn" @click="confirm.open = false">取消</button>
            <button
              class="btn is-primary"
              :class="{ 'is-danger': confirmDanger() }"
              :disabled="confirmDisabled()"
              @click="runConfirm"
            >
              {{ confirm.busy ? '执行中…' : confirmDanger() ? '强制断电' : '发送关机请求' }}
            </button>
          </div>
        </div>
      </div>
    </Teleport>
  </div>
</template>

<style scoped>
.vm {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

/* 工具条 */
.vm-bar {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
}
.vm-engine {
  font-size: 11px;
  color: var(--text-2);
}
.vm-counts {
  font-size: 11px;
  color: var(--text-3);
}
.vm-grow {
  flex: 1;
}
.vm-search {
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
.vm-search:focus {
  outline: none;
  border-color: var(--line-strong);
}

.vm-flash {
  padding: 7px 10px;
  border: 1px solid var(--line-strong);
  border-radius: 8px;
  font-size: 11.5px;
  color: var(--text-2);
}
.vm-flash.is-bad {
  border-color: var(--bad-text);
  color: var(--bad-text);
}

/* 列表 */
.vm-list {
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.vm-thead,
.vm-row {
  display: grid;
  /* 五块：身份 / 规格 / 磁盘 / 网络 / 资源。资源列给 128px：
     `↓103.0 MB/s` 这类值最长也就这么宽。 */
  grid-template-columns:
    16px minmax(170px, 1.1fr) minmax(120px, 0.75fr) minmax(130px, 0.85fr)
    minmax(140px, 1fr) 176px 30px;
  gap: 10px;
  align-items: start;
}
.vm-thead {
  padding: 0 12px 2px;
  font-size: 10px;
  font-weight: 600;
  letter-spacing: 1.1px;
  color: var(--text-2);
}
.vm-row {
  padding: 10px 12px;
  border: 1px solid var(--line);
  border-radius: 10px;
  background: var(--card);
  cursor: pointer;
}
.vm-row:hover {
  border-color: var(--line-strong);
}
/* 菜单浮在下面几行上面，行本身不标出来，就对不上这个菜单是哪一行的 */
.vm-row.is-menu-open {
  border-color: var(--line-strong);
  box-shadow: 0 0 0 1px var(--line-strong);
}
/* 主要视图的环是右对齐的，表头不跟着右对齐就会和环错开半个字 */
.vm-thead .is-right {
  text-align: right;
}

.vm-cell {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

.vm-dot {
  width: 7px;
  height: 7px;
  margin-top: 5px;
  border-radius: 50%;
  background: var(--text-3);
}
.vm-dot.is-ok {
  background: var(--mint);
}
.vm-dot.is-warn {
  background: #fbbf24;
}
.vm-dot.is-bad {
  background: var(--bad-text);
}

.vm-n {
  display: flex;
  align-items: center;
  gap: 5px;
  min-width: 0;
  font-size: 12.5px;
  font-weight: 600;
  overflow: hidden;
  white-space: nowrap;
}
.vm-s {
  font-size: 10.5px;
  color: var(--text-3);
}
.vm-s.is-warn {
  color: #fbbf24;
}
.vm-s.is-bad {
  color: var(--bad-text);
}
.vm-id {
  font-size: 9.5px;
  color: var(--text-3);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* 开机自启：文字常在，勾只在开着时出现 */
.vm-chip {
  display: inline-flex;
  align-items: center;
  flex-shrink: 0;
  height: 16px;
  padding: 0 5px;
  border: 1px solid var(--line-strong);
  border-radius: 4px;
  font-size: 9.5px;
  font-weight: 400;
  letter-spacing: 0.2px;
  color: var(--text-3);
}
.vm-chip.is-on {
  border-color: rgba(52, 211, 153, 0.35);
  background: var(--mint-soft);
  color: var(--mint);
}
.vm-ck {
  margin-left: 3px;
  font-size: 11px;
  font-weight: 600;
  line-height: 1;
}

.vm-line {
  font-size: 10.5px;
  color: var(--text-3);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.vm-line b {
  color: var(--text-2);
  font-weight: 500;
}
.vm-line b.is-warn {
  color: #fbbf24;
}
.vm-line b.is-bad {
  color: var(--bad-text);
}
.vm-muted {
  color: var(--text-3);
  opacity: 0.85;
}

.vm-res-cell {
  gap: 2px;
}
.vm-res {
  display: grid;
  grid-template-columns: 26px 1fr;
  gap: 6px;
  align-items: baseline;
}
.vm-res i {
  font-size: 9px;
  font-style: normal;
  letter-spacing: 0.5px;
  color: var(--text-3);
}
.vm-res b {
  font-size: 10.5px;
  font-weight: 500;
  color: var(--text-2);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.vm-more {
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
.vm-more:hover {
  background: var(--card);
  border-color: var(--line-strong);
  color: var(--text);
}

/* 主要视图：单行 7 列，资源列放环 */
.vm-list.is-compact .vm-thead,
.vm-list.is-compact .vm-row {
  grid-template-columns: 16px minmax(180px, 1.25fr) minmax(140px, 0.9fr) 64px 96px 30px;
  align-items: center;
}
.vm-list.is-compact .vm-row {
  padding: 8px 12px;
}
.vm-metric {
  display: flex;
  justify-content: flex-end;
  min-width: 0;
}

/* 菜单 */
.vm-menu {
  position: fixed;
  z-index: 40;
  min-width: 172px;
  padding: 5px;
  border: 1px solid var(--line-strong);
  border-radius: 10px;
  /* `--card-solid` 是每个主题都定义好、且不透明的面板底色：
     用写死的深色会在浅色主题下变成「深底 + 深字」。 */
  background: var(--card-solid);
  box-shadow: 0 16px 40px rgba(0, 0, 0, 0.28);
}
.vm-menu-k {
  padding: 4px 9px 7px;
  max-width: 190px;
  font-size: 11px;
  font-weight: 600;
  letter-spacing: 0.3px;
  color: var(--text-2);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.vm-menu-item {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  padding: 7px 9px;
  border: 0;
  border-radius: 7px;
  background: transparent;
  color: var(--text);
  font-size: 12px;
  text-align: left;
  cursor: pointer;
}
.vm-menu-item:hover {
  background: var(--tint-strong);
}
.vm-menu-item.is-danger {
  color: var(--bad-text);
}
.vm-menu-item.is-danger:hover {
  background: rgba(248, 113, 113, 0.14);
}
.vm-menu-ck {
  margin-left: auto;
  min-width: 10px;
  font-size: 12px;
  line-height: 1;
  color: var(--mint);
  text-align: right;
}
.vm-menu-sep {
  height: 1px;
  margin: 6px 4px;
  background: var(--line-strong);
}

/* 抽屉 */
.vm-mask {
  position: fixed;
  inset: 0;
  z-index: 45;
  display: flex;
  justify-content: flex-end;
  background: var(--mask);
}
.vm-mask.is-center {
  align-items: center;
  justify-content: center;
}
.vm-drawer {
  display: flex;
  flex-direction: column;
  width: 460px;
  max-width: 92vw;
  height: 100%;
  border-left: 1px solid var(--line-strong);
  background: var(--card-solid);
}
.vm-drawer-head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 10px;
  padding: 14px 16px 10px;
  border-bottom: 1px solid var(--line);
}
.vm-drawer-title {
  font-size: 14px;
  font-weight: 600;
}
.vm-drawer-sub {
  margin-top: 3px;
  font-size: 10.5px;
  color: var(--text-3);
  word-break: break-all;
}
.vm-tabs {
  display: flex;
  gap: 4px;
  padding: 8px 12px;
  border-bottom: 1px solid var(--line);
}
.vm-tabs button {
  padding: 5px 10px;
  border: 1px solid transparent;
  border-radius: 7px;
  background: transparent;
  color: var(--text-3);
  font-size: 11.5px;
  cursor: pointer;
}
.vm-tabs button.is-on {
  border-color: var(--line-strong);
  background: var(--card);
  color: var(--text);
}
.vm-drawer-body {
  flex: 1;
  overflow-y: auto;
  padding: 12px 16px 20px;
}

.vm-sec-k {
  margin: 14px 0 7px;
  font-size: 10px;
  font-weight: 600;
  letter-spacing: 1px;
  color: var(--text-3);
}
.vm-sec-k:first-child {
  margin-top: 0;
}

.vm-kv {
  display: grid;
  gap: 5px;
  margin: 0;
}
.vm-kv > div {
  display: grid;
  grid-template-columns: 96px 1fr;
  gap: 10px;
  font-size: 11.5px;
}
.vm-kv dt {
  color: var(--text-3);
}
.vm-kv dd {
  margin: 0;
  color: var(--text-2);
  word-break: break-all;
}
.vm-kv dd.is-on {
  color: var(--mint);
}

/* 抽屉里的资源环：并排两个，环下写名称与口径 */
.vm-rings {
  display: flex;
  gap: 22px;
  margin-top: 4px;
}
.vm-ring {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 6px;
}
.vm-ring .k {
  font-size: 11px;
  color: var(--text-2);
}
.vm-ring .s {
  font-size: 10px;
  color: var(--text-3);
  text-align: center;
}

.vm-disk,
.vm-net {
  display: flex;
  flex-direction: column;
  gap: 3px;
  padding: 9px 10px;
  margin-bottom: 8px;
  border: 1px solid var(--line);
  border-radius: 8px;
}
.vm-disk-head {
  display: flex;
  align-items: center;
  gap: 8px;
}
.vm-disk-t {
  font-size: 11.5px;
  font-weight: 600;
  color: var(--text);
}

.vm-display {
  margin-top: 10px;
}

.vm-boundary {
  margin-top: 14px;
}

/* 确认框 */
.vm-modal {
  width: 420px;
  max-width: 92vw;
  padding: 16px;
  border: 1px solid var(--line-strong);
  border-radius: 12px;
  background: var(--card-solid);
}
.vm-modal-title {
  margin-bottom: 10px;
  font-size: 13.5px;
  font-weight: 600;
}
.vm-modal-body {
  font-size: 11.5px;
  line-height: 1.7;
  color: var(--text-2);
}
.vm-modal-note {
  margin-top: 8px;
  padding: 7px 9px;
  border-radius: 7px;
  background: var(--tint);
  font-size: 10.5px;
  color: var(--text-3);
}
.vm-check {
  display: flex;
  align-items: flex-start;
  gap: 8px;
  margin-top: 10px;
  padding: 8px 10px;
  border: 1px solid var(--line);
  border-radius: 8px;
  font-size: 11px;
  color: var(--text-2);
  cursor: pointer;
}
.vm-check input {
  margin-top: 2px;
}
.vm-modal-foot {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 14px;
}
</style>

<script setup lang="ts">
// 服务器管理页：从隧道卡片进来，左侧是模块导航，右侧是当前模块。
//
// 边界：这一页**只展示核心采到的数据**，自己不调 fetch、不算指标口径；
// 未实现的模块只显示"规划中 + 将做什么"，不放半成品按钮。

import { computed, onMounted, onUnmounted, ref, watch } from 'vue';
import { onWindowFocusChange } from '../api/native';
import RingGauge from '../components/RingGauge.vue';
import Sparkline from '../components/Sparkline.vue';
import { useServersStore } from '../models/servers';
import {
  clockText,
  formatKb,
  formatUptime,
  rateText,
  usagePercent,
  useServerAdminStore,
} from '../models/serverAdmin';

const props = defineProps<{ serverId: string }>();
const emit = defineEmits<{ (e: 'back'): void }>();

const servers = useServersStore();
const admin = useServerAdminStore();

const active = ref('overview');

const server = computed(() => servers.servers.find((s) => s.id === props.serverId) ?? null);
const overview = computed(() => admin.overview);
const activeModule = computed(() => admin.modules.find((m) => m.id === active.value) ?? null);

/** 内存占比：用「已用 / 总量」，与核心给的 used_kb 口径一致。 */
const memoryPercent = computed(() =>
  overview.value ? usagePercent(overview.value.memory.used_kb, overview.value.memory.total_kb) : 0,
);

/**
 * 内存构成：把"已用"再拆成系统 / VM / 容器，缓存里分出 ZFS ARC，最后补上"空闲"。
 *
 * 口径很关键：核心给的 used_kb 是 `total − MemAvailable`（**不可回收**），
 * 而 MemAvailable 里含可回收的 cached/buffers。所以分配顺序是：
 *   1. 从 used 里先扣 VM（qemu 进程 RSS）与容器（cgroup memory.current）——它们确实占着；
 *   2. 扣完剩下的算"系统"；
 *   3. 再把 ZFS ARC 与 cached/buffers 依次放进剩余空间；
 *   4. 最后剩下的就是空闲——环上画不出来，但图例要报出来，否则 87% 之外那 13% 没有名字。
 * 每一步都 `min` 到剩余量，保证各段加起来不超过一圈——直接相加在 Linux 上经常超 100%。
 */
interface MemPart {
  key: string;
  label: string;
  kb: number;
  /** 占整机内存的百分比。 */
  value: number;
  color: string;
}

const memParts = computed<MemPart[]>(() => {
  const mem = overview.value?.memory;
  if (!mem || !mem.total_kb) {
    return [];
  }
  const total = mem.total_kb;
  const extra = overview.value?.mem_breakdown ?? { vm_kb: 0, container_kb: 0, zfs_arc_kb: 0 };

  let left = Math.min(mem.used_kb, total);
  const vm = Math.min(extra.vm_kb, left);
  left -= vm;
  const container = Math.min(extra.container_kb, left);
  left -= container;
  // ZFS ARC 在 Linux 上算在 used 里（`MemAvailable` 不含它），所以从 used 里扣，
  // 不能和 used 相加——否则环会画到 96% 而实际只用了 80%。
  const zfs = Math.min(extra.zfs_arc_kb, left);
  left -= zfs;
  const system = left;

  // 缓存含 Buffers（它本来就是缓存的一部分），再减掉 Shmem：
  // tmpfs 那部分删不掉也回收不了，留着会把"能腾多少"虚报大。
  const reclaimable = Math.max(0, mem.cached_kb + mem.buffers_kb - (mem.shmem_kb ?? 0));
  const cached = Math.min(reclaimable, total - system - vm - container - zfs);
  const free = Math.max(0, total - system - vm - container - zfs - cached);

  const part = (key: string, label: string, kb: number, color: string): MemPart => ({
    key,
    label,
    kb,
    value: (kb / total) * 100,
    color,
  });
  return [
    part('system', '系统', system, 'var(--mint)'),
    part('container', '容器', container, '#22d3ee'),
    part('vm', 'VM', vm, '#fb923c'),
    part('zfs', 'ZFS', zfs, '#a78bfa'),
    // 缓存用低饱和灰蓝：它不是"被占用的"，而是"随时能吐出来的"，
    // 用亮蓝会和系统/容器抢注意力，看环时会以为它也占着内存
    part('cached', '缓存', cached, 'rgba(148, 163, 184, 0.75)'),
    // 空闲 = 一点没被碰过的那部分；它和"可用"不是一个口径，见下
    part('free', '空闲', free, 'var(--text-3)'),
    // 可用 = MemAvailable（空闲 + 可回收）。它是内核对"还能要多少"的估算，
    // 与空闲/缓存有重叠，所以只放在图例里当参考，不画进环、不参与求和。
    part('available', '可用', mem.available_kb, 'var(--text-2)'),
  ];
});

/** 环上真正要画的段：去掉"空闲"（空弧就是它）与"可用"，也去掉小到看不见的。 */
const memSegments = computed(() =>
  memParts.value.filter(
    (part) => !['free', 'available'].includes(part.key) && part.value >= 0.3,
  ),
);

/*
 * 图例分两栏：左边是**正在占着的**（系统 / 容器 / VM / ZFS），
 * 右边是**还能腾出来的**（缓存 / 空闲 / 可用），缓存里已含 Buffers。
 * 内存紧不紧只看左栏；右栏是"理论上能还回来多少"。
 */
const MEM_USED_KEYS = ['system', 'container', 'vm', 'zfs'];
const MEM_OTHER_KEYS = ['cached', 'free', 'available'];

const memUsedParts = computed(() =>
  memParts.value.filter((part) => MEM_USED_KEYS.includes(part.key)),
);
const memOtherParts = computed(() =>
  memParts.value.filter((part) => MEM_OTHER_KEYS.includes(part.key)),
);

/*
 * 刷新节奏只分两档（没有开关，进页面就开始）：
 *   live   1s    CPU / 内存 / 网络 / GPU / 磁盘 IO —— 全是读 /proc，几乎不花 CPU
 *   disks  10s   磁盘容量 + 内存分源 —— `df` 会碰每个挂载点、`ps` 要扫进程表
 *
 * 为什么容量不跟着 1 秒：不是为了省 CPU（实测 61ms），而是 `df` 会去问**每个挂载点**，
 * 挂了 NFS/SMB 的机器上等于每秒朝远端查一次容量，网络盘没响应时整条命令会卡到超时。
 * 容量本身也几分钟才变一次，1 秒刷新只是让数字在抖。
 */
const LIVE_MS = 1000;
const DISK_MS = 10_000;

/**
 * 窗口有没有焦点。
 *
 * `document.hidden` 在 Tauri 里不总可靠：热键收起面板只是把窗口藏起来，
 * WebView 可能仍然认为自己可见。加上这一条才是"用户真的在看"。
 */
const windowFocused = ref(true);
let unlistenFocus: (() => void) | undefined;

let liveTimer: number | undefined;
let diskTimer: number | undefined;

/** 这一刻该不该采：有隧道 + 面板在前台 + 停在概览模块。 */
function watching(): boolean {
  return !!props.serverId && windowFocused.value && !document.hidden && active.value === 'overview';
}

function stopTimers() {
  window.clearTimeout(liveTimer);
  window.clearTimeout(diskTimer);
  liveTimer = undefined;
  diskTimer = undefined;
}

/*
 * 每档都用递归 `setTimeout`：远端慢的时候定时器不会叠起来，永远只有一条命令在飞。
 */

function scheduleLive(delay: number) {
  window.clearTimeout(liveTimer);
  liveTimer = undefined;
  if (watching()) {
    liveTimer = window.setTimeout(pollLive, delay);
  }
}

function scheduleDisks(delay: number) {
  window.clearTimeout(diskTimer);
  diskTimer = undefined;
  if (watching()) {
    diskTimer = window.setTimeout(pollDisks, delay);
  }
}

async function pollLive() {
  liveTimer = undefined;
  if (!watching()) {
    return;
  }
  // 失败退到 3s：会话刚断时每秒重试只会刷出同样的一串错误。
  const ok = await admin.load(props.serverId, 'live');
  recordLive();
  scheduleLive(ok ? LIVE_MS : 3000);
}

async function pollDisks() {
  diskTimer = undefined;
  if (!watching()) {
    return;
  }
  const ok = await admin.load(props.serverId, 'disks');
  scheduleDisks(ok ? DISK_MS : DISK_MS * 3);
}

/** 记一拍：CPU / 内存 / GPU 一条线，每张网卡、每块盘各一条。 */
function recordLive() {
  const current = admin.overview;
  if (!current) {
    return;
  }
  const at = Date.now();
  liveHistory.value = trimHistory([
    ...liveHistory.value,
    {
      at,
      cpu: current.cpu.usage_percent,
      mem: memoryPercent.value,
      gpu: current.gpus[0]?.usage_percent ?? 0,
    },
  ]);

  // 每张网卡各记一条：上行 + 下行，看的是这张卡有多忙
  let nics = nicHistory.value;
  for (const nic of current.network) {
    nics = pushPoint(nics, nic.name, nic.tx_rate + nic.rx_rate, at);
  }
  nicHistory.value = nics;

  // 磁盘 IO 由同一拍带回来：一行一条曲线，没有"整机汇总"这种东西
  let disks = diskHistory.value;
  for (const io of current.disk_io) {
    disks = pushPoint(disks, io.device, io.read_rate + io.write_rate, at);
  }
  diskHistory.value = disks;
}

/** 重新起两档节奏；`now` 为真时立刻各补一拍（回到前台、切回概览）。 */
function restartTimers(now = false) {
  stopTimers();
  if (!watching()) {
    return;
  }
  scheduleLive(now ? 0 : LIVE_MS);
  scheduleDisks(now ? 0 : DISK_MS);
}

/** 完整采集一次（进页面、手动刷新、切隧道），然后接上两档节奏。 */
async function load() {
  if (!props.serverId) {
    return;
  }
  await admin.load(props.serverId);
  // 首屏就落一个点：否则从进页面到第一拍之间，分时图是空的。
  recordLive();
  restartTimers();
}

function onVisibility() {
  if (document.hidden) {
    stopTimers();
  } else if (windowFocused.value) {
    restartTimers(true);
  }
}

// 切回概览立刻续上；切到别的模块（还是占位）就收手。
watch(active, () => restartTimers(true));

watch(() => props.serverId, load);

onMounted(async () => {
  document.addEventListener('visibilitychange', onVisibility);
  // 面板被热键收起、最小化、切到别的程序：立刻停采。
  unlistenFocus = await onWindowFocusChange((focused) => {
    windowFocused.value = focused;
    if (focused) {
      restartTimers(true);
    } else {
      stopTimers();
    }
  });
  void load();
});

onUnmounted(() => {
  document.removeEventListener('visibilitychange', onVisibility);
  unlistenFocus?.();
  unlistenFocus = undefined;
  stopTimers();
  // 离开管理页就丢掉这次的采集结果：下次进来重新采，避免展示已经过期的指标。
  admin.reset();
});

/** 切模块只换视图，不打断已排好的刷新。 */
function selectModule(id: string) {
  active.value = id;
}

/* ───────────────────── 概览的取舍 ───────────────────── */

/*
 * 磁盘与网络都是"一项一行"：整张卡占满一行，卡里一块盘 / 一张网卡各占一行，
 * 每行右边跟一条自己的近一分钟曲线。不折叠、不截断——机器上有几块盘就是几行。
 */

/**
 * 一块盘 = 一行：占用（`lsblk` + `df`）与读写速率（`/proc/diskstats`）合在一起。
 *
 * 名字只认物理盘（`lsblk` 的 TYPE=disk），不掺挂载点。ZFS 成员盘是个特例：
 * 它自己没有挂载点（池才有），所以占用要取所属池的用量，行里注明是哪个池。
 */
interface DiskTile {
  name: string;
  /** -1 = 占用未知（未挂载，也不是池成员）。 */
  percent: number;
  /** 排序用：已用 kB（-1 = 未知）与盘容量 kB。 */
  used_kb: number;
  size_kb: number;
  /** 已用 / 总容量，或"容量 · 未挂载"。 */
  detail: string;
  hint: string;
  read_rate: number;
  write_rate: number;
  /** 这块盘自己的读写曲线（kB/s）。 */
  series: number[];
}

const diskTiles = computed<DiskTile[]>(() => {
  const ioByDevice = new Map(
    (overview.value?.disk_io ?? []).map((io) => [io.device, io] as const),
  );
  const poolByName = new Map(
    (overview.value?.zfs_pools ?? []).map((pool) => [pool.name, pool] as const),
  );

  const tiles: DiskTile[] = (overview.value?.block_disks ?? []).map((disk) => {
    const mounted = disk.total_kb > 0;
    const io = ioByDevice.get(disk.name);
    const pool = disk.pool ? poolByName.get(disk.pool) : undefined;
    const size = formatKb(disk.size_bytes / 1024);

    let percent = mounted ? usagePercent(disk.used_kb, disk.total_kb) : -1;
    let usedKb = mounted ? disk.used_kb : -1;
    let detail = mounted ? `${formatKb(disk.used_kb)} / ${size}` : `${size} · 未挂载`;
    let hint = mounted
      ? `${disk.name}：挂载在 ${disk.mount}`
      : `${disk.name}：没有自己的文件系统（RAID/阵列成员，由阵列统一使用）`;

    if (disk.pool) {
      // 池的容量是整池共享的，所以这里报的是池的用量，并在行里写明"池 xxx"
      percent = pool && pool.size_bytes > 0 ? (pool.alloc_bytes / pool.size_bytes) * 100 : -1;
      usedKb = pool?.alloc_bytes ?? -1;
      detail = pool
        ? `池 ${disk.pool} · ${formatKb(pool.alloc_bytes / 1024)} / ${formatKb(pool.size_bytes / 1024)}`
        : `${size} · 池 ${disk.pool}`;
      hint = `${disk.name}：ZFS 池 ${disk.pool} 的成员盘，占用按整池计算`;
    }

    return {
      name: disk.name,
      percent,
      used_kb: usedKb,
      size_kb: disk.size_bytes / 1024,
      detail,
      hint,
      read_rate: io?.read_rate ?? 0,
      write_rate: io?.write_rate ?? 0,
      series: (diskHistory.value[disk.name] ?? []).map((point) => point.value),
    };
  });

  // 有占用的在前（按已用量降序），未知的按容量降序沉底
  return tiles.sort((a, b) => {
    const rankA = a.used_kb < 0 ? 1 : 0;
    const rankB = b.used_kb < 0 ? 1 : 0;
    return rankA - rankB || b.size_kb - a.size_kb;
  });
});

/**
 * 一张网卡 = 一行。
 *
 * 有流量的排前面，同组内保持核心给的顺序：按速率精确排序会让每秒刷新时行不断换位。
 */
const networkRows = computed(() =>
  activeFirst(overview.value?.network ?? [], (nic) => nic.rx_rate + nic.tx_rate).map((nic) => ({
    ...nic,
    series: (nicHistory.value[nic.name] ?? []).map((point) => point.value),
  })),
);

/**
 * 有活动的排前面，同组内保持核心给的顺序。
 *
 * 不按速率精确排序：每秒刷新时数字一变就换位，界面会一直跳。
 */
function activeFirst<T>(items: T[], rateOf: (item: T) => number): T[] {
  return [...items].sort((a, b) => Number(rateOf(b) > 0) - Number(rateOf(a) > 0));
}

/* ───────────────────── 最近一分钟的分时图 ───────────────────── */

/*
 * 只活在内存里：离开页面就没了，不落盘、不上报。
 * 三档采样节奏不同，所以各记各的——CPU 每分钟 60 点，磁盘 IO 只有 12 点。
 */
const HISTORY_MS = 60_000;
const liveHistory = ref<{ at: number; cpu: number; mem: number; gpu: number }[]>([]);

/**
 * 每块盘 / 每张网卡各记一条曲线：卡里每一行右边都是"这台设备自己"的近一分钟，
 * 而不是整机汇总——否则一块闲盘和一块忙盘看起来一模一样。
 *
 * 用普通对象而不是 Map：模板里直接 `tile.series` 就能读，不用再包一层。
 */
const diskHistory = ref<Record<string, { at: number; value: number }[]>>({});
const nicHistory = ref<Record<string, { at: number; value: number }[]>>({});

/** 往逐设备的历史里塞一个点，顺手把过期的清掉。 */
function pushPoint(
  store: Record<string, { at: number; value: number }[]>,
  name: string,
  value: number,
  at: number,
): Record<string, { at: number; value: number }[]> {
  return { ...store, [name]: trimHistory([...(store[name] ?? []), { at, value }]) };
}

function trimHistory<T extends { at: number }>(list: T[]): T[] {
  const floor = Date.now() - HISTORY_MS;
  return list.filter((item) => item.at >= floor);
}

const cpuSeries = computed(() => liveHistory.value.map((point) => point.cpu));
const memSeries = computed(() => liveHistory.value.map((point) => point.mem));
const gpuSeries = computed(() => liveHistory.value.map((point) => point.gpu));

/** 主 GPU：概览只放一块，多卡的用户去 GPU 模块看（本期没做，先给第一块）。 */
const primaryGpu = computed(() => overview.value?.gpus[0] ?? null);

/* ───────────────────── 系统信息 ───────────────────── */

function pad2(value: number): string {
  return value < 10 ? `0${value}` : `${value}`;
}

/** 启动时间 = 现在 − 已运行秒数。前端算，省得核心再跑一次 `uptime -s`。 */
const bootTime = computed(() => {
  const secs = overview.value?.system.uptime_secs ?? 0;
  if (secs <= 0) {
    return '';
  }
  const at = new Date(Date.now() - secs * 1000);
  return `${at.getFullYear()}-${pad2(at.getMonth() + 1)}-${pad2(at.getDate())} ${pad2(at.getHours())}:${pad2(at.getMinutes())}:${pad2(at.getSeconds())}`;
});

/** 键值列表：空值整行不显示，比留一堆「未知」干净。 */
const systemRows = computed(() => {
  const system = overview.value?.system;
  if (!system) {
    return [];
  }
  return [
    { label: '主机名称', value: system.hostname },
    { label: '发行版本', value: system.os },
    { label: '内核版本', value: system.kernel },
    { label: '系统类型', value: system.arch },
    { label: '主机地址', value: system.host_ip ?? '' },
    { label: '启动时间', value: bootTime.value },
    { label: '运行时间', value: formatUptime(system.uptime_secs) },
  ].filter((row) => row.value);
});


</script>

<template>
  <div class="admin">
    <header class="admin-head">
      <button class="icon-btn" title="返回隧道" @click="emit('back')">
        <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M9.5 3.5L5 8l4.5 4.5"/></svg>
      </button>
      <div class="admin-id">
        <div class="admin-title">{{ server?.name || '服务器管理' }}</div>
        <div class="admin-sub mono">
          {{ server ? `${server.username}@${server.host}:${server.port}` : serverId }}
        </div>
      </div>
      <span class="spacer" />
      <button class="btn" :disabled="admin.loading" @click="load">
        <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M13 8a5 5 0 1 1-1.6-3.7"/><path d="M13 2.5V5h-2.5"/></svg>
        {{ admin.loading ? '采集中…' : '刷新' }}
      </button>
    </header>

    <div class="admin-body">
      <nav class="admin-nav">
        <div class="admin-nav-k">模块</div>
        <!--
          不用"规划中"这种文字徽章：八行里七行都挂一个同样的标签，
          等于把侧边栏一半宽度用来重复同一句话。未可用只用一个淡点表示，
          具体原因在右侧空态里说。
        -->
        <button
          v-for="item in admin.modules"
          :key="item.id"
          class="admin-nav-item"
          :class="{ 'is-active': active === item.id, 'is-off': item.state !== 'ready' }"
          :title="item.state === 'planned' ? `${item.title}（规划中）` : item.title"
          @click="selectModule(item.id)"
        >
          <span class="admin-nav-text">{{ item.title }}</span>
          <span
            v-if="item.state === 'unavailable'"
            class="admin-nav-dot is-bad"
            title="不可用"
          />
          <span v-else-if="item.state === 'planned'" class="admin-nav-dot" />
        </button>
        <div v-if="!admin.modules.length" class="admin-nav-empty">清单加载中…</div>
      </nav>

      <section class="admin-main">
        <div v-if="admin.error" class="admin-error">
          <div class="admin-error-title">采集失败</div>
          <p class="admin-error-body">{{ admin.error }}</p>
          <button class="btn is-primary" :disabled="admin.loading" @click="load">重试</button>
        </div>

        <div v-else-if="admin.loading && !overview" class="admin-loading">
          正在通过这条隧道的会话采集系统信息…
        </div>

        <template v-else-if="overview">
          <div v-if="admin.degraded.length" class="admin-degraded">
            <span class="admin-degraded-k">部分数据缺失</span>
            <span v-for="item in admin.degraded" :key="item" class="pill is-warn">{{ item }}</span>
          </div>

          <!-- ── 概览 ─────────────────────────────────────── -->
          <template v-if="active === 'overview'">
            <!-- 系统信息：一眼看清"这是哪台机器、什么系统、开了多久" -->
            <section class="panel admin-sys">
              <h3 class="panel-title">系统信息</h3>
              <dl class="admin-sys-list">
                <div v-for="row in systemRows" :key="row.label" class="admin-sys-row">
                  <dt>{{ row.label }}</dt>
                  <dd class="mono">{{ row.value }}</dd>
                </div>
              </dl>
              <div class="admin-hero-when mono">
                采集于 {{ clockText(overview.collected_at) }} · 耗时 {{ overview.elapsed_ms }} ms
                <span
                  v-if="admin.lightError"
                  class="admin-stale"
                  :title="admin.lightError"
                >· 最近一次刷新失败（显示的是上一份数据）</span>
              </div>
            </section>

            <!-- CPU / 内存 / GPU 并排：都是"一个百分比 + 一条近 1 分钟的曲线" -->
            <div class="admin-grid" :class="primaryGpu ? 'is-trio' : 'is-pair'">
              <section class="panel admin-gauge-card">
                <!-- 背景分时图：最近 60 秒，只描形状不标刻度 -->
                <Sparkline :points="cpuSeries" :max="100" />
                <div class="admin-gauge-head">
                  <span class="admin-metric-title">CPU</span>
                  <span class="admin-gauge-hint">每 1 秒 · 近 1 分钟</span>
                </div>
                <RingGauge :value="overview.cpu.usage_percent" :size="112" label="占用" />
              </section>

              <section class="panel admin-gauge-card">
                <Sparkline :points="memSeries" :max="100" />
                <div class="admin-gauge-head">
                  <span class="admin-metric-title">内存</span>
                  <span class="admin-gauge-hint">占用每 1 秒 · 构成每 10 秒</span>
                </div>
                <!-- 分段环：已用 / 缓存 / Buffers 各一段颜色，空着的就是空闲 -->
                <RingGauge
                  :value="memoryPercent"
                  :segments="memSegments"
                  :size="112"
                  label="已用"
                />
                <p class="admin-gauge-note mono">
                  {{ formatKb(overview.memory.used_kb) }} / {{ formatKb(overview.memory.total_kb) }}
                </p>
                <!-- 图例带用量：左栏是真正占着的，右栏是其它（缓存 + 空闲） -->
                <div class="mem-legend">
                  <dl class="mem-legend-col">
                    <div v-for="part in memUsedParts" :key="part.key" class="mem-legend-item">
                      <i :style="{ background: part.color }" />
                      <dt>{{ part.label }}</dt>
                      <dd class="mono">{{ part.kb > 0 ? formatKb(part.kb) : '—' }}</dd>
                    </div>
                  </dl>
                  <dl class="mem-legend-col">
                    <div v-for="part in memOtherParts" :key="part.key" class="mem-legend-item">
                      <i :style="{ background: part.color }" />
                      <dt>{{ part.label }}</dt>
                      <dd class="mono">{{ part.kb > 0 ? formatKb(part.kb) : '—' }}</dd>
                    </div>
                  </dl>
                </div>
              </section>

              <!-- 没有 GPU 的机器不占位：直接两列，不留一张空卡 -->
              <section v-if="primaryGpu" class="panel admin-gauge-card">
                <Sparkline :points="gpuSeries" :max="100" />
                <div class="admin-gauge-head">
                  <span class="admin-metric-title">GPU</span>
                  <span class="admin-gauge-hint">每 1 秒 · 近 1 分钟</span>
                </div>
                <RingGauge :value="primaryGpu.usage_percent" :size="112" label="占用" />
                <p class="admin-gauge-note mono admin-gpu-name" :title="primaryGpu.name">
                  {{ primaryGpu.name }}
                  <span v-if="primaryGpu.temperature_c > 0"> · {{ primaryGpu.temperature_c }}°C</span>
                </p>
                <p v-if="primaryGpu.memory_total_kb > 0" class="admin-gauge-note mono">
                  显存 {{ formatKb(primaryGpu.memory_used_kb) }} /
                  {{ formatKb(primaryGpu.memory_total_kb) }}
                </p>
              </section>
            </div>

            <!-- 磁盘与网络并排：都是"一项一行 + 各自曲线"，左右对照正好 -->
            <div class="admin-grid is-pair is-devices">
              <!-- 磁盘：一块盘一行，各自带自己的读写曲线 -->
              <section class="panel">
                <div class="admin-gauge-head">
                  <h3 class="panel-title">磁盘</h3>
                  <span class="admin-gauge-hint">读写每 1 秒 · 占用每 10 秒</span>
                </div>
                <div v-if="!diskTiles.length" class="admin-empty">没有读到磁盘清单</div>
                <div class="dev-rows">
                  <div v-for="tile in diskTiles" :key="tile.name" class="dev-row" :title="tile.hint">
                    <RingGauge v-if="tile.percent >= 0" :value="tile.percent" :size="44" />
                    <span v-else class="disk-hole is-small mono">—</span>
                    <div class="dev-main">
                      <span class="dev-name mono">{{ tile.name }}</span>
                      <span class="dev-detail mono">{{ tile.detail }}</span>
                    </div>
                    <span class="dev-io mono">
                      <span class="rate-up">R {{ rateText(tile.read_rate) }}</span>
                      <span class="rate-down">W {{ rateText(tile.write_rate) }}</span>
                    </span>
                    <div class="dev-spark">
                      <Sparkline :points="tile.series" :slots="60" />
                    </div>
                  </div>
                </div>
              </section>

              <!-- 网络：同样一项一行，每张网卡一条自己的曲线 -->
              <section class="panel">
                <div class="admin-gauge-head">
                  <h3 class="panel-title">网络流量</h3>
                  <span class="admin-gauge-hint">每 1 秒</span>
                </div>
                <div v-if="!overview.network.length" class="admin-empty">没有可展示的网卡</div>
                <div class="dev-rows">
                  <div
                    v-for="nic in networkRows"
                    :key="nic.name"
                    class="dev-row"
                    :title="`累计 ↑ ${formatKb(nic.tx_bytes / 1024)} · ↓ ${formatKb(nic.rx_bytes / 1024)}`"
                  >
                    <!-- 占位：磁盘那行左边有环，这里留同宽的空白，
                         两张卡的曲线才会从同一个 x 起、长度一模一样 -->
                    <span class="dev-lead" />
                    <div class="dev-main">
                      <span class="dev-name mono">{{ nic.name }}</span>
                      <span class="dev-detail mono">
                        累计 ↑ {{ formatKb(nic.tx_bytes / 1024) }} · ↓ {{ formatKb(nic.rx_bytes / 1024) }}
                      </span>
                    </div>
                    <span class="dev-io mono">
                      <span class="rate-up">↑ {{ rateText(nic.tx_rate) }}</span>
                      <span class="rate-down">↓ {{ rateText(nic.rx_rate) }}</span>
                    </span>
                    <div class="dev-spark">
                      <Sparkline :points="nic.series" :slots="60" />
                    </div>
                  </div>
                </div>
              </section>
            </div>

          </template>

          <!-- ── 未实现的模块：说清"规划中"，不给半成品按钮 ── -->
          <section v-else class="panel admin-placeholder">
            <div class="admin-placeholder-title">{{ activeModule?.title || active }}</div>
            <p class="admin-placeholder-note">{{ activeModule?.note || '这个模块还在规划中。' }}</p>
            <p class="hint">
              位置已经留好：核心的模块清单里已有这一项，接口前缀
              <code>/api/v1/servers/{id}/admin/</code>
              也已划定，实现后这一页直接换成本模块的界面。
            </p>
          </section>
        </template>

        <div v-else class="admin-empty">没有数据。点右上角「刷新」重新采集。</div>
      </section>
    </div>
  </div>
</template>

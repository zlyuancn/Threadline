<script setup lang="ts">
import { nextTick, onUnmounted, ref, watch } from 'vue';
import { ElMessage, ElMessageBox } from 'element-plus';
import { gatewayApi, hostsApi } from '../api';
import ProfileExportDialog from '../components/ProfileExportDialog.vue';
import ProfileImportDialog from '../components/ProfileImportDialog.vue';
import {
  exportGatewayCa,
  gatewayCaTrust,
  installGatewayCa,
  syncProxyOverride,
} from '../api/native';
import type { GatewayCaStatus, HostsStatus } from '../api/types';
import { useRuntimeStore } from '../models/runtime';
import { useHotkeyStore } from '../models/hotkey';
import { useServersStore } from '../models/servers';
import { THEMES, useThemeStore } from '../models/theme';
import { useUpdateStore } from '../models/update';
import { withTimeout } from '../models/asyncAction';
import {
  openExternal,
  setAutostartElevated,
  startCore,
  syncLogonEntry,
} from '../api/native';

const runtime = useRuntimeStore();
const hotkey = useHotkeyStore();
const servers = useServersStore();
const theme = useThemeStore();
const update = useUpdateStore();
const saving = ref(false);
const serviceBusy = ref(false);

/** 由 vite 的 define 注入（package.json 的 version），与界面角落同源。 */
const appVersion = __APP_VERSION__;

function formatSize(bytes: number): string {
  if (!bytes) {
    return '';
  }
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

/** 手动检查：有结果才提示，「检查中」的状态由按钮自己显示。 */
async function checkUpdateNow(): Promise<void> {
  const found = await update.check();
  if (update.error) {
    ElMessage.error(`检查更新失败：${update.error}`);
    return;
  }
  ElMessage.success(found ? `发现新版本 v${update.info?.latest}` : '已是最新版本');
}

/** 下载 + 安装：装完应用会自己退出，由安装器拉起新版。 */
async function runUpdate(): Promise<void> {
  if (update.installerPath) {
    await update.install();
    return;
  }
  const ok = await update.download();
  if (!ok) {
    ElMessage.error(update.error || '下载失败');
    return;
  }
  await update.install();
}

function openReleasePage(): void {
  void openExternal('https://github.com/zlyuancn/Threadline/releases/latest');
}

/** 主题按暗色 / 亮色分组展示。 */
const themeGroups = [
  { label: '暗色', items: THEMES.filter((option) => option.mode === 'dark') },
  { label: '亮色', items: THEMES.filter((option) => option.mode === 'light') },
];

/* ── 域名访问 / 本地网关证书 ───────────────────────────────────── */

const hostsStatus = ref<HostsStatus | null>(null);
const caStatus = ref<GatewayCaStatus | null>(null);
const hostsBusy = ref(false);
const caBusy = ref(false);

async function loadHostsAndCa() {
  if (runtime.connection !== 'online') {
    return;
  }
  try {
    hostsStatus.value = await hostsApi.status();
  } catch {
    hostsStatus.value = null;
  }
  try {
    caStatus.value = await gatewayApi.status();
  } catch {
    caStatus.value = null;
  }
}

/**
 * 写（或清空）托管区块。
 *
 * `POST /hosts/sync` 返回 409 表示需要提权，此时才拉 UAC；
 * 是不是真的写成功以 Core 重新给出的状态为准。
 */
async function writeHosts(clear = false) {
  if (hostsBusy.value) {
    return;
  }
  hostsBusy.value = true;
  try {
    const result = clear ? await hostsApi.clear() : await hostsApi.sync();
    const done = clear ? '托管区块已清空' : '已写入 hosts';
    if (result.written) {
      ElMessage.success(done);
    } else if (result.needs_elevation) {
      const { syncHostsElevated } = await import('../api/native');
      await syncHostsElevated();
      ElMessage.success(done);
    } else {
      ElMessage.warning(result.status.last_error ?? '写入未完成');
    }
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : String(e));
  } finally {
    hostsBusy.value = false;
    await loadHostsAndCa();
  }
}

async function syncProxy() {
  if (hostsBusy.value || !hostsStatus.value) {
    return;
  }
  hostsBusy.value = true;
  try {
    const report = await syncProxyOverride(
      hostsStatus.value.proxy.desired,
      hostsStatus.value.proxy.managed,
    );
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
    hostsBusy.value = false;
    await loadHostsAndCa();
  }
}

async function clearProxy() {
  try {
    await hostsApi.clearProxy();
    ElMessage.success('已清空代理例外的管理集');
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : String(e));
  } finally {
    await loadHostsAndCa();
  }
}

/** 装根会弹 Windows 安全警告：先用自己的确认说清楚，再交给 certutil。 */
async function installCa() {
  if (caBusy.value) {
    return;
  }
  try {
    await ElMessageBox.confirm(
      '接下来 Windows 会弹出系统安全警告，点「是」才会安装。只装到当前用户，不需要管理员。',
      '安装本地网关根证书',
      { confirmButtonText: '继续', cancelButtonText: '取消', type: 'warning' },
    );
  } catch {
    return;
  }
  caBusy.value = true;
  try {
    const result = await installGatewayCa(caStatus.value?.thumbprint_sha1);
    if (result.ok) {
      ElMessage.success('根证书已安装');
      await refreshCaTrust();
    } else {
      ElMessage.warning(result.message ?? '安装未完成（可能取消了系统警告）');
    }
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : String(e));
  } finally {
    caBusy.value = false;
    await loadHostsAndCa();
  }
}

/** 由 Agent（登录用户）查库后上报：Core 是 SYSTEM 时自己查永远是「未信任」。 */
async function refreshCaTrust() {
  if (!caStatus.value) {
    return;
  }
  // certutil 只认 SHA-1；SHA-256 只用于展示。
  const trusted = await gatewayCaTrust(caStatus.value.thumbprint_sha1);
  await gatewayApi.trustResult({
    trusted,
    thumbprint: caStatus.value.thumbprint,
    at: new Date().toISOString(),
  });
  await loadHostsAndCa();
}

/** 无可见桌面会话时装不了根：退化成让用户自己导入。 */
async function exportCa() {
  const pem = await exportGatewayCa();
  if (!pem) {
    ElMessage.warning('读不到根证书（Core 可能还没生成）');
    return;
  }
  const url = URL.createObjectURL(new Blob([pem], { type: 'application/x-pem-file' }));
  const anchor = document.createElement('a');
  anchor.href = url;
  anchor.download = 'threadline-gateway-root.pem';
  anchor.click();
  URL.revokeObjectURL(url);
}

async function resetCa() {
  try {
    await ElMessageBox.confirm(
      '删除后需要重启核心才会换新根；已安装的旧根不会自动从系统里删除。',
      '重新生成本地根证书',
      { confirmButtonText: '删除并重生成', cancelButtonText: '取消', type: 'warning' },
    );
  } catch {
    return;
  }
  try {
    await gatewayApi.reset();
    ElMessage.success('已删除，重启核心后生效');
    await loadHostsAndCa();
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : String(e));
  }
}

watch(
  () => runtime.connection,
  () => {
    void loadHostsAndCa();
  },
  { immediate: true },
);

/* ── 隧道顺序 ───────────────────────────────────────────────────── */

const orderOpen = ref(false);
const orderSaving = ref(false);
/** 导入导出各自一个弹窗：两个流程的确认逻辑完全不同，合在一个里会很乱。 */
const exportOpen = ref(false);
const importOpen = ref(false);
/** 弹窗里的本地顺序（id 数组）：拖动时改它，点保存才写回。 */
const order = ref<string[]>([]);
const listRef = ref<HTMLElement | null>(null);
/** 正在拖的行下标；null = 没在拖。 */
const dragging = ref<number | null>(null);
/** 拖动过程中的落点，用于给目标行加高亮。 */
const overIndex = ref<number | null>(null);
/**
 * 每个"槽位"的中线 Y（视口坐标），拖动开始时采一次就不动了。
 *
 * 为什么不能在每次 pointermove 里重新量当前 DOM：换位会改变行位置，
 * 而落点又是由位置算出来的 —— 指针正好压在某个中线上时，算出的落点会
 * 一帧往下一帧往回，表现为原地高频跳动。槽位是固定的，用它做映射就没有回路。
 */
const slotCenters = ref<number[]>([]);
/**
 * 按下点相对"被拖行中线"的偏移（通常为正：多数人点在中线以下）。
 *
 * 落点要按**被拖行的中线**去比，而不是按指针本身。否则在第一行偏下的位置按下，
 * 指针天然就已经越过第一行的中线，手指还没动，顺序就先跳了一格。
 */
const pressOffset = ref(0);
/**
 * 槽位之间的"边界"（相邻中线的中点）。
 *
 * 用边界而不是中线做判断：以中线为准会让"换过去"和"换回来"的阈值不对称，
 * 往回拖要退将近一整行才生效，手感很黏。
 */
const slotBounds = ref<number[]>([]);

function openOrder() {
  order.value = servers.servers.map((server) => server.id);
  orderOpen.value = true;
}

/**
 * 兜底：弹窗打开时隧道列表可能还没拉到（首帧就打开、或刚重启核心）。
 * 列表到位后再补一次，否则会看到一个空弹窗。
 */
watch(
  () => servers.servers.length,
  () => {
    if (orderOpen.value && order.value.length === 0 && servers.servers.length > 0) {
      order.value = servers.servers.map((server) => server.id);
    }
  },
);

function nameOf(id: string): string {
  return servers.servers.find((server) => server.id === id)?.name ?? id;
}

function hostOf(id: string): string {
  const server = servers.servers.find((item) => item.id === id);
  return server ? `${server.username}@${server.host}` : '';
}

/**
 * 指针拖拽，而不是 HTML5 draggable。
 *
 * 原因：WebView2 里 `draggable=true` 时，一旦在 dragover 中改动节点（换位），
 * Chromium 判定"拖拽源消失"直接中断整次拖拽；不换位则没有预览、手感也很差。
 * 指针事件完全由页面控制：按下记录源、移动算落点、松手才真正换位。
 */
function onPointerDown(index: number, event: PointerEvent) {
  if (event.button !== 0) {
    return;
  }
  // 行内按钮是独立操作，不触发拖动
  if ((event.target as HTMLElement).closest('button')) {
    return;
  }
  dragging.value = index;
  overIndex.value = index;
  const rows = listRef.value
    ? [...listRef.value.querySelectorAll<HTMLElement>('.order-item')]
    : [];
  slotCenters.value = rows.map((row) => {
    const rect = row.getBoundingClientRect();
    return rect.top + rect.height / 2;
  });
  slotBounds.value = slotCenters.value
    .slice(0, -1)
    .map((center, i) => (center + slotCenters.value[i + 1]) / 2);
  const source = rows[index]?.getBoundingClientRect();
  pressOffset.value = source ? event.clientY - (source.top + source.height / 2) : 0;
  window.addEventListener('pointermove', onPointerMove);
  window.addEventListener('pointerup', onPointerUp, { once: true });
}

/**
 * 用每一行的中线判断落点，并且**立刻换位**。
 *
 * 这里能边拖边换，是因为拖拽由指针事件驱动——不存在"原生拖拽源被移除"的问题
 * （那正是之前 HTML5 版本拖不动的根因）。换位后 `dragging` 跟着走，视觉上就是
 * 被拖的那行跟着鼠标在列表里移动。
 */
/**
 * 记录换位前每一行的位置（按 id key），用于事后做 FLIP 动画。
 *
 * 数组换位是"瞬间"的：DOM 一重排，行就跳过去了。要让它看起来在移动，
 * 只能先量旧位置、换完再补一个反向位移然后 transition 回 0。
 */
function capturePositions(): Map<string, number> {
  const positions = new Map<string, number>();
  listRef.value?.querySelectorAll<HTMLElement>('.order-item').forEach((row) => {
    if (row.dataset.id) {
      positions.set(row.dataset.id, row.offsetTop);
    }
  });
  return positions;
}

async function playFlip(before: Map<string, number>) {
  await nextTick();
  listRef.value?.querySelectorAll<HTMLElement>('.order-item').forEach((row) => {
    const previous = row.dataset.id ? before.get(row.dataset.id) : undefined;
    if (previous === undefined) {
      return;
    }
    const delta = previous - row.offsetTop;
    if (!delta) {
      return;
    }
    row.style.transition = 'none';
    row.style.transform = `translateY(${delta}px)`;
    requestAnimationFrame(() => {
      row.style.transition = 'transform 160ms cubic-bezier(0.2, 0.7, 0.3, 1)';
      row.style.transform = '';
    });
  });
}

function onPointerMove(event: PointerEvent) {
  if (dragging.value === null || !listRef.value) {
    return;
  }
  const bounds = slotBounds.value;
  if (!bounds.length) {
    return;
  }
  // 用"被拖行中线"的位置去比，而不是指针位置：按下时的偏移先扣掉，
  // 这样刚按下、还没移动时落点正好等于它自己所在的槽位。
  const rowCenter = event.clientY - pressOffset.value;
  let target = 0;
  while (target < bounds.length && rowCenter > bounds[target]) {
    target += 1;
  }
  overIndex.value = target;
  if (target === dragging.value) {
    return;
  }
  const before = capturePositions();
  const [moved] = order.value.splice(dragging.value, 1);
  order.value.splice(target, 0, moved);
  dragging.value = target;
  void playFlip(before);
}

function onPointerUp() {
  stopDragging();
}

function stopDragging() {
  dragging.value = null;
  overIndex.value = null;
  slotCenters.value = [];
  slotBounds.value = [];
  pressOffset.value = 0;
  window.removeEventListener('pointermove', onPointerMove);
}

/** 一键置顶 / 置底：长列表里拖到顶会很难受。 */
function moveToFirst(index: number) {
  const [moved] = order.value.splice(index, 1);
  order.value.unshift(moved);
}

function moveToLast(index: number) {
  const [moved] = order.value.splice(index, 1);
  order.value.push(moved);
}

async function saveOrder() {
  if (orderSaving.value) {
    return;
  }
  orderSaving.value = true;
  try {
    const ok = await servers.reorder(order.value);
    if (ok) {
      ElMessage.success('隧道顺序已保存');
      orderOpen.value = false;
    } else {
      ElMessage.error(servers.lastError);
    }
  } finally {
    orderSaving.value = false;
  }
}

/* ── 快捷键录制 ─────────────────────────────────────────────────── */

const recording = ref(false);

function stopCapture() {
  recording.value = false;
  window.removeEventListener('keydown', onCapture, true);
}

/** 把按键事件翻成 "Ctrl+Alt+T"。只认字母/数字/F1~F24，与后端解析保持一致。 */
function keyName(event: KeyboardEvent): string | null {
  const key = event.key;
  if (/^[a-zA-Z0-9]$/.test(key)) {
    return key.toUpperCase();
  }
  if (/^F([1-9]|1[0-9]|2[0-4])$/.test(key)) {
    return key.toUpperCase();
  }
  return null;
}

async function onCapture(event: KeyboardEvent) {
  event.preventDefault();
  event.stopPropagation();
  if (event.key === 'Escape') {
    stopCapture();
    return;
  }
  const name = keyName(event);
  if (!name) {
    // 只按下了修饰键：继续等主键，别急着报错
    return;
  }
  const mods: string[] = [];
  if (event.ctrlKey) mods.push('Ctrl');
  if (event.altKey) mods.push('Alt');
  if (event.shiftKey) mods.push('Shift');
  if (event.metaKey) mods.push('Win');
  if (!mods.length) {
    ElMessage.warning('至少要带一个修饰键（Ctrl / Alt / Shift / Win）');
    return;
  }

  stopCapture();
  const next = [...mods, name].join('+');
  const error = await hotkey.rebind(next);
  if (error) {
    ElMessage.error(`没改成：${error}`);
  } else {
    ElMessage.success(`快捷键已改为 ${hotkey.spec}`);
  }
}

function startCapture() {
  if (recording.value) {
    stopCapture();
    return;
  }
  recording.value = true;
  window.addEventListener('keydown', onCapture, true);
}

onUnmounted(stopCapture);

/**
 * 启停核心。
 *
 * 停止是最容易误触的破坏性操作，所以放在设置页并要二次确认；
 * 侧栏只负责显示状态。
 */
async function toggleService() {
  if (serviceBusy.value) {
    return;
  }
  const online = runtime.connection === 'online';
  if (online) {
    const ok = window.confirm('停止后所有转发会立即断开。\n开机自启不受影响，下次开机仍会拉起。确认停止？');
    if (!ok) {
      return;
    }
  }
  serviceBusy.value = true;
  try {
    if (online) {
      await withTimeout(runtime.shutdown(), 15_000, '停止超时：核心没有响应');
      ElMessage.success('服务已停止');
    } else {
      await startCore();
      ElMessage.success('正在启动，几秒后自动刷新');
      await runtime.refresh();
    }
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : String(e));
  } finally {
    serviceBusy.value = false;
  }
}

/**
 * 登录启动项与"开机自启"同生共死：开关一变就同步。
 *
 * 开机自启只负责登录前把**核心**拉起来；登录后的托盘靠这里写的 HKCU Run 项。
 * 写失败不阻断开关本身——注册层已经生效，只是登录后不会自动出托盘。
 */
async function syncLogonEntrySafe(enabled: boolean) {
  try {
    await syncLogonEntry(enabled);
  } catch (e) {
    ElMessage.warning(
      `开机自启已设置，但登录启动项没写成：${e instanceof Error ? e.message : String(e)}`,
    );
  }
}

function logonHint(enabled: boolean): string {
  return enabled ? '；登录后助手会自动启动' : '；登录后不再自动启动助手';
}

async function toggle(enabled: boolean) {
  if (saving.value) {
    return;
  }
  saving.value = true;
  try {
    const outcome = await withTimeout(
      runtime.setAutostart(enabled),
      15_000,
      '修改开机自启超时：核心没有响应',
    );

    if (outcome.ok) {
      await syncLogonEntrySafe(enabled);
      ElMessage.success(`${outcome.message}${logonHint(enabled)}`);
      return;
    }

    // 核心以用户身份运行，改 per-machine 任务计划必然没权限：
    // 由助手弹**一次** UAC，跑 `tl-core --autostart on|off`，完事复核。
    if (outcome.needsElevation) {
      const confirmed = window.confirm(
        `修改开机自启需要管理员权限。\n\n接下来会弹出一次 UAC 授权窗口，点「是」即可完成。`,
      );
      if (!confirmed) {
        return;
      }
      await withTimeout(
        setAutostartElevated(enabled),
        120_000,
        '等待管理员授权超时（可能取消了 UAC）',
      );
      await runtime.refresh();
      if (runtime.autostart?.enabled === enabled) {
        await syncLogonEntrySafe(enabled);
        ElMessage.success(
          enabled ? '已开启开机自启（下次开机生效）' : '已关闭开机自启（下次开机生效）',
        );
      } else {
        // 提权进程已成功，但核心（普通用户）可能读不到 SYSTEM 任务的定义。
        // 不假装成功，也不报失败——如实说明。
        ElMessage.warning('提权已执行，但当前身份读不到任务计划状态；以重启后的实际行为为准');
      }
      return;
    }

    ElMessage.error(outcome.message);
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : String(e));
  } finally {
    saving.value = false;
  }
}
</script>

<template>
  <div class="panel">
    <h3 class="panel-title">外观</h3>
    <template v-for="group in themeGroups" :key="group.label">
      <p class="hint" style="margin: 10px 0 6px">{{ group.label }}</p>
      <div class="theme-grid">
        <button
          v-for="option in group.items"
          :key="option.id"
          class="theme-card"
          :class="{ 'is-active': theme.current === option.id }"
          :data-theme="option.id"
          :style="{ background: option.bg }"
          @click="theme.set(option.id)"
        >
          <span class="theme-swatch">
            <span class="theme-dot" :style="{ background: option.accent }" />
            <span class="theme-dot" :style="{ background: option.accent2 }" />
          </span>
          <span class="theme-name">{{ option.name }}</span>
          <span class="theme-hint">{{ option.hint }}</span>
        </button>
      </div>
    </template>
    <p class="hint">主题只改变配色，不影响布局与尺寸。选择会记住，下次打开仍是这套。</p>
  </div>

  <div class="panel">
    <h3 class="panel-title">隧道管理</h3>
    <div class="service-row">
      <span class="hint" style="margin: 0">侧栏与切换条里的先后次序</span>
      <button class="btn" :disabled="!servers.servers.length" @click="openOrder">
        <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M3 5h10M3 8h10M3 11h10"/></svg>
        调整顺序
      </button>
    </div>
    <div class="service-row">
      <span class="hint" style="margin: 0">导出可分享的配置包（不含密码/私钥）</span>
      <button class="btn" :disabled="!servers.servers.length" @click="exportOpen = true">
        <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M8 3v8M5 8l3 3 3-3"/><path d="M3.5 13h9"/></svg>
        导出配置
      </button>
    </div>
    <div class="service-row">
      <span class="hint" style="margin: 0">从别人的配置包导入（导入后需自填密码）</span>
      <button class="btn" @click="importOpen = true">
        <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M8 10V2M5 5l3-3 3 3"/><path d="M3.5 13h9"/></svg>
        导入配置
      </button>
    </div>
  </div>

  <el-dialog v-model="orderOpen" title="调整隧道顺序" width="440px">
    <p class="hint" style="margin-top: 0">
      拖动行可以调整位置；也可以用右侧两个按钮直接置顶 / 置底。
    </p>
    <ul ref="listRef" class="order-list">
      <li
        v-for="(id, index) in order"
        :key="id"
        :data-id="id"
        class="order-item"
        :class="{
          'is-dragging': dragging === index,
          'is-over': dragging !== null && dragging !== index && overIndex === index,
        }"
        @pointerdown="onPointerDown(index, $event)"
      >
        <span class="order-grip" aria-hidden="true">
          <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"><path d="M6 4h.01M10 4h.01M6 8h.01M10 8h.01M6 12h.01M10 12h.01"/></svg>
        </span>
        <span class="order-text">
          <span class="order-name">{{ nameOf(id) }}</span>
          <span class="order-host">{{ hostOf(id) }}</span>
        </span>
        <button class="icon-btn" title="移到最前" :disabled="index === 0" @click="moveToFirst(index)">
          <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M8 12.5V4M4.5 7.5L8 4l3.5 3.5"/><path d="M3.5 2.5h9"/></svg>
        </button>
        <button class="icon-btn" title="移到最后" :disabled="index === order.length - 1" @click="moveToLast(index)">
          <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M8 3.5V12M4.5 8.5L8 12l3.5-3.5"/><path d="M3.5 13.5h9"/></svg>
        </button>
      </li>
    </ul>
    <template #footer>
      <el-button size="small" @click="orderOpen = false">取消</el-button>
      <el-button size="small" type="primary" :loading="orderSaving" @click="saveOrder">
        保存顺序
      </el-button>
    </template>
  </el-dialog>

  <div class="panel">
    <h3 class="panel-title">全局快捷键</h3>
    <div class="service-row">
      <span class="kbd-group">
        <kbd v-for="key in hotkey.keys" :key="key">{{ key }}</kbd>
      </span>
      <button class="btn" :class="{ 'is-primary': recording }" @click="startCapture">
        {{ recording ? '按下组合键…（Esc 取消）' : '修改快捷键' }}
      </button>
    </div>
    <p class="hint">
      在任何窗口按下即可呼出/收起面板。至少要带一个修饰键；如果组合已被其他程序占用，
      会直接告诉你并保留原来的设置。
    </p>
  </div>

  <div class="panel">
    <h3 class="panel-title">域名访问</h3>
    <el-descriptions :column="1" size="small" border>
      <el-descriptions-item label="hosts 文件">
        {{ hostsStatus?.path ?? '—' }}
      </el-descriptions-item>
      <el-descriptions-item label="托管域名">
        <span v-if="!hostsStatus?.managed.length">—</span>
        <span
          v-for="entry in hostsStatus?.managed ?? []"
          :key="entry.domain"
          class="mono"
          style="margin-right: 8px"
        >{{ entry.domain }} → {{ entry.address }}</span>
      </el-descriptions-item>
      <el-descriptions-item label="状态">
        <el-tag v-if="hostsStatus?.conflicts.length" type="danger" size="small">有冲突</el-tag>
        <el-tag v-else-if="hostsStatus?.needs_elevation" type="warning" size="small">待提权</el-tag>
        <el-tag v-else-if="hostsStatus?.pending" type="warning" size="small">待同步</el-tag>
        <el-tag v-else type="success" size="small">已同步</el-tag>
      </el-descriptions-item>
      <el-descriptions-item label="代理例外">
        <span v-if="!hostsStatus">—</span>
        <template v-else>
          {{ hostsStatus.proxy.pending ? '待同步' : '已同步' }}
          <span v-if="hostsStatus.proxy.pac" class="hint">（系统使用 PAC，例外不生效）</span>
          <span v-else-if="hostsStatus.proxy.enabled === false" class="hint">（系统代理未开启）</span>
        </template>
      </el-descriptions-item>
    </el-descriptions>
    <div class="service-row" style="margin-top: 10px">
      <button class="btn" :disabled="hostsBusy" @click="writeHosts(false)">写入域名映射</button>
      <button class="btn" :disabled="hostsBusy" @click="writeHosts(true)">移除域名映射</button>
      <button class="btn" :disabled="hostsBusy" @click="syncProxy">写入代理例外</button>
      <button class="btn" :disabled="hostsBusy" @click="clearProxy">移除代理例外</button>
    </div>
    <p class="hint">
      「写入」把域名映射写进 hosts、把代理例外写进系统代理设置；
      「移除」只删本软件写入的行，hosts 其它行和你自己的代理例外不受影响。
      移除后一旦保存转发，下一次写入会重新补上。
    </p>
    <p class="hint">
      用 <span class="mono">https://域名</span> 访问时，还需要在下方「本地网关证书」里安装根证书，
      否则浏览器会报「证书无效」。
    </p>
  </div>

  <div class="panel">
    <h3 class="panel-title">本地网关证书</h3>
    <el-descriptions :column="1" size="small" border>
      <el-descriptions-item label="指纹">{{ caStatus?.thumbprint ?? '—' }}</el-descriptions-item>
      <el-descriptions-item label="信任状态">
        <el-tag v-if="caStatus?.trusted === true" type="success" size="small">已信任</el-tag>
        <el-tag v-else-if="caStatus?.trusted === false" type="warning" size="small">未信任</el-tag>
        <el-tag v-else type="info" size="small">未知</el-tag>
      </el-descriptions-item>
      <el-descriptions-item label="存储位置">{{ caStatus?.dir ?? '—' }}</el-descriptions-item>
    </el-descriptions>
    <p v-if="caStatus?.trusted === false" class="hint" style="color: var(--el-color-warning)">
      当前根证书<strong>未被系统信任</strong>：浏览器访问 <span class="mono">https://域名</span>
      会报「证书无效」。点「安装到当前用户」，在 Windows 安全警告里点「是」。
    </p>
    <p
      v-else-if="!caStatus || caStatus.trusted === null"
      class="hint"
      style="color: var(--el-color-warning)"
    >
      还没探测到信任状态：点「刷新信任状态」。若未安装，浏览器访问
      <span class="mono">https://域名</span> 会报证书错误。
    </p>
    <p v-else class="hint">
      已信任：只用于「本地 HTTPS + 远端 HTTP」的域名（本地解密后明文回源）。
    </p>
    <div class="service-row">
      <button
        class="btn"
        :class="{ 'is-primary': caStatus?.trusted !== true }"
        :disabled="caBusy"
        @click="installCa"
      >
        安装到当前用户
      </button>
      <button class="btn" :disabled="caBusy" @click="refreshCaTrust">刷新信任状态</button>
      <button class="btn" @click="exportCa">下载证书</button>
      <button class="btn is-danger" @click="resetCa">重新生成</button>
    </div>
  </div>

  <div class="panel">
    <h3 class="panel-title">核心服务</h3>
    <div class="service-row">
      <span class="pill" :class="runtime.connection === 'online' ? 'is-on' : 'is-off'">
        {{ runtime.connection === 'online' ? '运行中' : '未运行' }}
      </span>
      <button
        class="btn"
        :class="{ 'is-danger': runtime.connection === 'online' }"
        :disabled="serviceBusy"
        @click="toggleService"
      >
        {{ serviceBusy ? '处理中…' : runtime.connection === 'online' ? '停止服务' : '启动服务' }}
      </button>
    </div>
    <p class="hint">
      停止后<strong>所有转发会立即断开</strong>，隧道配置不受影响，点「启动服务」即可恢复。
      它和上面的「开机自启」是两件事：这里只管当前这一次。
    </p>
  </div>

  <ProfileExportDialog v-model="exportOpen" />
  <ProfileImportDialog v-model="importOpen" />

  <div class="panel">
    <h3 class="panel-title">开机自启</h3>
    <el-switch
      :model-value="runtime.autostart?.enabled ?? false"
      :loading="saving"
      :disabled="saving"
      active-text="开启"
      inactive-text="关闭"
      @update:model-value="toggle"
    />
    <p class="hint">
      开启后由系统任务计划在<strong>登录前</strong>拉起核心，<strong>登录后助手会自动启动</strong>（托盘，
      由助手自己维护当前用户的启动项）；关闭后只在双击 <code>Threadline.exe</code> 时启动。
      切换开关只改注册层，<strong>不会停止正在运行的隧道</strong>；生效时机是下一次开机。
    </p>
    <el-alert
      v-if="runtime.autostart?.privilege_required"
      type="warning"
      :closable="false"
      show-icon
      title="当前核心以用户身份运行"
      description="修改开机自启需要一次管理员授权（会弹一次 UAC）。"
    />
  </div>

  <div class="panel">
    <h3 class="panel-title">核心信息</h3>
    <el-descriptions :column="1" size="small" border>
      <el-descriptions-item label="实例">
        {{ runtime.whoami?.instance_id ?? '—' }}
      </el-descriptions-item>
      <el-descriptions-item label="版本">
        {{ runtime.whoami?.version ?? '—' }}
      </el-descriptions-item>
      <el-descriptions-item label="运行方式">
        {{ runtime.modeLabel(runtime.whoami?.run_mode) }}
      </el-descriptions-item>
      <el-descriptions-item label="运行身份">
        {{ runtime.whoami?.identity ?? '—' }}
      </el-descriptions-item>
      <el-descriptions-item label="任务计划">
        {{ runtime.autostart?.task_exists ? (runtime.autostart.task_enabled ? '存在且启用' : '存在但禁用') : '不存在' }}
      </el-descriptions-item>
      <el-descriptions-item label="降级">
        {{ runtime.degradationLabels().join('；') || '无' }}
      </el-descriptions-item>
    </el-descriptions>
  </div>

  <div class="panel">
    <h3 class="panel-title">版本更新</h3>
    <div class="service-row">
      <span class="hint" style="margin: 0">
        当前版本 v{{ appVersion }}
        <template v-if="update.info"> · 最新 v{{ update.info.latest }}</template>
      </span>
      <button
        class="btn"
        :disabled="update.checking || update.downloading"
        @click="checkUpdateNow"
      >
        {{ update.checking ? '检查中…' : '检查更新' }}
      </button>
    </div>

    <template v-if="update.available">
      <p class="hint" style="margin: 8px 0 0">
        发现新版本 <strong>v{{ update.info?.latest }}</strong>
        <span v-if="update.info?.size"> · {{ formatSize(update.info.size) }}</span>
      </p>
      <el-progress
        v-if="update.downloading"
        :percentage="update.percent"
        :stroke-width="6"
        style="margin-top: 8px"
      />
      <div class="service-row" style="margin-top: 8px">
        <span class="hint" style="margin: 0">
          {{
            update.info?.installable
              ? '下载完成后自动安装并重启'
              : '当前是开发态，请到发布页手动下载'
          }}
        </span>
        <button
          v-if="update.info?.installable"
          class="btn is-primary"
          :disabled="update.downloading"
          @click="runUpdate"
        >
          {{
            update.downloading
              ? '下载中…'
              : update.installerPath
                ? '立即安装'
                : '下载并升级'
          }}
        </button>
        <button v-else class="btn" @click="openReleasePage">打开下载页</button>
      </div>
    </template>

    <p v-if="update.error" class="hint" style="color: var(--warn)">
      {{ update.error }}
    </p>
  </div>
</template>

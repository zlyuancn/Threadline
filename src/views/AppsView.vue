<script setup lang="ts">
// 「应用」：把标记了「添加到应用」的转发铺平成导航入口，并能一次搜全部。
//
// 这里刻意不复用首页的 ForwardCard：那边是管理单条转发的表单卡片，
// 这边是"点开就用"的应用磁贴——图标 + 本地地址，管理动作收进「…」菜单。
// 是否出现在这里由转发自己的 `in_app` 开关决定，不看所属隧道。
//
// 编辑模式只负责"怎么摆"：拖磁贴改顺序/归属、建组、改组名、删组。
// 名称/图标/端口仍在转发编辑弹窗里改——同一个字段不留两个入口。
import { computed, onMounted, ref } from 'vue';
import { ElMessage } from 'element-plus';
import { useServersStore } from '../models/servers';
import { useTunnelsStore } from '../models/tunnels';
import { useSelectionStore } from '../models/selection';
import { useDesktopPrefsStore } from '../models/desktopPrefs';
import { withTimeout } from '../models/asyncAction';
import { matchesQuery, type ForwardLike } from '../models/forwardView';
import {
  groupOf,
  groupOrder,
  insertAt,
  shiftItem,
  sortedIds,
  UNGROUPED,
  useAppLayoutStore,
} from '../models/appLayout';
import type { ServerDto, TunnelView } from '../api/types';
import ForwardTile from '../components/ForwardTile.vue';

const emit = defineEmits<{ (e: 'edit-forward', serverId: string, forwardId: string): void }>();

const servers = useServersStore();
const forwards = useTunnelsStore();
const selection = useSelectionStore();
const layout = useAppLayoutStore();
const prefs = useDesktopPrefsStore();

const query = ref('');
/**
 * 开 = 显示全部；关 = 只看启用的（auto_start）。
 *
 * 存在偏好里而不是组件里：这是"我怎么用这个列表"的习惯，关一次就该记住，
 * 每次进来都重新打开等于没开关。它也会随配置包一起导出。
 */
const showAll = computed({
  get: () => prefs.showAllApps,
  set: (value: boolean) => prefs.setShowAllApps(value),
});
const busyKey = ref<string | null>(null);

/** 编辑模式：拖动磁贴、改分组、建组/改组名/删组。 */
const editing = ref(false);
const renamingGroup = ref<string | null>(null);
const renameDraft = ref('');
const creatingGroup = ref(false);
const createDraft = ref('');

interface DragState {
  /** 正在拖的磁贴 id。 */
  id: string;
  label: string;
}

interface DropTarget {
  /** tile = 落在某张磁贴上；bucket = 组容器空白处（追加到该组末尾）。 */
  kind: 'tile' | 'bucket';
  id: string;
  /** 落在目标右半边：插到它后面。 */
  after: boolean;
}

const dragState = ref<DragState | null>(null);
const dropTarget = ref<DropTarget | null>(null);
/** 跟随指针的浮层位置。 */
const ghost = ref({ x: 0, y: 0 });

interface AppRow {
  view: TunnelView;
  server: ServerDto | null;
}

onMounted(() => {
  void layout.refresh();
});

/**
 * 顺序由 `app_order` 决定，配置数组的顺序只是缺省值
 * （全 0 时保持原顺序，旧配置不需要迁移）。
 */
const rows = computed<AppRow[]>(() =>
  forwards.items
    .filter((view) => view.definition.in_app !== false)
    .map((view, index) => ({ view, index }))
    .sort(
      (a, b) =>
        (a.view.definition.app_order ?? 0) - (b.view.definition.app_order ?? 0) || a.index - b.index,
    )
    .map(({ view }) => ({
      view,
      server: servers.servers.find((s) => s.id === view.definition.server_id) ?? null,
    })),
);

const filtered = computed(() =>
  rows.value
    .filter((row) => (showAll.value ? true : row.view.definition.auto_start))
    .filter((row) =>
      matchesQuery(
        row.view as unknown as ForwardLike,
        [
          row.view.definition.name,
          row.view.definition.note ?? '',
          row.view.definition.group ?? '',
          row.server?.name ?? '',
          row.server?.host ?? '',
        ],
        query.value,
      ),
    ),
);

/**
 * 分组：顺序与空组来自核心的清单，成员归属仍看转发自己的 `group`（与所属隧道无关）。
 * 编辑模式下保留空组——不然刚建的组看不见，也没地方拖进去。
 */
const groups = computed(() =>
  groupOrder(
    filtered.value,
    (row) => groupOf(row.view),
    layout.groups,
    editing.value && !query.value,
  ),
);

const runningCount = computed(
  () => rows.value.filter((row) => row.view.status.state === 'running').length,
);

/**
 * 统一的"执行 + 回读"。
 *
 * 布局操作（key 为 `layout` / `g:`）**不受互斥锁限制**：拖动是连续动作，
 * 前一次还没落地就把后一次丢掉，用户会觉得"拖了没反应"。启停/删除仍互斥——那些会动端口。
 */
async function run(
  key: string,
  work: () => Promise<void>,
  force = key === 'layout' || key.startsWith('g:'),
) {
  if (busyKey.value && !force) {
    return;
  }
  busyKey.value = key;
  try {
    await withTimeout(work(), 30_000, '操作超时：核心没有响应');
    await forwards.refresh();
    // 改完布局回读清单：新组（自动收录的）与顺序都以核心为准。
    await layout.refresh();
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : String(e));
  } finally {
    busyKey.value = null;
  }
}

function toggle(view: TunnelView, enabled: boolean) {
  void run(`f:${view.definition.id}`, async () => {
    if (enabled) {
      await forwards.setAutoStart(view.definition.id, true);
      await forwards.start(view.definition.id);
    } else {
      await forwards.stop(view.definition.id);
      await forwards.setAutoStart(view.definition.id, false);
    }
    if (forwards.lastError) {
      ElMessage.error(forwards.lastError);
    }
  });
}

function edit(serverId: string | undefined, forwardId: string) {
  if (!serverId) {
    return;
  }
  // 编辑表单在隧道详情页；带上转发 id，让那边滚到这张卡并直接打开弹窗。
  selection.select(serverId);
  emit('edit-forward', serverId, forwardId);
}

function remove(view: TunnelView) {
  if (view.status.state === 'running' || view.status.state === 'connecting') {
    const ok = window.confirm(
      `「${view.definition.name}」正在运行，删除会先停止这条转发。继续？`,
    );
    if (!ok) {
      return;
    }
  }

  void run(`f:${view.definition.id}`, async () => {
    await forwards.remove(view.definition.id);
    if (forwards.lastError) {
      ElMessage.error(forwards.lastError);
    } else {
      ElMessage.success(`已删除「${view.definition.name}」`);
    }
  });
}

/* ───────────────────────── 编辑模式 ───────────────────────── */

/*
 * 拖拽用**指针事件**实现，不用 HTML5 拖放。
 *
 * 原因：Windows 的 WebView2 里 Tauri 接管了原生拖放（`dragDropEnabled` 默认开），
 * `draggable=true` 的元素收不到 `dragstart`——表现就是"怎么拖都不动"。
 * 指针拖拽不依赖那条链路，落点用 `elementFromPoint` 判定，跨组与组排序都能做。
 */

function toggleEditing() {
  editing.value = !editing.value;
  renamingGroup.value = null;
  creatingGroup.value = false;
  stopDrag();
  if (editing.value) {
    // 过滤视图下拖动会算出错误顺序：进编辑就先清掉过滤条件。
    query.value = '';
    showAll.value = true;
    void layout.refresh();
  }
}

function startDrag(event: PointerEvent, state: DragState) {
  if (!editing.value || event.button !== 0) {
    return;
  }
  dragState.value = state;
  ghost.value = { x: event.clientX, y: event.clientY };
  window.addEventListener('pointermove', onPointerMove);
  window.addEventListener('pointerup', onPointerUp);
  window.addEventListener('pointercancel', stopDrag);
}

function onPointerMove(event: PointerEvent) {
  if (!dragState.value) {
    return;
  }
  ghost.value = { x: event.clientX, y: event.clientY };
  dropTarget.value = hitTest(event.clientX, event.clientY);
}

function hitTest(x: number, y: number): DropTarget | null {
  const drag = dragState.value;
  const element = document.elementFromPoint(x, y) as HTMLElement | null;
  if (!drag || !element) {
    return null;
  }

  const tile = element.closest<HTMLElement>('[data-tile-id]');
  if (tile?.dataset.tileId) {
    // 拖到自己身上 = 没动
    if (tile.dataset.tileId === drag.id) {
      return null;
    }
    const rect = tile.getBoundingClientRect();
    return { kind: 'tile', id: tile.dataset.tileId, after: x > rect.left + rect.width / 2 };
  }

  const bucket = element.closest<HTMLElement>('[data-group]');
  if (bucket?.dataset.group) {
    return { kind: 'bucket', id: bucket.dataset.group, after: false };
  }
  return null;
}

function onPointerUp() {
  const drag = dragState.value;
  const target = dropTarget.value;
  stopDrag();
  if (!drag || !target) {
    return;
  }
  applyTileDrop(drag.id, target);
}

function stopDrag() {
  dragState.value = null;
  dropTarget.value = null;
  window.removeEventListener('pointermove', onPointerMove);
  window.removeEventListener('pointerup', onPointerUp);
  window.removeEventListener('pointercancel', stopDrag);
}

/**
 * 磁贴落位：先算插入位置，跨组时再改归属。
 *
 * 位置在**去掉自己之后**的序列里算——否则"拖到自己的右半边"会把自己算两遍。
 * 顺序只认核心的 `app-order`，本地不存第二份，免得两边不一致时来回跳。
 */
function applyTileDrop(tileId: string, target: DropTarget) {
  const all = sortedIds(forwards.items);
  const next = insertAt(
    all,
    tileId,
    target.kind === 'tile' ? target.id : null,
    target.kind === 'tile' ? target.after : false,
  );

  const current = rows.value.find((row) => row.view.definition.id === tileId);
  if (!current) {
    return;
  }
  const anchorRow =
    target.kind === 'tile' ? rows.value.find((row) => row.view.definition.id === target.id) : null;
  const wanted = target.kind === 'bucket' ? target.id : anchorRow ? groupOf(anchorRow.view) : null;

  void run('layout', async () => {
    if (wanted !== null && wanted !== groupOf(current.view)) {
      await forwards.update(tileId, { group: wanted === UNGROUPED ? '' : wanted });
      if (forwards.lastError) {
        throw new Error(forwards.lastError);
      }
    }
    if (next.join('\n') !== all.join('\n')) {
      await layout.reorder(next);
    }
  });
}

/** 分组在清单里的位置（未分组不在清单里，返回 -1）。 */
function groupIndex(name: string): number {
  return layout.groups.findIndex((group) => group.name === name);
}

/**
 * 分组排序：按钮上移/下移/置顶/置底。
 *
 * 不用拖动——组头横条太窄，拖起来比点按钮难命中；而且组数量少，
 * "一步到位"的四个动作比拖到位置更省事。只改清单顺序，成员归属一律不动。
 */
function moveGroup(name: string, delta: number | 'first' | 'last') {
  const names = layout.groups.map((group) => group.name);
  const next = shiftItem(names, name, delta);
  // 引用没变 = 本来就在这个位置，不发无意义的写请求
  if (next === names) {
    return;
  }
  void run('layout', async () => {
    await layout.setAll(next);
  });
}

/** 磁贴上的分组选择：选已有的，或直接输入新名字（核心会自动收录这个组）。 */
function pickGroup(row: AppRow, value: string) {
  const target = (value || '').trim();
  if (target === groupOf(row.view)) {
    return;
  }
  void run(`g:${row.view.definition.id}`, async () => {
    await forwards.update(row.view.definition.id, {
      group: target === UNGROUPED ? '' : target,
    });
    if (forwards.lastError) {
      throw new Error(forwards.lastError);
    }
  });
}

function startRename(name: string) {
  renamingGroup.value = name;
  renameDraft.value = name;
}

function commitRename() {
  const from = renamingGroup.value;
  const to = renameDraft.value.trim();
  renamingGroup.value = null;
  if (!from || !to || to === from) {
    return;
  }
  void run('layout', async () => {
    await layout.rename(from, to);
  });
}

function removeGroup(name: string) {
  const ok = window.confirm(`删除分组「${name}」？组里的应用会回到「${UNGROUPED}」，不会被删除。`);
  if (!ok) {
    return;
  }
  void run('layout', async () => {
    await layout.remove(name);
  });
}

function commitCreate() {
  const name = createDraft.value.trim();
  creatingGroup.value = false;
  createDraft.value = '';
  if (!name) {
    return;
  }
  void run('layout', async () => {
    await layout.create(name);
  });
}
</script>

<template>
  <div class="main">
    <div class="page-head">
      <div>
        <div class="eyebrow">APPS</div>
        <h1 class="page-title">应用</h1>
        <p class="section-sub">
          已添加到应用的 {{ rows.length }} 个服务，其中 {{ runningCount }} 个运行中。
        </p>
      </div>
      <div class="section-actions">
        <button
          class="btn"
          :class="{ 'is-primary': editing }"
          :title="editing ? '完成布局编辑（改动已即时保存）' : '编辑布局：拖动磁贴、修改分组'"
          @click="toggleEditing"
        >
          <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M3 13h3l7-7-3-3-7 7v3z"/><path d="M9.5 3.5l3 3"/></svg>
          {{ editing ? '完成' : '编辑' }}
        </button>
        <label
          :title="showAll ? '关闭后只显示已启用（随隧道自启）的转发' : '打开后显示全部转发'"
          style="
            display: flex;
            align-items: center;
            gap: 6px;
            font-size: 12px;
            color: var(--text-3);
            white-space: nowrap;
          "
        >
          <el-switch v-model="showAll" size="small" :disabled="editing" />
          {{ showAll ? '显示全部' : '仅显示启用' }}
        </label>
        <!-- 和"显示全部"并排：两个都是"我怎么用这一页"的习惯，放在使用现场才找得到 -->
        <label
          :title="
            prefs.hideOnLaunch
              ? '点开应用、浏览器接手之后，面板自动收进托盘（快捷键仍可唤出）'
              : '点开应用后，本窗口保持打开'
          "
          style="
            display: flex;
            align-items: center;
            gap: 6px;
            font-size: 12px;
            color: var(--text-3);
            white-space: nowrap;
          "
        >
          <el-switch
            :model-value="prefs.hideOnLaunch"
            size="small"
            :disabled="editing"
            @update:model-value="
              (value: string | number | boolean) => prefs.setHideOnLaunch(value === true)
            "
          />
          {{ prefs.hideOnLaunch ? '打开后隐藏' : '打开后保留' }}
        </label>
        <input
          v-model="query"
          class="search"
          type="search"
          :disabled="editing"
          :placeholder="editing ? '编辑布局时不搜索' : '搜索名称 / 备注 / IP / 端口 / 隧道'"
        />
      </div>
    </div>

    <div v-if="editing" class="edit-hint">
      按住磁贴拖动可以换位置、换分组；点组名直接改名；组头右侧的箭头调分组顺序与先后；
      磁贴下方的下拉能选已有分组，也能直接输入新名字建组。改动即时保存，点「完成」退出。
    </div>

    <div v-if="groups.length" class="all-groups" :class="{ 'is-editing': editing }">
      <section
        v-for="group in groups"
        :key="group.group"
        class="group"
        :class="{ 'is-drop': dropTarget?.kind === 'bucket' && dropTarget.id === group.group }"
        :data-group="group.group"
      >
        <header class="group-head">
          <span class="group-mark">
            <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M2 4.5A1.5 1.5 0 0 1 3.5 3h3l1.5 2h4.5A1.5 1.5 0 0 1 14 6.5v5A1.5 1.5 0 0 1 12.5 13h-9A1.5 1.5 0 0 1 2 11.5v-7z"/></svg>
          </span>
          <div class="group-id">
            <input
              v-if="editing && renamingGroup === group.group"
              v-model="renameDraft"
              class="group-rename-input"
              autofocus
              @keyup.enter="commitRename"
              @keyup.esc="renamingGroup = null"
              @blur="commitRename"
            />
            <button
              v-else-if="editing && group.group !== UNGROUPED"
              class="group-name is-editable"
              title="点击改名"
              @click="startRename(group.group)"
            >
              {{ group.group }}
            </button>
            <span v-else class="group-name">{{ group.group }}</span>
          </div>
          <div v-if="editing && group.group !== UNGROUPED" class="group-actions">
            <button
              class="group-move"
              :disabled="groupIndex(group.group) === 0"
              title="移到最前"
              @click="moveGroup(group.group, 'first')"
            >
              <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M3 3v10"/><path d="M13 4.5L8 8l5 3.5"/></svg>
            </button>
            <button
              class="group-move"
              :disabled="groupIndex(group.group) === 0"
              title="上移"
              @click="moveGroup(group.group, -1)"
            >
              <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M8 12.5v-9"/><path d="M4.5 7L8 3.5 11.5 7"/></svg>
            </button>
            <button
              class="group-move"
              :disabled="groupIndex(group.group) === layout.groups.length - 1"
              title="下移"
              @click="moveGroup(group.group, 1)"
            >
              <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M8 3.5v9"/><path d="M4.5 9L8 12.5 11.5 9"/></svg>
            </button>
            <button
              class="group-move"
              :disabled="groupIndex(group.group) === layout.groups.length - 1"
              title="移到最后"
              @click="moveGroup(group.group, 'last')"
            >
              <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M13 3v10"/><path d="M3 4.5L8 8l-5 3.5"/></svg>
            </button>
            <button
              class="group-del"
              title="删除分组（组内应用回到未分组，不会被删除）"
              @click="removeGroup(group.group)"
            >
              <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M4.5 4.5l7 7M11.5 4.5l-7 7"/></svg>
            </button>
          </div>
          <span class="group-count">{{ group.items.length }}</span>
        </header>

        <div class="tiles">
          <div
            v-for="row in group.items"
            :key="row.view.definition.id"
            class="tile-cell"
            :class="{
              'is-editing': editing,
              'is-dragging': dragState?.id === row.view.definition.id,
              'is-drop-before':
                dropTarget?.kind === 'tile' &&
                dropTarget.id === row.view.definition.id &&
                !dropTarget.after,
              'is-drop-after':
                dropTarget?.kind === 'tile' &&
                dropTarget.id === row.view.definition.id &&
                dropTarget.after,
            }"
            :data-tile-id="row.view.definition.id"
          >
            <div class="tile-body">
              <ForwardTile
                :view="row.view"
                :server="row.server"
                :busy-key="busyKey"
                @toggle="(value: boolean) => toggle(row.view, value)"
                @edit="edit(row.server?.id, row.view.definition.id)"
                @remove="remove(row.view)"
              />
              <!-- 编辑态盖一层：整块磁贴都能拖，同时挡住"点一下就打开浏览器" -->
              <div
                v-if="editing"
                class="tile-mask"
                title="拖动调整位置；拖到别的组即改分组"
                @pointerdown.stop.prevent="
                  startDrag($event, {
                    id: row.view.definition.id,
                    label: row.view.definition.name,
                  })
                "
              />
            </div>
            <el-select
              v-if="editing"
              :model-value="groupOf(row.view)"
              size="small"
              filterable
              allow-create
              default-first-option
              class="tile-group"
              title="改分组：选已有的，或直接输入新名字"
              @change="(value: string) => pickGroup(row, value)"
            >
              <el-option v-for="g in layout.groups" :key="g.name" :label="g.name" :value="g.name" />
              <el-option :label="UNGROUPED" :value="UNGROUPED" />
            </el-select>
          </div>
        </div>

        <div v-if="editing && !group.items.length" class="group-empty">
          把应用拖到这一组
        </div>
      </section>

      <div v-if="editing" class="group-create">
        <input
          v-if="creatingGroup"
          v-model="createDraft"
          class="group-create-input"
          placeholder="新分组名，回车创建"
          autofocus
          @keyup.enter="commitCreate"
          @keyup.esc="creatingGroup = false"
          @blur="commitCreate"
        />
        <button v-else class="btn" @click="creatingGroup = true">＋ 新建分组</button>
      </div>
    </div>

    <div v-else-if="rows.length" class="empty">
      <div class="empty-title">没有匹配的应用</div>
      <div>换个关键词试试：名称、备注、分组、IP、端口、隧道名都能搜。</div>
    </div>

    <div v-else class="empty">
      <img src="/logo.svg" alt="" />
      <div class="empty-title">还没有添加到应用的服务</div>
      <div>在「所有转发」里编辑一条转发，打开「添加到应用」开关，它就会出现在这里。</div>
    </div>

    <!-- 拖动时跟随指针的浮层：明确"正在拖哪一个" -->
    <Teleport to="body">
      <div
        v-if="dragState"
        class="drag-ghost"
        :style="{ transform: `translate3d(${ghost.x + 14}px, ${ghost.y + 12}px, 0)` }"
      >
        {{ dragState.label }}
      </div>
    </Teleport>
  </div>
</template>

<style scoped>
.page-head {
  display: flex;
  align-items: flex-end;
  justify-content: space-between;
  gap: 16px;
  margin-bottom: 18px;
  flex-wrap: wrap;
}

.page-title {
  margin: 6px 0 0;
  font-size: 26px;
  font-weight: 700;
  letter-spacing: -0.3px;
}

.all-groups {
  display: flex;
  flex-direction: column;
  gap: 22px;
}

.group-head {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-bottom: 8px;
  padding: 0 2px 8px;
  border-bottom: 1px solid var(--line-strong);
}

.group-mark {
  display: grid;
  place-items: center;
  width: 26px;
  height: 26px;
  flex-shrink: 0;
  border: 1px solid var(--line);
  border-radius: 8px;
  background: var(--tint);
  color: var(--text-2);
}

.group-mark svg {
  width: 14px;
  height: 14px;
}

.group-id {
  display: flex;
  align-items: baseline;
  gap: 8px;
  min-width: 0;
}

.group-name {
  color: var(--text);
  font-size: 13.5px;
  font-weight: 650;
  white-space: nowrap;
}

.group-count {
  min-width: 18px;
  height: 18px;
  margin-left: auto;
  padding: 0 5px;
  border-radius: 6px;
  background: var(--tint);
  font-size: 10px;
  line-height: 18px;
  text-align: center;
  color: var(--text-2);
}

/* 导航页式的入口矩阵：窄格子、图标为主，一屏能放下整组服务 */
.tiles {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(140px, 1fr));
  gap: 8px 10px;
}

/* ───────────────────────── 编辑模式 ───────────────────────── */

.edit-hint {
  margin: -4px 0 16px;
  padding: 8px 12px;
  border: 1px dashed var(--line-strong);
  border-radius: 10px;
  background: var(--tint);
  font-size: 11.5px;
  line-height: 1.6;
  color: var(--text-3);
}

.group {
  border-radius: 12px;
  transition: background 120ms ease;
}

/* 拖到组容器空白处：整组高亮，表示"放进这一组" */
.group.is-drop {
  background: var(--mint-soft);
  box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--mint) 35%, transparent);
}

.group-name.is-editable {
  padding: 0 0 1px;
  border: none;
  border-bottom: 1px dashed var(--line-strong);
  background: none;
  color: var(--text);
  font: inherit;
  font-size: 13.5px;
  font-weight: 650;
  cursor: text;
}

/* 组头左侧的拖拽手柄：只有它能把整组拖走，组名仍然用来改名 */
/* 排序按钮组：置顶 / 上移 / 下移 / 置底，一步到位比拖动好命中 */
.group-actions {
  display: flex;
  align-items: center;
  gap: 2px;
  flex-shrink: 0;
  padding-left: 2px;
}

.group-move,
.group-del {
  display: grid;
  place-items: center;
  width: 20px;
  height: 20px;
  padding: 0;
  border: none;
  border-radius: 5px;
  background: transparent;
  color: var(--text-3);
  cursor: pointer;
  transition:
    background 120ms ease,
    color 120ms ease;
}

.group-move:hover:not(:disabled),
.group-del:hover {
  background: var(--tint-strong);
  color: var(--text);
}

.group-move:disabled {
  opacity: 0.3;
  cursor: default;
}

.group-move svg {
  width: 11px;
  height: 11px;
}

.group-rename-input,
.group-create-input {
  height: 26px;
  padding: 0 8px;
  border: 1px solid color-mix(in srgb, var(--mint) 45%, var(--line));
  border-radius: 8px;
  background: var(--tint);
  color: var(--text);
  font: inherit;
  font-size: 12.5px;
  outline: none;
}

.group-rename-input {
  width: 150px;
  font-weight: 650;
}

.group-create-input {
  width: 220px;
}

/* 删除单独给红色 hover：和四个排序按钮区分开，避免误点 */
.group-del:hover {
  background: var(--bad-soft);
  color: var(--bad-text);
}

.group-del svg {
  width: 11px;
  height: 11px;
}

/* 磁贴外面这层只负责拖动与落点提示，视觉上仍然是一张不描边的卡片 */
.tile-cell {
  position: relative;
  display: flex;
  flex-direction: column;
  gap: 4px;
  padding: 4px;
  border: 1px solid transparent;
  border-radius: 12px;
  transition:
    border-color 120ms ease,
    opacity 120ms ease,
    background 120ms ease;
}

.all-groups.is-editing .tile-cell {
  border-color: var(--line);
  border-style: dashed;
}

.tile-body {
  position: relative;
}

/* 编辑态盖在磁贴上的透明层：整块都能按住拖，同时挡住磁贴内部的点击 */
.tile-mask {
  position: absolute;
  inset: 0;
  border-radius: 10px;
  cursor: grab;
  touch-action: none;
}

.tile-mask:active {
  cursor: grabbing;
}

.tile-cell.is-dragging {
  opacity: 0.35;
}

/* 落点：竖线画在两块之间，比整块高亮更能表达"插到哪" */
.tile-cell.is-drop-before::before,
.tile-cell.is-drop-after::after {
  content: '';
  position: absolute;
  top: 2px;
  bottom: 2px;
  width: 2px;
  border-radius: 2px;
  background: var(--mint);
  box-shadow: 0 0 6px color-mix(in srgb, var(--mint) 60%, transparent);
}

.tile-cell.is-drop-before::before {
  left: -3px;
}

.tile-cell.is-drop-after::after {
  right: -3px;
}

/* 跟随指针的拖动浮层 */
.drag-ghost {
  position: fixed;
  top: 0;
  left: 0;
  z-index: 3000;
  max-width: 220px;
  padding: 4px 10px;
  border: 1px solid color-mix(in srgb, var(--mint) 45%, transparent);
  border-radius: 8px;
  background: var(--card-grad);
  box-shadow: 0 10px 24px rgba(0, 0, 0, 0.35);
  color: var(--text);
  font-size: 11.5px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  /* 不能让浮层挡住 elementFromPoint，否则永远命中它自己 */
  pointer-events: none;
}

.tile-group {
  width: 100%;
}

.tile-group :deep(.el-select__wrapper) {
  min-height: 24px;
  padding: 0 6px;
  font-size: 11px;
}

.group-empty {
  padding: 10px 12px;
  border: 1px dashed var(--line-strong);
  border-radius: 10px;
  font-size: 11px;
  color: var(--text-3);
  text-align: center;
}

.group-create {
  display: flex;
  justify-content: flex-start;
}
</style>

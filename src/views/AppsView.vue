<script setup lang="ts">
// 「应用」：把标记了「添加到应用」的转发铺平成导航入口，并能一次搜全部。
//
// 这里刻意不复用首页的 ForwardCard：那边是管理单条转发的表单卡片，
// 这边是"点开就用"的应用磁贴——图标 + 本地地址，管理动作收进「…」菜单。
// 是否出现在这里由转发自己的 `in_app` 开关决定，不看所属隧道。
import { computed, ref } from 'vue';
import { ElMessage } from 'element-plus';
import { useServersStore } from '../models/servers';
import { useTunnelsStore } from '../models/tunnels';
import { useSelectionStore } from '../models/selection';
import { withTimeout } from '../models/asyncAction';
import { matchesQuery, type ForwardLike } from '../models/forwardView';
import type { TunnelView } from '../api/types';
import ForwardTile from '../components/ForwardTile.vue';

const emit = defineEmits<{ (e: 'edit-forward', serverId: string, forwardId: string): void }>();

const servers = useServersStore();
const forwards = useTunnelsStore();
const selection = useSelectionStore();

const query = ref('');
/** 开 = 显示全部；关 = 只看启用的（auto_start）。 */
const showAll = ref(true);
const busyKey = ref<string | null>(null);

const rows = computed(() =>
  forwards.items
    .filter((view) => view.definition.in_app !== false)
    .map((view) => ({
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
 * 按「转发自己的分组字段」聚合，与所属隧道无关——
 * 同一台机器上的 HA 和 Grafana 可以分别在「app」「工具」里。
 * 未分组统一收在最后，不参与字母序。
 */
const groups = computed(() => {
  const map = new Map<string, { group: string; rows: typeof rows.value }>();
  for (const row of filtered.value) {
    const group = row.view.definition.group?.trim() || '未分组';
    if (!map.has(group)) {
      map.set(group, { group, rows: [] });
    }
    map.get(group)!.rows.push(row);
  }
  // 顺序跟用户建立分组的先后走（导入时就是 nav 里的顺序），不按字母重排；
  // 「未分组」永远收在最后。
  const all = [...map.values()];
  return [...all.filter((g) => g.group !== '未分组'), ...all.filter((g) => g.group === '未分组')];
});

const runningCount = computed(
  () => rows.value.filter((row) => row.view.status.state === 'running').length,
);

async function run(key: string, work: () => Promise<void>) {
  if (busyKey.value) {
    return;
  }
  busyKey.value = key;
  try {
    await withTimeout(work(), 30_000, '操作超时：核心没有响应');
    await forwards.refresh();
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
          <el-switch v-model="showAll" size="small" />
          {{ showAll ? '显示全部' : '仅显示启用' }}
        </label>
        <input
          v-model="query"
          class="search"
          type="search"
          placeholder="搜索名称 / 备注 / IP / 端口 / 隧道"
        />
      </div>
    </div>

    <div v-if="groups.length" class="all-groups">
      <section v-for="group in groups" :key="group.group" class="group">
        <header class="group-head">
          <span class="group-mark">
            <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M2 4.5A1.5 1.5 0 0 1 3.5 3h3l1.5 2h4.5A1.5 1.5 0 0 1 14 6.5v5A1.5 1.5 0 0 1 12.5 13h-9A1.5 1.5 0 0 1 2 11.5v-7z"/></svg>
          </span>
          <div class="group-id">
            <span class="group-name">{{ group.group }}</span>
          </div>
          <span class="group-count">{{ group.rows.length }}</span>
        </header>

        <div class="tiles">
          <ForwardTile
            v-for="row in group.rows"
            :key="row.view.definition.id"
            :view="row.view"
            :server="row.server"
            :busy-key="busyKey"
            @toggle="(value: boolean) => toggle(row.view, value)"
            @edit="edit(row.server?.id, row.view.definition.id)"
            @remove="remove(row.view)"
          />
        </div>
      </section>
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
</style>

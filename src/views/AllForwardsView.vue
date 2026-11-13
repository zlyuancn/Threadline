<script setup lang="ts">
// 「所有转发」：把各个隧道下的端口转发铺平了看，并能一次搜全部。
// 与首页的区别只在"范围"——卡片本身是同一个组件，行为不会漂移。
import { computed, ref } from 'vue';
import { ElMessage } from 'element-plus';
import { useServersStore } from '../models/servers';
import { useTunnelsStore } from '../models/tunnels';
import { useSelectionStore } from '../models/selection';
import { withTimeout } from '../models/asyncAction';
import { matchesQuery, type ForwardLike } from '../models/forwardView';
import type { TunnelView } from '../api/types';
import ForwardCard from '../components/ForwardCard.vue';

const emit = defineEmits<{
  (e: 'open-tunnel', id: string): void;
  (e: 'edit-forward', serverId: string, forwardId: string): void;
}>();

const servers = useServersStore();
const forwards = useTunnelsStore();
const selection = useSelectionStore();

const query = ref('');
/** 开 = 显示全部；关 = 只看启用的（auto_start）。 */
const showAll = ref(true);
const busyKey = ref<string | null>(null);

const rows = computed(() =>
  forwards.items.map((view) => ({
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
          row.server?.name ?? '',
          row.server?.host ?? '',
        ],
        query.value,
      ),
    ),
);

/** 按隧道分组：同一份权限/会话下的转发放在一起，语义清楚。 */
const groups = computed(() => {
  const map = new Map<string, { server: (typeof servers.servers)[number] | null; rows: typeof rows.value }>();
  for (const row of filtered.value) {
    const key = row.server?.id ?? 'unknown';
    if (!map.has(key)) {
      map.set(key, { server: row.server, rows: [] });
    }
    map.get(key)!.rows.push(row);
  }
  return [...map.values()];
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

/** 点分组标题只切到该隧道的页面，不针对某条转发。 */
function openTunnel(serverId: string) {
  selection.select(serverId);
  emit('open-tunnel', serverId);
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
        <div class="eyebrow">PORT FORWARDS</div>
        <h1 class="page-title">所有转发</h1>
        <p class="section-sub">
          跨隧道查看全部 {{ rows.length }} 条转发，其中 {{ runningCount }} 条运行中。
        </p>
      </div>
      <div class="section-actions">
        <label
          :title="showAll ? '关闭后只显示卡片开关为开启的转发' : '打开后显示全部转发'"
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
      <section v-for="group in groups" :key="group.server?.id ?? 'unknown'" class="group">
        <header class="group-head">
          <button class="group-name" @click="group.server && openTunnel(group.server.id)">
            {{ group.server?.name ?? '未知隧道' }}
          </button>
          <span class="group-host mono">
            {{ group.server ? `${group.server.username}@${group.server.host}` : '' }}
          </span>
          <span class="group-count">{{ group.rows.length }}</span>
        </header>

        <div class="grid">
          <ForwardCard
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
      <div class="empty-title">没有匹配的转发</div>
      <div>换个关键词试试：名称、备注、IP、端口、隧道名都能搜。</div>
    </div>

    <div v-else class="empty">
      <img src="/logo.svg" alt="" />
      <div class="empty-title">还没有端口转发</div>
      <div>先在左侧选一条隧道，在它的详情页里新建转发。</div>
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
  margin-bottom: 10px;
}

.group-name {
  padding: 0;
  border: none;
  background: transparent;
  color: var(--text);
  font-family: inherit;
  font-size: 13.5px;
  font-weight: 650;
  cursor: pointer;
}

.group-name:hover {
  color: var(--mint);
}

.group-host {
  font-size: 10.5px;
  color: var(--text-3);
}

.group-count {
  min-width: 18px;
  height: 18px;
  padding: 0 5px;
  border-radius: 6px;
  background: rgba(255, 255, 255, 0.07);
  font-size: 10px;
  line-height: 18px;
  text-align: center;
  color: var(--text-2);
}
</style>

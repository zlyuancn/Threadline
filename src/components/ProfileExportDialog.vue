<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import { ElMessage } from 'element-plus';
import { isTauri, saveProfilePackage } from '../api/native';
import type { TunnelDto } from '../api/types';
import { useHotkeyStore } from '../models/hotkey';
import { buildProfile, packageFileName, writePackage } from '../models/profile';
import { useServersStore } from '../models/servers';
import { readStoredTheme } from '../models/theme';
import { useTunnelsStore } from '../models/tunnels';

const open = defineModel<boolean>({ required: true });

const servers = useServersStore();
const tunnels = useTunnelsStore();
const hotkey = useHotkeyStore();

/** 勾选状态用"排除法"存：默认全选，用户取消的才记下来。 */
const excludedServers = ref(new Set<string>());
const excludedForwards = ref(new Set<string>());
const includeIcons = ref(true);
const includeFingerprint = ref(true);
const includePreferences = ref(false);
const query = ref('');

const forwardsOf = (serverId: string): TunnelDto[] =>
  tunnels.items
    .map((view) => view.definition)
    .filter((forward) => forward.server_id === serverId);

const visibleServers = computed(() => {
  const keyword = query.value.trim().toLowerCase();
  if (!keyword) {
    return servers.servers;
  }
  return servers.servers.filter(
    (server) =>
      server.name.toLowerCase().includes(keyword) ||
      server.host.toLowerCase().includes(keyword) ||
      forwardsOf(server.id).some((forward) => forward.name.toLowerCase().includes(keyword)),
  );
});

const serverChecked = (id: string) => !excludedServers.value.has(id);
const forwardChecked = (id: string) => !excludedForwards.value.has(id);

/** 半选：连接本身勾着，但它下面有转发被取消。 */
function partial(id: string): boolean {
  if (!serverChecked(id)) {
    return false;
  }
  const list = forwardsOf(id);
  return list.length > 0 && list.some((forward) => !forwardChecked(forward.id));
}

function toggleServer(id: string, checked: boolean): void {
  const next = new Set(excludedServers.value);
  const list = forwardsOf(id);
  if (checked) {
    next.delete(id);
    // 勾连接 = 恢复它下面全部转发
    for (const forward of list) {
      excludedForwards.value.delete(forward.id);
    }
  } else {
    next.add(id);
  }
  excludedServers.value = next;
}

function toggleForward(serverId: string, forwardId: string, checked: boolean): void {
  const next = new Set(excludedForwards.value);
  if (checked) {
    next.delete(forwardId);
    // 勾一条转发时把所属连接也放回来
    excludedServers.value.delete(serverId);
  } else {
    next.add(forwardId);
  }
  excludedForwards.value = next;
}

const selectedServerIds = computed(() =>
  servers.servers.filter((server) => serverChecked(server.id)).map((server) => server.id),
);

const selectedForwardIds = computed(() =>
  tunnels.items
    .map((view) => view.definition)
    .filter((forward) => forwardChecked(forward.id))
    .map((forward) => forward.id),
);

/** 实际会进包的数量：连接被勾时它下面全部转发都算。 */
const counts = computed(() => {
  let forwardCount = 0;
  for (const server of servers.servers) {
    if (!serverChecked(server.id)) {
      continue;
    }
    forwardCount += forwardsOf(server.id).filter((f) => forwardChecked(f.id)).length;
  }
  return { servers: selectedServerIds.value.length, forwards: forwardCount };
});

/** 关掉图标/指纹只影响体积与信任，不改结构。 */
const estimate = computed(() => {
  if (!includeIcons.value) {
    return 'structure only';
  }
  return `${counts.value.forwards} icons max`;
});

function selectAll(): void {
  excludedServers.value = new Set();
  excludedForwards.value = new Set();
}

const exporting = ref(false);

async function doExport(): Promise<void> {
  if (counts.value.servers === 0) {
    ElMessage.warning('至少选一条连接');
    return;
  }
  exporting.value = true;
  try {
    const built = buildProfile({
      servers: servers.servers,
      forwards: tunnels.items.map((view) => view.definition),
      serverIds: selectedServerIds.value,
      forwardIds: selectedForwardIds.value,
      includeIcons: includeIcons.value,
      includeFingerprint: includeFingerprint.value,
      ...(includePreferences.value
        ? { preferences: { theme: readStoredTheme(), hotkey: hotkey.saved } }
        : {}),
    });
    const bytes = writePackage(built);
    const name = packageFileName();
    // 桌面端走系统保存对话框：让用户自己选目录，并回显最终落点
    const saved = await saveProfilePackage(bytes, name);
    if (saved) {
      ElMessage.success(
        `已导出 ${counts.value.servers} 条连接 / ${counts.value.forwards} 条转发 → ${saved}`,
      );
      open.value = false;
      return;
    }
    if (isTauri()) {
      // 用户在对话框里点了取消：不要假装成功
      return;
    }
    // 浏览器调试（没有 Tauri）：退回普通下载
    const blob = new Blob([bytes as BlobPart], { type: 'application/octet-stream' });
    const url = URL.createObjectURL(blob);
    const link = document.createElement('a');
    link.href = url;
    link.download = name;
    link.click();
    URL.revokeObjectURL(url);
    ElMessage.success(`已导出 ${counts.value.servers} 条连接 / ${counts.value.forwards} 条转发`);
    open.value = false;
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : String(e));
  } finally {
    exporting.value = false;
  }
}

watch(open, (value) => {
  if (value) {
    // 每次打开都回到"全选"：导出多数时候就是要全部
    excludedServers.value = new Set();
    excludedForwards.value = new Set();
    query.value = '';
  }
});
</script>

<template>
  <el-dialog v-model="open" title="导出配置" width="640px">
    <p class="hint" style="margin-top: 0">
      勾连接 = 连它下面的转发一起带走；只勾转发时，所属连接会自动补上。
      <strong>包里不含密码、私钥</strong>，对方导入后需要自己填。
      点「导出」后会弹出保存位置，自己选存到哪。
    </p>

    <div class="pf-toolbar">
      <input v-model="query" class="pf-search" placeholder="搜索连接或转发" />
      <button class="btn" @click="selectAll">全选</button>
    </div>

    <ul class="pf-tree">
      <li v-for="server in visibleServers" :key="server.id" class="pf-node">
        <label class="pf-line">
          <el-checkbox
            :model-value="serverChecked(server.id)"
            :indeterminate="partial(server.id)"
            @change="toggleServer(server.id, Boolean($event))"
          />
          <span class="pf-name">{{ server.name }}</span>
          <span class="hint">{{ server.username }}@{{ server.host }}:{{ server.port }}</span>
          <span class="pf-count">{{ forwardsOf(server.id).length }} 条转发</span>
        </label>
        <ul v-if="forwardsOf(server.id).length" class="pf-sub">
          <li v-for="forward in forwardsOf(server.id)" :key="forward.id">
            <label class="pf-line">
              <el-checkbox
                :model-value="forwardChecked(forward.id)"
                @change="toggleForward(server.id, forward.id, Boolean($event))"
              />
              <span class="pf-name">{{ forward.name }}</span>
              <span class="hint">
                {{ forward.local_host }}:{{ forward.local_port }} →
                {{ forward.remote_host }}:{{ forward.remote_port }}
              </span>
              <span class="pf-badge">{{ forward.scheme }}</span>
            </label>
          </li>
        </ul>
      </li>
    </ul>

    <div class="pf-options">
      <el-checkbox v-model="includeIcons">包含自定义图标</el-checkbox>
      <el-checkbox v-model="includeFingerprint">包含已信任的主机指纹</el-checkbox>
      <el-checkbox v-model="includePreferences">包含本机偏好（主题、呼出快捷键）</el-checkbox>
    </div>
    <p class="hint">
      关掉指纹后，对方首次连接会重新确认服务端身份；关掉图标不影响转发本身。
    </p>

    <template #footer>
      <span class="hint" style="margin-right: auto">
        已选 {{ counts.servers }} 条连接 / {{ counts.forwards }} 条转发（{{ estimate }}）
      </span>
      <button class="btn" @click="open = false">取消</button>
      <button class="btn is-primary" :disabled="exporting || !counts.servers" @click="doExport">
        {{ exporting ? '导出中…' : '导出' }}
      </button>
    </template>
  </el-dialog>
</template>

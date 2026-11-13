<script setup lang="ts">
import { computed, reactive, ref, watch } from 'vue';
import { ElMessage } from 'element-plus';
import { profileApi } from '../api/profile';
import type {
  ProfileImportRequest,
  ProfileImportResult,
  ProfileImportServer,
  ProfilePlan,
} from '../api/types';
import { applyPreferences, diffPreferences, readPreferences } from '../models/preferences';
import { readPackage, type ParsedPackage } from '../models/profile';
import { useServersStore } from '../models/servers';
import { useTunnelsStore } from '../models/tunnels';

const open = defineModel<boolean>({ required: true });

const servers = useServersStore();
const tunnels = useTunnelsStore();

type Step = 'pick' | 'review' | 'done';

const step = ref<Step>('pick');
const fileName = ref('');
const parsed = ref<ParsedPackage | null>(null);
const plan = ref<ProfilePlan | null>(null);
const busy = ref(false);
const result = ref<ProfileImportResult | null>(null);
const fileInput = ref<HTMLInputElement | null>(null);

/** key 形如 `s:0`、`f:0:2`：直接以 plan 里的下标定位。 */
const actions = reactive<Record<string, string>>({});
const ports = reactive<Record<string, number>>({});
const names = reactive<Record<string, string>>({});

const keepLocalPreferences = ref(true);

const localPreferences = ref(readPreferences());

const preferenceDiff = computed(() =>
  diffPreferences(localPreferences.value, parsed.value?.document.preferences),
);

const hasDomain = computed(() =>
  (parsed.value?.document.servers ?? []).some((server) =>
    server.forwards?.some((forward) => !!forward.domain),
  ),
);

function reset(): void {
  step.value = 'pick';
  fileName.value = '';
  parsed.value = null;
  plan.value = null;
  result.value = null;
  for (const key of Object.keys(actions)) delete actions[key];
  for (const key of Object.keys(ports)) delete ports[key];
  for (const key of Object.keys(names)) delete names[key];
  if (fileInput.value) {
    fileInput.value.value = '';
  }
}

async function onPick(event: Event): Promise<void> {
  const input = event.target as HTMLInputElement;
  const file = input.files?.[0];
  if (!file) {
    return;
  }
  busy.value = true;
  try {
    const bytes = new Uint8Array(await file.arrayBuffer());
    const pkg = await readPackage(bytes);
    parsed.value = pkg;
    fileName.value = file.name;
    localPreferences.value = readPreferences();
    const planned = await profileApi.plan(pkg.document.servers);
    plan.value = planned;
    seedsActions(planned);
    step.value = 'review';
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : String(e));
  } finally {
    busy.value = false;
  }
}

/** 默认动作：能建就建，冲突一律先跳过——重复导入最怕"悄悄覆盖"。 */
function seedsActions(planned: ProfilePlan): void {
  for (const server of planned.servers) {
    actions[`s:${server.index}`] = server.action === 'ok' ? 'create' : 'skip';
    if (server.action === 'name_taken' && server.suggested_name) {
      names[`s:${server.index}`] = server.suggested_name;
    }
    for (const forward of server.forwards) {
      const key = `f:${server.index}:${forward.index}`;
      actions[key] = forward.action === 'ok' ? 'create' : 'skip';
    }
  }
}

function serverOptions(serverIndex: number): { label: string; value: string }[] {
  const action = plan.value?.servers[serverIndex]?.action;
  const options = [
    { label: '新建', value: 'create' },
    { label: '跳过', value: 'skip' },
  ];
  // 覆盖只在「同一条」时才有意义：id 不同时覆盖会落空
  if (action === 'exists') {
    options.push({ label: '覆盖', value: 'overwrite' });
  }
  return options;
}

function forwardOptions(serverIndex: number, forwardIndex: number): { label: string; value: string }[] {
  const action = plan.value?.servers[serverIndex]?.forwards[forwardIndex]?.action;
  const options = [
    { label: '新建', value: 'create' },
    { label: '跳过', value: 'skip' },
  ];
  if (action === 'name_taken') {
    options.push({ label: '覆盖', value: 'overwrite' });
  }
  return options;
}

function portValue(serverIndex: number, forwardIndex: number): number {
  const key = `f:${serverIndex}:${forwardIndex}`;
  if (ports[key] !== undefined) {
    return ports[key];
  }
  return parsed.value?.document.servers[serverIndex]?.forwards[forwardIndex]?.local_port ?? 0;
}

/** 名字重复时按名字找现有转发：`target_id` 只用于覆盖。 */
function existingForwardId(name: string): string | null {
  const hit = tunnels.items.find((view) => view.definition.name === name);
  return hit ? hit.definition.id : null;
}

function buildRequest(): ProfileImportRequest {
  const source = parsed.value!.document.servers;
  const out: ProfileImportServer[] = [];
  for (const planned of plan.value!.servers) {
    const index = planned.index;
    const server = source[index];
    const action = (actions[`s:${index}`] ?? 'skip') as 'create' | 'overwrite' | 'skip';
    out.push({
      source_id: server.id,
      name: names[`s:${index}`] ?? server.name,
      host: server.host,
      port: server.port,
      username: server.username,
      credential_kind: server.credential_kind,
      keepalive_secs: server.keepalive_secs,
      // 覆盖时忽略；新建时采用分享者的指纹
      host_key_fingerprint: server.host_key_fingerprint ?? null,
      action,
      forwards: (server.forwards ?? []).map((forward, forwardIndex) => {
        const key = `f:${index}:${forwardIndex}`;
        const forwardAction = (actions[key] ?? 'skip') as 'create' | 'overwrite' | 'skip';
        const iconPath = forward.icon;
        return {
          name: forward.name,
          local_host: forward.local_host,
          local_port: ports[key] ?? forward.local_port,
          remote_host: forward.remote_host,
          remote_port: forward.remote_port,
          scheme: forward.scheme,
          domain: forward.domain,
          remote_scheme: (forward.remote_scheme as 'http' | 'https' | null) ?? null,
          open_path: forward.open_path,
          note: forward.note,
          group: forward.group,
          in_app: forward.in_app,
          auto_start: forward.auto_start,
          icon: iconPath ? (parsed.value!.icons.get(iconPath) ?? '') : '',
          action: forwardAction,
          target_id:
            forwardAction === 'overwrite' ? existingForwardId(forward.name) : null,
        };
      }),
    });
  }
  // 固定停用：包里没有密码，启用必然失败（Core 会返回 credential_missing）。
  // 给用户一个「导入后自动连接」的开关只会让连接立刻进入失败状态。
  return { servers: out, keep_disabled: true };
}

async function doImport(): Promise<void> {
  if (!parsed.value || !plan.value) {
    return;
  }
  busy.value = true;
  try {
    const res = await profileApi.import(buildRequest());
    result.value = res;

    // 偏好写回放在落地之后：连接都没导进来时不该改用户的界面设置
    if (parsed.value.document.preferences && !keepLocalPreferences.value) {
      const failures = await applyPreferences(parsed.value.document.preferences);
      for (const failure of failures) {
        ElMessage.warning(failure);
      }
    }

    step.value = 'done';
    await Promise.all([servers.refresh(), tunnels.refresh()]);
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : String(e));
  } finally {
    busy.value = false;
  }
}

function backToReview(): void {
  result.value = null;
  step.value = 'review';
}

watch(open, (value) => {
  if (value) {
    reset();
  }
});
</script>

<template>
  <el-dialog v-model="open" title="导入配置" width="680px">
    <template v-if="step === 'pick'">
      <p class="hint" style="margin-top: 0">
        选择一个 <code>.threadline</code> 文件。包里不含密码/私钥，
        导入后连接会标成「待填密码」，编辑里补一次即可。
      </p>
      <input
        ref="fileInput"
        type="file"
        accept=".threadline"
        :disabled="busy"
        @change="onPick"
      />
    </template>

    <template v-else-if="step === 'review'">
      <p class="hint" style="margin-top: 0">
        {{ fileName }} · 逐条确认去向，重复项默认跳过。
        <span v-if="hasDomain">含域名的条目导入后需到设置里同步一次 hosts。</span>
      </p>

      <ul class="pf-review">
        <li v-for="server in plan?.servers ?? []" :key="server.index" class="pf-review-row">
          <div class="pf-review-main">
            <span class="pf-name">{{ server.name }}</span>
            <span class="hint">
              {{ parsed?.document.servers[server.index]?.username }}@{{
                parsed?.document.servers[server.index]?.host
              }}:{{ parsed?.document.servers[server.index]?.port }}
            </span>
            <span v-if="server.reason" class="pf-reason">{{ server.reason }}</span>
          </div>
          <div class="pf-review-side">
            <input
              v-if="server.action === 'name_taken'"
              v-model="names[`s:${server.index}`]"
              class="pf-search pf-port"
              style="width: 140px"
              placeholder="改名后导入"
            />
            <el-select v-model="actions[`s:${server.index}`]" size="small" style="width: 96px">
              <el-option
                v-for="option in serverOptions(server.index)"
                :key="option.value"
                :label="option.label"
                :value="option.value"
              />
            </el-select>
          </div>

          <ul v-if="server.forwards.length" class="pf-sub-review">
            <li
              v-for="forward in server.forwards"
              :key="forward.index"
              class="pf-review-row"
            >
              <div class="pf-review-main">
                <span>{{ forward.name }}</span>
                <span class="hint">
                  {{ parsed?.document.servers[server.index]?.forwards[forward.index]?.local_host }}:
                  {{ portValue(server.index, forward.index) }} →
                  {{ parsed?.document.servers[server.index]?.forwards[forward.index]?.remote_host }}:{{
                    parsed?.document.servers[server.index]?.forwards[forward.index]?.remote_port
                  }}
                </span>
                <span v-if="forward.reason" class="pf-reason">{{ forward.reason }}</span>
              </div>
              <div class="pf-review-side">
                <input
                  v-if="forward.action === 'port_taken'"
                  :value="portValue(server.index, forward.index)"
                  type="number"
                  class="pf-search pf-port"
                  style="width: 92px"
                  @input="
                    ports[`f:${server.index}:${forward.index}`] = Number(
                      ($event.target as HTMLInputElement).value,
                    )
                  "
                />
                <el-select
                  v-model="actions[`f:${server.index}:${forward.index}`]"
                  size="small"
                  style="width: 96px"
                >
                  <el-option
                    v-for="option in forwardOptions(server.index, forward.index)"
                    :key="option.value"
                    :label="option.label"
                    :value="option.value"
                  />
                </el-select>
              </div>
            </li>
          </ul>
        </li>
      </ul>

      <div class="pf-options">
        <el-checkbox v-model="keepLocalPreferences">不覆盖本机偏好</el-checkbox>
      </div>
      <p class="hint">
        导入的连接没有密码，会保持停用；在「编辑」里填好密码后再自己打开。
      </p>

      <div v-if="preferenceDiff.length && !keepLocalPreferences" class="pf-options">
        <p class="hint" style="margin: 0 0 4px; width: 100%">将覆盖本机设置：</p>
        <p v-for="line in preferenceDiff" :key="line.key" class="hint" style="margin: 0; width: 100%">
          {{ line.label }}
        </p>
      </div>
    </template>

    <template v-else>
      <p class="hint" style="margin-top: 0">
        完成：写入 {{ result?.created_servers ?? 0 }} 条连接 /
        {{ result?.created_forwards ?? 0 }} 条转发，跳过 {{ result?.skipped ?? 0 }} 条。
      </p>
      <ul v-if="result?.failed.length" class="pf-review">
        <li v-for="failure in result.failed" :key="failure.name" class="pf-review-row">
          <div class="pf-review-main">
            <span class="pf-name">{{ failure.name }}</span>
          </div>
          <span class="pf-reason">{{ failure.reason }}</span>
        </li>
      </ul>
      <p v-if="!result?.failed.length" class="hint">
        导入的连接没有密码，编辑里填一次就能连。
      </p>
    </template>

    <template #footer>
      <template v-if="step === 'review'">
        <button class="btn" :disabled="busy" @click="open = false">取消</button>
        <button class="btn is-primary" :disabled="busy" @click="doImport">
          {{ busy ? '导入中…' : '开始导入' }}
        </button>
      </template>
      <template v-else-if="step === 'done'">
        <button class="btn" @click="backToReview">再检查一次</button>
        <button class="btn is-primary" @click="open = false">完成</button>
      </template>
    </template>
  </el-dialog>
</template>

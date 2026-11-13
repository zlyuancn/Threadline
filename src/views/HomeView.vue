<script setup lang="ts">
// 一个页面装两层：隧道（一条 SSH 连接，自带登录信息）与其下的端口转发。
// 界面上没有「凭据」这一层——用户名和密钥在新建隧道时一起填。
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue';
import { ElMessage } from 'element-plus';
import { useTunnelsStore } from '../models/tunnels';
import { useServersStore } from '../models/servers';
import { withTimeout } from '../models/asyncAction';
import { matchesQuery, type ForwardLike } from '../models/forwardView';
import { formatRate, useTrafficStore } from '../models/traffic';
import ForwardCard from '../components/ForwardCard.vue';

import { useSelectionStore } from '../models/selection';
import {
  importIcon,
  isTauri,
  knownHostsFingerprint,
  listSshKeys,
  readLocalPrivateKey,
} from '../api/native';
import type { CredentialKind, CreateServerRequest } from '../api/types';

const forwards = useTunnelsStore();
const tunnels = useServersStore();
// 隧道列表在侧栏（App.vue），选中态必须共享，否则点左边右边不动。
const selection = useSelectionStore();

const props = defineProps<{
  /** 从「应用」「所有转发」跳转过来：滚动到这张卡并打开它的编辑弹窗。 */
  focusForward?: { id: string; nonce: number } | null;
}>();

const emit = defineEmits<{ (e: 'focus-handled'): void }>();

const busyKey = ref<string | null>(null);
/**
 * 本地端口是否被用户手动改过。
 *
 * 需求是"远端端口改了、而本地端口还没动过，就跟着变"——所以必须记住这个意图，
 * 而不是比较两个数字相不相等（那样用户手动填成同一个值时会被误判为没动过）。
 */
const localPortTouched = ref(false);

/** 新建时默认跟远端端口走：同一个服务通常两边端口相同，少一次输入。 */
function followRemotePort(value: number | undefined) {
  if (localPortTouched.value || typeof value !== 'number') {
    return;
  }
  forwardForm.value.local_port = value;
}

function onLocalPortEdited() {
  localPortTouched.value = true;
}

/**
 * 本地协议变了：入口端口按格给默认值（① 80 / ② 443 / ③ 远端端口）。
 *
 * 不能统一用 remote_port：② 的远端是 80，默认填 80 会让 `https://域名` 直接连不上。
 */
const DEFAULT_LOCAL_PORT: Record<Scheme, number | null> = {
  http: 80,
  https: 443,
  rdp: 3389,
  ssh: 22,
  tcp: null,
};

/** 远端端口的协议默认值：选 RDP 就该是 3389，选 SSH 就该是 22。 */
const DEFAULT_REMOTE_PORT: Partial<Record<Scheme, number>> = {
  http: 8080,
  https: 443,
  rdp: 3389,
  ssh: 22,
};

/** 这些值视为「没改过」，切协议时允许被覆盖；用户填的其它端口一律保留。 */
const COMMON_REMOTE_PORTS = new Set([22, 80, 443, 3389, 8080]);

function onSchemeChanged(value: Scheme) {
  // 切协议是明确动作：端口必须跟着协议走（HTTP→HTTPS 就是 80→443），
  // 不能因为之前手填过一次就保留旧值，否则 `https://域名:80` 会直接连不上。
  const form = forwardForm.value;
  const remotePreset = DEFAULT_REMOTE_PORT[value];
  if (remotePreset !== undefined && COMMON_REMOTE_PORTS.has(form.remote_port)) {
    form.remote_port = remotePreset;
  }
  const preset = DEFAULT_LOCAL_PORT[value];
  form.local_port = preset ?? form.remote_port;
}

/** 只有 http/https 才有远端协议与「打开路径」这两回事。 */
function isWebScheme(value: Scheme): boolean {
  return value === 'http' || value === 'https';
}

/** 远端是 https 时本地必须也是 https；只在填了域名时生效（无域名是纯字节管道）。 */
function invalidCombo(): boolean {
  const form = forwardForm.value;
  return (
    form.domain.trim() !== '' && form.scheme === 'http' && form.remote_scheme === 'https'
  );
}

/** 随机一个本地端口：避开本机已被其它转发占用的那些。 */
function randomLocalPort() {
  const taken = new Set(
    forwards.items
      .filter((item) => item.definition.id !== forwardForm.value.id)
      .map((item) => item.definition.local_port),
  );
  for (let attempt = 0; attempt < 200; attempt += 1) {
    const candidate = 20000 + Math.floor(Math.random() * 40000);
    if (!taken.has(candidate)) {
      forwardForm.value.local_port = candidate;
      localPortTouched.value = true;
      return;
    }
  }
  ElMessage.warning('没找到空闲端口，请手动填一个');
}

/** 端口转发搜索词：名称 / 备注 / IP / 端口。 */
const query = ref('');

/** 混合表格的数据：隧道是父行，端口是子行。 */
const rows = computed(() => {
  return tunnels.servers.map((server) => ({
    key: `t:${server.id}`,
    rowType: 'tunnel' as const,
    server,
    children: forwards.items
      .filter((item) => item.definition.server_id === server.id)
      .map((item) => ({
        key: `f:${item.definition.id}`,
        rowType: 'forward' as const,
        view: item,
      })),
  }));
});

async function run(key: string, work: () => Promise<void>) {
  if (busyKey.value) {
    return;
  }
  busyKey.value = key;
  try {
    await withTimeout(work(), 30_000, '操作超时：核心没有响应');
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : String(e));
  } finally {
    busyKey.value = null;
  }
}

/* ───────────── 隧道 ───────────── */

interface TunnelForm {
  id: string | null;
  name: string;
  host: string;
  port: number;
  username: string;
  kind: CredentialKind;
  secret: string;
  passphrase: string;
  keepalive_secs: number;
}

const tunnelDialog = ref(false);
const tunnelSaving = ref(false);
const tunnelTesting = ref(false);
const tunnelForm = ref<TunnelForm>(emptyTunnel());

/**
 * 测试连接：依次探 TCP、SSH 握手、认证，把卡在哪一步告诉用户。
 *
 * 已保存的连接用后端存的凭据；还没保存的（新建）用表单里的值直接探，
 * 密码只在这一次调用里用，不落库。
 */
async function testTunnel() {
  const form = tunnelForm.value;
  if (tunnelTesting.value) {
    return;
  }
  tunnelTesting.value = true;
  try {
    const report = form.id
      ? await tunnels.probeServer(form.id)
      : await tunnels.probeDraft({
          host: form.host,
          port: form.port,
          username: form.username,
          credential_kind: form.kind,
          secret: form.secret,
          passphrase: form.passphrase === '' ? null : form.passphrase,
          host_key_fingerprint: null,
        });
    if (!report) {
      ElMessage.error(tunnels.lastError || '测试失败');
      return;
    }
    if (report.stage === 'ok') {
      ElMessage.success(`连接正常：${report.message}`);
    } else {
      ElMessage.warning(report.message);
    }
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : String(e));
  } finally {
    tunnelTesting.value = false;
  }
}

function emptyTunnel(): TunnelForm {
  return {
    id: null,
    name: '',
    host: '',
    port: 22,
    username: 'root',
    kind: 'password',
    secret: '',
    passphrase: '',
    keepalive_secs: 30,
  };
}

function openTunnel(row?: {
  id: string;
  name: string;
  host: string;
  port: number;
  username: string;
  credential_kind?: CredentialKind;
  keepalive_secs: number;
}) {
  if (row) {
    tunnelForm.value = {
      id: row.id,
      name: row.name,
      host: row.host,
      port: row.port,
      username: row.username,
      kind: row.credential_kind ?? 'password',
      secret: '',
      passphrase: '',
      keepalive_secs: row.keepalive_secs,
    };
  } else {
    tunnelForm.value = emptyTunnel();
  }
  tunnelDialog.value = true;
}

async function saveTunnel() {
  const form = tunnelForm.value;
  if (tunnelSaving.value) {
    return;
  }
  if (!form.name.trim() || !form.host.trim() || !form.username.trim()) {
    ElMessage.warning('名称、主机、用户名都要填');
    return;
  }
  if (!form.id && !form.secret.trim()) {
    ElMessage.warning('请填写密码，或粘贴私钥');
    return;
  }

  tunnelSaving.value = true;
  try {
    const ok = form.id
      ? await tunnels.updateServer(form.id, {
          name: form.name,
          host: form.host,
          port: form.port,
          username: form.username,
          kind: form.kind,
          secret: form.secret.trim() === '' ? undefined : form.secret,
          passphrase: form.passphrase === '' ? undefined : form.passphrase,
          keepalive_secs: form.keepalive_secs,
        })
      : await tunnels.createServer({
          name: form.name,
          host: form.host,
          port: form.port,
          username: form.username,
          kind: form.kind,
          secret: form.secret,
          passphrase: form.passphrase === '' ? null : form.passphrase,
          host_key_fingerprint: null,
          keepalive_secs: form.keepalive_secs,
          enabled: false,
        } as CreateServerRequest);

    if (!ok) {
      ElMessage.error(tunnels.lastError);
      return;
    }
    ElMessage.success(form.id ? '隧道已更新' : '隧道已创建');
    tunnelDialog.value = false;
  } finally {
    tunnelSaving.value = false;
  }
}

/** `~/.ssh` 下有哪些私钥：打开下拉时刷新一次。 */
const localKeys = ref<{ name: string; path: string }[]>([]);

async function loadLocalKeys() {
  localKeys.value = await listSshKeys();
}

async function importKey(name: string) {
  try {
    const key = await readLocalPrivateKey(name);
    tunnelForm.value.secret = key.pem;
    tunnelForm.value.kind = 'private_key';
    ElMessage.success(`已填入 ${name}`);
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : String(e));
  }
}

async function toggleTunnel(row: { id: string; name: string; enabled: boolean }, enabled: boolean) {
  if (busyKey.value) {
    return;
  }
  busyKey.value = `t:${row.id}`;
  try {
    const affected = await withTimeout(
      tunnels.setServerEnabled(row.id, enabled),
      30_000,
      '操作超时：核心没有响应',
    );
    if (affected === null) {
      // 导入的连接没有密码：这是「还没配置完」，不是故障，别用红色错误吓人
      if (tunnels.lastError.includes('还没填密码')) {
        ElMessage.warning('这条隧道还没填密码：先点「编辑」填一次密码或私钥，再打开');
        return;
      }
      if (tunnels.lastError.includes('待确认')) {
        // 首次连接必须先确认主机密钥：开关保持"开"，指纹与按钮显示在状态列。
        ElMessage.warning('首次连接需要确认主机密钥：指纹已显示在状态列，点「信任并记录」即可');
        await tunnels.refresh();
      } else {
        ElMessage.error(tunnels.lastError);
      }
      return;
    }
    if (!enabled) {
      ElMessage.success(
        affected > 0 ? `已关闭「${row.name}」，${affected} 条端口随之停止` : `已关闭「${row.name}」`,
      );
    } else {
      ElMessage.success(
        affected > 0 ? `「${row.name}」已连接，恢复 ${affected} 条端口` : `「${row.name}」已连接`,
      );
    }
  } finally {
    busyKey.value = null;
  }
}

const knownNote = ref<Record<string, { tone: string; text: string }>>({});

/** 用本机 known_hosts 记录做参照物（终端 ssh 过一次就有）。 */
async function checkKnown(server: { id: string; host: string; port: number }) {
  const pending = tunnels.pendingFingerprint(server.id);
  const lookup = await knownHostsFingerprint(server.host, server.port);
  if (lookup.found && lookup.fingerprint === pending) {
    knownNote.value[server.id] = { tone: 'ok', text: '✓ 与本机 known_hosts 一致，可以信任' };
  } else if (lookup.found) {
    knownNote.value[server.id] = {
      tone: 'bad',
      text: `✗ 与本机记录不一致（本机记 ${lookup.fingerprint?.slice(0, 24)}…），请勿信任`,
    };
  } else if (lookup.hashed) {
    knownNote.value[server.id] = { tone: 'dim', text: '本机 known_hosts 是哈希格式，无法直接比对' };
  } else {
    knownNote.value[server.id] = {
      tone: 'dim',
      text: '本机没有这台的记录：用终端 ssh 连一次就会有，或到服务器上执行 ssh-keygen -lf',
    };
  }
}

/** 信任/拒绝指纹。信任后直接把隧道打开——用户本来就是要连它。 */
async function trustKey(server: { id: string; name: string }, reject: boolean) {
  await run(`t:${server.id}`, async () => {
    const ok = await tunnels.acceptHostKey(server.id, reject);
    if (!ok) {
      ElMessage.error(tunnels.lastError);
      return;
    }
    if (reject) {
      ElMessage.success('已拒绝该指纹');
      return;
    }
    const affected = await tunnels.setServerEnabled(server.id, true);
    ElMessage.success(
      affected && affected > 0
        ? `已信任并连接「${server.name}」，恢复 ${affected} 条端口`
        : `已信任并连接「${server.name}」`,
    );
  });
}

async function removeTunnel(row: { id: string; name: string }) {
  // 隧道下面挂着转发时，光删隧道会被拦下来。这里把"连带删除"摆到台面上：
  // 用户点一次就完成，但要先知道会连带删掉哪些。
  const owned = forwards.items.filter((item) => item.definition.server_id === row.id);
  const detail = owned.map((item) => item.definition.name).join('、');
  const question = owned.length
    ? `删除隧道「${row.name}」，会同时删除它下面的 ${owned.length} 条端口转发（${detail}）。继续？`
    : `删除隧道「${row.name}」？`;
  if (!window.confirm(question)) {
    return;
  }

  await run(`t:${row.id}`, async () => {
    for (const item of owned) {
      await forwards.remove(item.definition.id);
      if (forwards.lastError) {
        ElMessage.error(forwards.lastError);
        return;
      }
    }
    await tunnels.removeServer(row.id);
    if (tunnels.lastError) {
      ElMessage.error(tunnels.lastError);
    } else {
      ElMessage.success(
        owned.length
          ? `已删除「${row.name}」及其 ${owned.length} 条转发`
          : `已删除「${row.name}」`,
      );
    }
    await forwards.refresh();
  });
}

/* ───────────── 端口转发 ───────────── */

/** 本机用哪种协议访问；rdp/ssh 本质是 TCP，只是默认端口不同。 */
type Scheme = 'tcp' | 'http' | 'https' | 'rdp' | 'ssh';

interface ForwardForm {
  id: string | null;
  name: string;
  server_id: string;
  local_host: string;
  local_port: number;
  remote_host: string;
  remote_port: number;
  enabled: boolean;
  /** 备注：显示在卡片名称后面，也参与搜索。 */
  note: string;
  /** 「打开本地页面」的路径，仅 http/https 有意义。 */
  open_path: string;
  /** 本地这一端用什么协议访问；只有 http/https 提供「打开本地页面」。 */
  scheme: Scheme;
  /** 访问域名；空 = 不用域名（与老版本完全一致）。 */
  domain: string;
  /** 远端实际协议；空串 = 由本地协议推导。 */
  remote_scheme: 'http' | 'https' | '';
  /** 自定义图标（导入后存成 data:image）；空 = 按入口自动取 favicon。 */
  icon: string;
  /** 自定义分组；空 = 未分组。 */
  group: string;
  /** 是否出现在「应用」页。 */
  in_app: boolean;
}

const forwardDialog = ref(false);
const forwardSaving = ref(false);
const forwardTesting = ref(false);

/**
 * 测试连接：确认这条转发所属的服务器能 SSH 登录。
 *
 * 远端端口是否可达取决于隧道启动后的实际转发，这里只覆盖「连不上服务器」这一类问题。
 */
async function testForward() {
  const form = forwardForm.value;
  if (!form.server_id) {
    ElMessage.info('先选择一条连接');
    return;
  }
  if (forwardTesting.value) {
    return;
  }
  forwardTesting.value = true;
  try {
    const report = await tunnels.probeServer(form.server_id);
    if (!report) {
      return;
    }
    if (report.stage === 'ok') {
      ElMessage.success(`连接正常：${report.message}`);
    } else {
      ElMessage.warning(report.message);
    }
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : String(e));
  } finally {
    forwardTesting.value = false;
  }
}
const forwardForm = ref<ForwardForm>(emptyForward());
/** 填了域名就不再由用户决定绑定地址：后端会分配 127.0.0.2–254。 */
const usesDomain = computed(() => forwardForm.value.domain.trim() !== '');

/** 表单里「分组」的建议项：从现有转发里收，允许直接输入新分组（个人数据不进代码）。 */
const groupOptions = computed(() => {
  const seen = new Set<string>();
  for (const item of forwards.items) {
    const group = item.definition.group?.trim();
    if (group) {
      seen.add(group);
    }
  }
  return [...seen].sort((a, b) => a.localeCompare(b, 'zh-Hans-CN'));
});

// 域名从空变非空：立即把入口端口切到该协议的默认值（用户手动改过就不动）。
watch(
  () => forwardForm.value.domain,
  (next, prev) => {
    if (!prev?.trim() && next.trim()) {
      onSchemeChanged(forwardForm.value.scheme);
    }
  },
);

function emptyForward(serverId = ''): ForwardForm {
  return {
    id: null,
    name: '',
    server_id: serverId,
    local_host: '127.0.0.1',
    local_port: 18080,
    remote_host: '127.0.0.1',
    remote_port: 8080,
    note: '',
    open_path: '',
    scheme: 'http',
    domain: '',
    remote_scheme: '',
    icon: '',
    group: '',
    in_app: true,
    enabled: true,
  };
}

/* ───────────── 图标导入 ───────────── */

/** 图标导入框里临时粘贴的远程地址；导入成功后清空。 */
const iconUrl = ref('');
const iconBusy = ref(false);

/**
 * 把任意图片地址转成 128px 的 data URI。
 *
 * 图标必须随配置走本地：远程地址一旦失效、离线或站点换图，导航页就花了。
 */
async function iconToLocal(url: string): Promise<string> {
  const { mime, base64 } = await importIcon(url);
  return shrinkDataUrl(`data:${mime};base64,${base64}`);
}

/** 缩到 128×128（保持比例），PNG 留透明；结果通常 < 20KB。 */
function shrinkDataUrl(dataUrl: string): Promise<string> {
  return new Promise((resolve, reject) => {
    const image = new Image();
    image.onload = () => {
      const size = 128;
      const canvas = document.createElement('canvas');
      canvas.width = size;
      canvas.height = size;
      const ctx = canvas.getContext('2d');
      if (!ctx) {
        reject(new Error('当前环境不支持 canvas'));
        return;
      }
      // 允许放大：不少站点的图标只有 32/64px，不放大在磁贴里会缩成一个小点。
      const scale = Math.min(size / image.width, size / image.height);
      const width = Math.max(1, Math.round(image.width * scale));
      const height = Math.max(1, Math.round(image.height * scale));
      ctx.drawImage(image, (size - width) / 2, (size - height) / 2, width, height);
      resolve(canvas.toDataURL('image/png'));
    };
    image.onerror = () => reject(new Error('图片无法解析'));
    image.src = dataUrl;
  });
}

async function importIconFromUrl() {
  const url = iconUrl.value.trim();
  if (!url || iconBusy.value) {
    return;
  }
  iconBusy.value = true;
  try {
    forwardForm.value.icon = await iconToLocal(url);
    iconUrl.value = '';
    ElMessage.success('图标已存到本地');
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : String(e));
  } finally {
    iconBusy.value = false;
  }
}

async function importIconFromFile(event: Event) {
  const input = event.target as HTMLInputElement;
  const file = input.files?.[0];
  input.value = '';
  if (!file) {
    return;
  }
  if (!file.type.startsWith('image/')) {
    ElMessage.warning('请选择图片文件');
    return;
  }
  try {
    const dataUrl = await new Promise<string>((resolve, reject) => {
      const reader = new FileReader();
      reader.onload = () => resolve(String(reader.result));
      reader.onerror = () => reject(new Error('读取文件失败'));
      reader.readAsDataURL(file);
    });
    forwardForm.value.icon = await shrinkDataUrl(dataUrl);
    ElMessage.success('图标已存到本地');
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : String(e));
  }
}

function openForward(
  row?: {
    id: string;
    name: string;
    server_id: string;
    local_host: string;
    local_port: number;
    remote_host: string;
    remote_port: number;
    auto_start: boolean;
    note?: string;
    open_path?: string;
    scheme?: Scheme;
    domain?: string;
    remote_scheme?: 'http' | 'https' | null;
    icon?: string;
    group?: string;
    in_app?: boolean;
  },
  serverId?: string,
) {
  localPortTouched.value = !!row;
  if (row) {
    // 定义里叫 auto_start，表单里叫 enabled：两者是同一个意图，改名在这里做，
    // 否则编辑端口时「启用」会一直是关的（之前就是这样）。
    forwardForm.value = {
      id: row.id,
      name: row.name,
      server_id: row.server_id,
      local_host: row.local_host,
      local_port: row.local_port,
      remote_host: row.remote_host,
      remote_port: row.remote_port,
      enabled: row.auto_start,
      note: row.note ?? '',
      open_path: row.open_path ?? '',
      scheme: row.scheme ?? 'tcp',
      domain: row.domain ?? '',
      remote_scheme: row.remote_scheme ?? '',
      icon: row.icon ?? '',
      group: row.group ?? '',
      in_app: row.in_app !== false,
    };
  } else {
    forwardForm.value = emptyForward(serverId ?? tunnels.servers[0]?.id ?? '');
  }
  iconUrl.value = '';
  forwardDialog.value = true;
}

/** 跳转定位：清掉搜索、滚到那张卡、闪一下，再打开编辑弹窗。 */
let focusFlashTimer = 0;

async function focusForwardCard() {
  const request = props.focusForward;
  if (!request) {
    return;
  }
  // 搜索词可能正好把目标卡过滤掉，先清空，等列表重排完再找 DOM
  query.value = '';
  await nextTick();
  await nextTick();
  const definition = forwards.items.find((item) => item.definition.id === request.id)?.definition;
  const card = document.querySelector<HTMLElement>(`.fcard[data-forward-id="${request.id}"]`);
  if (card) {
    card.scrollIntoView({ behavior: 'smooth', block: 'center' });
    card.classList.add('is-focus');
    window.clearTimeout(focusFlashTimer);
    focusFlashTimer = window.setTimeout(() => card.classList.remove('is-focus'), 1600);
  }
  if (definition) {
    openForward(definition);
  }
  // 一次性请求：处理完就让上层清掉，否则下次切回本页还会自动弹窗
  emit('focus-handled');
}

watch(() => props.focusForward?.nonce, () => void focusForwardCard(), { flush: 'post' });

async function saveForward() {
  const form = forwardForm.value;
  if (forwardSaving.value) {
    return;
  }
  if (!form.name.trim() || !form.server_id) {
    ElMessage.warning('名称和所属隧道要填');
    return;
  }
  if (invalidCombo()) {
    ElMessage.warning('远端是 https 时本地必须也是 https');
    return;
  }
  forwardSaving.value = true;
  try {
    // 入口处的 icon 只允许本地 data URI；老配置里的远程地址在保存时顺手落盘。
    let icon = form.icon.trim();
    if (icon.startsWith('http://') || icon.startsWith('https://')) {
      try {
        icon = await iconToLocal(icon);
        form.icon = icon;
      } catch (e) {
        ElMessage.error(e instanceof Error ? e.message : String(e));
        return;
      }
    }
    await withTimeout(
      form.id
        ? forwards.update(form.id, {
            name: form.name,
            server_id: form.server_id,
            local_host: form.local_host,
            local_port: form.local_port,
            remote_host: form.remote_host,
            remote_port: form.remote_port,
            auto_start: form.enabled,
            note: form.note,
            open_path: form.open_path,
            scheme: form.scheme,
            domain: form.domain.trim(),
            remote_scheme: form.remote_scheme || undefined,
            icon,
            group: form.group.trim(),
            in_app: form.in_app,
          })
        : forwards.create({
            name: form.name,
            server_id: form.server_id,
            local_host: form.local_host,
            local_port: form.local_port,
            remote_host: form.remote_host,
            remote_port: form.remote_port,
            auto_start: form.enabled,
            note: form.note,
            open_path: form.open_path,
            scheme: form.scheme,
            domain: form.domain.trim(),
            remote_scheme: form.remote_scheme || undefined,
            icon,
            group: form.group.trim(),
            in_app: form.in_app,
          }),
      20_000,
      '保存超时：核心没有响应',
    );
    if (forwards.lastError) {
      ElMessage.error(forwards.lastError);
      return;
    }
    ElMessage.success(form.id ? '端口已更新' : '端口已添加');
    forwardDialog.value = false;
  } finally {
    forwardSaving.value = false;
  }
}

async function toggleForward(row: { id: string }, enabled: boolean) {
  await run(`f:${row.id}`, async () => {
    if (enabled) {
      await forwards.setAutoStart(row.id, true);
      await forwards.start(row.id);
    } else {
      await forwards.stop(row.id);
      await forwards.setAutoStart(row.id, false);
    }
    if (forwards.lastError) {
      ElMessage.error(forwards.lastError);
    }
  });
}

async function removeForward(row: { id: string; name: string }) {
  // 运行中的转发可以直接删，但要先说清"会先停掉它"——静默变灰比报错更糟。
  const state = forwards.items.find((item) => item.definition.id === row.id)?.status.state;
  if (state === 'running' || state === 'connecting') {
    const ok = window.confirm(`「${row.name}」正在运行，删除会先停止这条转发。继续？`);
    if (!ok) {
      return;
    }
  }

  await run(`f:${row.id}`, async () => {
    await forwards.remove(row.id);
    if (forwards.lastError) {
      ElMessage.error(forwards.lastError);
    } else {
      ElMessage.success(`已删除「${row.name}」`);
    }
  });
}

/* ───────────── 派生展示 ───────────── */

/** 隧道前的小圆点：颜色即状态。 */
/** 当前查看的隧道：由侧栏点选，store 里只有 id。 */
const selectedId = computed(() => selection.selectedId);

const selected = computed(() => {
  const list = rows.value;
  if (!list.length) {
    return null;
  }
  return list.find((row) => row.server.id === selectedId.value) ?? list[0];
});

// 隧道被删掉、或列表第一次有数据时，选中项要跟着变，否则右栏会空白。
watch(
  rows,
  (list) => {
    if (!list.length) {
      selection.select(null);
      return;
    }
    if (!list.some((row) => row.server.id === selection.selectedId)) {
      selection.select(list[0].server.id);
    }
  },
  { immediate: true },
);

/**
 * 已启用但没连上时的按钮语义：用户想的是"再试一次"，不是"取消保持连接"。
 * 停一下再起，等于让核心重跑一次握手。
 */
async function retryTunnel() {
  const row = selected.value;
  if (!row) {
    return;
  }
  await run(`t:${row.server.id}`, async () => {
    // 先掐掉现有会话（可能是半死不活的那种），再让它按"启用"重建一次。
    await tunnels.disconnectSession(row.server.id);
    await tunnels.setServerEnabled(row.server.id, true);
    if (tunnels.lastError) {
      ElMessage.error(tunnels.lastError);
    }
  });
}

/** Hero / 页头主按钮的动作：连不上时是重试，不是断开。 */
function primaryAction() {
  const row = selected.value;
  if (!row) {
    return;
  }
  if (!row.server.enabled) {
    void setTunnelEnabled(true);
    return;
  }
  if (tunnels.sessionConnected(row.server.id)) {
    void setTunnelEnabled(false);
    return;
  }
  void retryTunnel();
}

/** v-if 的类型收窄进不到回调里，所以开关走方法而不是内联箭头函数。 */
function setTunnelEnabled(value: boolean) {
  if (selected.value) {
    void toggleTunnel(selected.value.server, value);
  }
}

/** ↑/↓ 在隧道之间移动——列表是主导航时，键盘不该缺席。 */
function onKeydown(event: KeyboardEvent) {
  const target = event.target as HTMLElement | null;
  if (target && ['INPUT', 'TEXTAREA', 'SELECT'].includes(target.tagName)) {
    return;
  }
  if (tunnelDialog.value || forwardDialog.value) {
    return;
  }
  if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') {
    return;
  }
  const list = rows.value;
  if (!list.length) {
    return;
  }
  const index = list.findIndex((row) => row.server.id === selected.value?.server.id);
  const next = event.key === 'ArrowDown'
    ? Math.min(index + 1, list.length - 1)
    : Math.max(index - 1, 0);
  selection.select(list[next].server.id);
  event.preventDefault();
}

// 侧栏的「新建连接」是"待办"而不是"事件"：可能在本组件挂载之前就点了。
// 所以挂载时先补一次，之后靠 watch 接后续请求。
function openIfRequested() {
  if (selection.hasPendingCreate()) {
    selection.markCreateHandled();
    openTunnel();
  }
}

watch(() => selection.createRequests, openIfRequested);

onMounted(() => {
  openIfRequested();
  // 从别的页面点「编辑」跳进来时，本组件是刚挂载的，定位要在这里补一次
  void focusForwardCard();
  window.addEventListener('keydown', onKeydown);
});
onUnmounted(() => window.removeEventListener('keydown', onKeydown));

function tunnelPill(row: { server: { id: string } }): string {
  if (tunnels.pendingFingerprint(row.server.id)) {
    return 'is-warn';
  }
  if (!tunnels.sessionKnownOf(row.server.id)) {
    return 'is-warn';
  }
  return tunnels.sessionConnected(row.server.id) ? 'is-on' : 'is-off';
}

function tunnelLabel(row: { server: { id: string } }): string {
  if (tunnels.pendingFingerprint(row.server.id)) {
    return '待确认主机密钥';
  }
  if (!tunnels.sessionKnownOf(row.server.id)) {
    return '状态未知';
  }
  return tunnels.sessionConnected(row.server.id) ? '已连接' : '未连接';
}

/** 正在运行的转发条数（Hero 里的 x / y）。 */
const activeCount = computed(
  () => selected.value?.children.filter((c) => c.view.status.state === 'running').length ?? 0,
);

const serverDesc = computed(() => {
  const row = selected.value;
  if (!row) {
    return '';
  }
  if (tunnels.pendingFingerprint(row.server.id)) {
    return '首次连接，需要先核对这台主机的密钥指纹。';
  }
  if (!tunnels.sessionKnownOf(row.server.id)) {
    return '正在确认隧道状态（核心可能刚启动）。';
  }
  if (!tunnels.sessionConnected(row.server.id)) {
    return 'SSH 隧道未建立，转发规则不会生效。';
  }
  return `SSH 隧道已建立，${row.children.length} 条转发规则共享这一条连接。`;
});

/** 指纹太长，卡片里只显示前缀，完整值放 title。 */
function shortFingerprint(fingerprint: string): string {
  const body = fingerprint.startsWith('SHA256:') ? fingerprint.slice(7) : fingerprint;
  return `SHA256:${body.slice(0, 10)}…`;
}

const primaryLabel = computed(() => {
  const row = selected.value;
  if (!row) {
    return '建立连接';
  }
  if (!row.server.enabled) {
    return '建立连接';
  }
  return tunnels.sessionConnected(row.server.id) ? '断开连接' : '重新连接';
});

const traffic = useTrafficStore();

/** 隧道的合计速率；全 0 时不显示，避免满屏 0 B/s。 */
const tunnelRate = computed(() => {
  const row = selected.value;
  if (!row) {
    return null;
  }
  const speed = traffic.tunnelSpeed(row.server.id);
  const up = formatRate(speed.up);
  const down = formatRate(speed.down);
  return up || down ? { up: up || '0', down: down || '0' } : null;
});

const filteredChildren = computed(() => {
  const children = selected.value?.children ?? [];
  if (!query.value.trim()) {
    return children;
  }
  return children.filter((child) =>
    matchesQuery(
      child.view as unknown as ForwardLike,
      [child.view.definition.name, child.view.definition.note ?? ''],
      query.value,
    ),
  );
});

function tunnelDot(row: { server: { id: string } }): string {
  if (tunnels.pendingFingerprint(row.server.id)) {
    return 'is-warn';
  }
  if (!tunnels.sessionKnownOf(row.server.id)) {
    return 'is-unknown';
  }
  return tunnels.sessionConnected(row.server.id) ? 'is-on' : 'is-off';
}

</script>

<template>
  <div class="main">
    <template v-if="selected">
      <!-- SSH SERVER：一条连接的身份证 + 操作，不再堆"在线时长/监听范围"这类无信息量的格子 -->
      <section class="server-card" :class="{ 'is-live': tunnels.sessionConnected(selected.server.id) }">
        <div class="server-icon"><svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><rect x="2.5" y="2.5" width="11" height="4.5" rx="1.2"/><rect x="2.5" y="9" width="11" height="4.5" rx="1.2"/><path d="M5 4.75h.01M5 11.25h.01"/></svg></div>

        <div class="server-body">
          <div class="server-top">
            <span class="eyebrow">SSH SERVER</span>
            <span class="pill" :class="tunnelPill(selected)">
              <span class="dot" :class="tunnelDot(selected)" />
              {{ tunnelLabel(selected) }}
            </span>
          </div>

          <div class="server-addr mono">
            {{ selected.server.username }}@{{ selected.server.host }}:{{ selected.server.port }}
          </div>

          <p class="server-desc">{{ serverDesc }}</p>

          <div class="tags">
            <span class="tag"><svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><rect x="3.5" y="7" width="9" height="6.5" rx="1.5"/><path d="M5.5 7V5.2a2.5 2.5 0 0 1 5 0V7"/></svg> 保活 {{ selected.server.keepalive_secs }}s</span>
            <span class="tag">{{ selected.children.length }} 条转发</span>
            <span v-if="tunnelRate" class="tag is-rate" title="这条隧道下所有转发的合计速率">
              <span class="rate-up">↑ {{ tunnelRate.up }}</span>
              <span class="rate-down">↓ {{ tunnelRate.down }}</span>
            </span>
            <span
              v-if="selected.server.host_key_fingerprint"
              class="tag mono"
              :title="selected.server.host_key_fingerprint"
            >
              {{ shortFingerprint(selected.server.host_key_fingerprint) }}
            </span>
          </div>
        </div>

        <div class="server-actions">
          <button class="btn" @click="openTunnel(selected.server)">
            <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M11.2 2.3l2.5 2.5-8 8L3 13.5l.7-2.7 7.5-8.5z"/></svg> 编辑
          </button>
          <button class="btn" :disabled="busyKey !== null" @click="primaryAction">
            {{ primaryLabel }}
          </button>
          <button
            class="icon-btn is-danger"
            title="删除这条隧道"
            :disabled="busyKey !== null"
            @click="removeTunnel(selected.server)"
          >
            <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M3 4.5h10M6.5 4.5V3h3v1.5M4.5 4.5l.6 8.2h5.8l.6-8.2"/></svg>
          </button>
        </div>
      </section>

      <div
        v-if="selected.server.session_error && !tunnels.sessionConnected(selected.server.id)"
        class="server-error"
        :title="selected.server.session_error"
      >
        {{ selected.server.session_error }}
      </div>

      <!-- 首次连接：指纹确认 -->
      <div v-if="tunnels.pendingFingerprint(selected.server.id)" class="trust">
        <div class="trust-title">这台主机的密钥还没有确认过，核对无误后再信任</div>
        <div class="fp">{{ tunnels.pendingFingerprint(selected.server.id) }}</div>
        <div class="trust-ops">
          <button
            class="btn is-primary"
            :disabled="busyKey !== null"
            @click="trustKey(selected.server, false)"
          >
            信任并记录
          </button>
          <button
            class="btn is-danger"
            :disabled="busyKey !== null"
            @click="trustKey(selected.server, true)"
          >
            拒绝
          </button>
          <button class="btn" :disabled="busyKey !== null" @click="checkKnown(selected.server)">
            与本机 known_hosts 核对
          </button>
        </div>
        <div
          v-if="knownNote[selected.server.id]"
          class="known-note"
          :class="knownNote[selected.server.id].tone"
        >
          {{ knownNote[selected.server.id].text }}
        </div>
      </div>

      <!-- 端口转发 -->
      <div class="section-head">
        <div>
          <div class="section-title-row">
            <h3>端口转发</h3>
            <span class="count">{{ selected.children.length }}</span>
            <span v-if="activeCount > 0" class="running">
              <span class="dot is-on" />{{ activeCount }} 条运行中
            </span>
          </div>
          <p class="section-sub">每个规则把本机端口指向远端的服务</p>
        </div>
        <div class="section-actions">
          <input
            v-model="query"
            class="search"
            type="search"
            placeholder="搜索名称 / 备注 / IP / 端口"
          />
          <button class="btn is-primary" @click="openForward(undefined, selected.server.id)">
            + 新建转发
          </button>
        </div>
      </div>

      <div v-if="filteredChildren.length" class="grid">
        <ForwardCard
          v-for="child in filteredChildren"
          :key="child.key"
          :view="child.view"
          :server="selected.server"
          :busy-key="busyKey"
          @toggle="(value: boolean) => toggleForward(child.view.definition, value)"
          @edit="openForward(child.view.definition)"
          @remove="removeForward(child.view.definition)"
        />
      </div>

      <div v-else-if="selected.children.length" class="empty">
        <div class="empty-title">没有匹配的转发</div>
        <div>换个关键词再试：名称、备注、IP 或端口都能搜。</div>
      </div>

      <div v-else class="empty">
        <div class="empty-title">还没有转发规则</div>
        <div>点「新建转发」建立第一条端口映射。</div>
      </div>

      <p class="foot-note"><svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><rect x="3.5" y="7" width="9" height="6.5" rx="1.5"/><path d="M5.5 7V5.2a2.5 2.5 0 0 1 5 0V7"/></svg> 转发流量通过 SSH 加密，不会额外开放服务器端口</p>
    </template>

    <div v-else class="empty" style="margin-top: 40px">
      <img src="/logo.svg" alt="" />
      <div class="empty-title">还没有隧道</div>
      <div>在左侧点「新建连接」添加第一条 SSH 隧道。</div>
    </div>
  </div>

    <!-- 隧道：登录信息与连接信息放在同一个表单里，不再有独立的「凭据」 -->
    <el-dialog
      v-model="tunnelDialog"
      :title="tunnelForm.id ? '编辑隧道' : '新建隧道'"
      width="560px"
    >
      <el-form label-width="96px" label-position="left">
        <el-form-item label="名称">
          <el-input v-model="tunnelForm.name" placeholder="便于辨认，例如 办公室" />
        </el-form-item>
        <el-form-item label="主机">
          <div style="display: flex; gap: 8px; width: 100%">
            <el-input v-model="tunnelForm.host" placeholder="IP 或域名" style="flex: 1" />
            <el-input-number v-model="tunnelForm.port" :min="1" :max="65535" controls-position="right" style="width: 120px" />
          </div>
        </el-form-item>
        <el-form-item label="用户名">
          <el-input v-model="tunnelForm.username" placeholder="SSH 登录用户，例如 root" />
        </el-form-item>
        <el-form-item label="认证方式">
          <el-radio-group v-model="tunnelForm.kind" size="small">
            <el-radio-button value="password">密码</el-radio-button>
            <el-radio-button value="private_key">私钥</el-radio-button>
          </el-radio-group>
        </el-form-item>
        <el-form-item :label="tunnelForm.kind === 'password' ? '密码' : '私钥'">
          <el-input
            v-model="tunnelForm.secret"
            type="textarea"
            :rows="tunnelForm.kind === 'password' ? 2 : 5"
            :placeholder="
              tunnelForm.id
                ? '留空表示不修改；要换就填新的'
                : tunnelForm.kind === 'password'
                  ? '服务器登录密码'
                  : '粘贴 OpenSSH 私钥全文'
            "
          />
          <div v-if="tunnelForm.kind === 'private_key'" style="margin-top: 6px">
            <el-dropdown
              trigger="click"
              @visible-change="(visible: boolean) => visible && loadLocalKeys()"
              @command="importKey"
            >
              <el-button size="small" :disabled="!isTauri()">填入本机私钥</el-button>
              <template #dropdown>
                <el-dropdown-menu>
                  <el-dropdown-item v-if="localKeys.length === 0" disabled>
                    没找到私钥
                  </el-dropdown-item>
                  <el-dropdown-item v-for="key in localKeys" :key="key.name" :command="key.name">
                    {{ key.name }}
                  </el-dropdown-item>
                </el-dropdown-menu>
              </template>
            </el-dropdown>
          </div>
        </el-form-item>
        <el-form-item v-if="tunnelForm.kind === 'private_key'" label="私钥口令">
          <el-input v-model="tunnelForm.passphrase" placeholder="私钥没加密就留空" show-password />
        </el-form-item>
      </el-form>
      <template #footer>
        <div class="dialog-footer">
          <el-button size="small" :loading="tunnelTesting" @click="testTunnel">测试连接</el-button>
          <div>
            <el-button size="small" @click="tunnelDialog = false">取消</el-button>
            <el-button size="small" type="primary" :loading="tunnelSaving" @click="saveTunnel">
              保存
            </el-button>
          </div>
        </div>
      </template>
    </el-dialog>

    <!-- 端口转发 -->
    <el-dialog
      v-model="forwardDialog"
      :title="forwardForm.id ? '编辑端口' : '添加端口'"
      width="560px"
    >
      <el-form label-width="96px" label-position="left">
        <el-form-item label="名称">
          <el-input v-model="forwardForm.name" placeholder="例如 homeassistant" />
        </el-form-item>
        <el-form-item label="备注">
          <el-input
            v-model="forwardForm.note"
            maxlength="120"
            show-word-limit
            placeholder="一句话说明，例如「内网 HA 面板」"
          />
        </el-form-item>
        <el-form-item label="分组">
          <el-select
            v-model="forwardForm.group"
            filterable
            allow-create
            default-first-option
            clearable
            placeholder="留空即未分组；可直接输入新分组"
            style="width: 100%"
          >
            <el-option v-for="g in groupOptions" :key="g" :label="g" :value="g" />
          </el-select>
        </el-form-item>
        <el-form-item label="图标">
          <div class="icon-edit">
            <img v-if="forwardForm.icon" :src="forwardForm.icon" class="icon-thumb" alt="" />
            <span v-else class="icon-thumb is-empty">自动</span>
            <el-input v-model="iconUrl" placeholder="粘贴图标 URL 后点导入" />
            <button
              class="btn"
              type="button"
              :disabled="iconBusy || !iconUrl.trim()"
              @click="importIconFromUrl"
            >
              {{ iconBusy ? '导入中…' : '导入' }}
            </button>
            <label class="btn">
              本地文件
              <input type="file" accept="image/*" hidden @change="importIconFromFile" />
            </label>
            <button
              v-if="forwardForm.icon"
              class="btn"
              type="button"
              @click="forwardForm.icon = ''"
            >
              清除
            </button>
          </div>
          <p class="hint" style="width: 100%">
            导入后缩成 128px 并保存到本地配置，离线也能显示；留空则按入口自动取 favicon。
          </p>
        </el-form-item>
        <el-form-item label="添加到应用">
          <el-switch v-model="forwardForm.in_app" />
          <span class="hint" style="margin-left: 10px">
            打开后出现在「应用」页，可以按分组摆放。
          </span>
        </el-form-item>
        <el-form-item label="所属隧道">
          <el-select v-model="forwardForm.server_id" style="width: 100%">
            <el-option
              v-for="s in tunnels.servers"
              :key="s.id"
              :label="`${s.name}（${s.username}@${s.host}）`"
              :value="s.id"
            />
          </el-select>
        </el-form-item>
        <el-form-item label="访问域名（可选）">
          <el-input v-model="forwardForm.domain" placeholder="留空即不用域名；例如 home.example.com" />
          <p class="hint" style="width: 100%">
            留空就只用本机地址访问，与以前完全一致；填了才额外分配域名入口（127.0.0.2–254），
            本机 hosts 会把域名指到这条转发，首次写入需要一次 UAC 授权。
          </p>
        </el-form-item>
        <el-form-item label="协议">
          <div style="display: flex; gap: 8px; width: 100%; align-items: center">
            <span class="hint" style="white-space: nowrap">本地</span>
            <el-select v-model="forwardForm.scheme" style="width: 110px" @change="onSchemeChanged">
              <el-option label="HTTP" value="http" />
              <el-option label="HTTPS" value="https" />
              <el-option label="RDP" value="rdp" />
              <el-option label="SSH" value="ssh" />
              <el-option label="TCP" value="tcp" />
            </el-select>
            <span class="hint" style="white-space: nowrap">→ 远端</span>
            <el-select
              v-if="isWebScheme(forwardForm.scheme)"
              v-model="forwardForm.remote_scheme"
              placeholder="跟随本地"
              style="width: 130px"
            >
              <el-option label="跟随本地" value="" />
              <el-option label="HTTP" value="http" />
              <el-option label="HTTPS" value="https" />
            </el-select>
            <span v-else class="hint">纯 TCP，没有远端协议</span>
          </div>
          <p class="hint" style="width: 100%">
            HTTP→HTTP 明文直通；HTTPS→HTTP 本地解密；HTTPS→HTTPS 透传。
            RDP / SSH / TCP 都是纯端口转发，只有默认端口不同。
          </p>
          <p
            v-if="invalidCombo()"
            class="hint"
            style="width: 100%; color: var(--el-color-danger)"
          >
            远端是 https 时本地必须也是 https。
          </p>
        </el-form-item>
        <el-form-item v-if="isWebScheme(forwardForm.scheme)" label="打开路径">
          <el-input
            v-model="forwardForm.open_path"
            placeholder="留空就是 /；可填 /lovelace/0"
          >
            <template #prepend>
              {{ forwardForm.scheme }}://{{ forwardForm.domain || '127.0.0.1' }}:{{ forwardForm.local_port }}
            </template>
          </el-input>
        </el-form-item>
        <el-form-item label="本地">
          <div style="display: flex; gap: 8px; width: 100%">
            <span v-if="usesDomain" class="hint" style="flex: 1; line-height: 32px">
              保存后自动分配（127.0.0.2–254）
            </span>
            <el-input v-else v-model="forwardForm.local_host" style="flex: 1" />
            <el-input-number
              v-model="forwardForm.local_port"
              :min="1"
              :max="65535"
              controls-position="right"
              style="width: 130px"
              @update:model-value="onLocalPortEdited"
            />
            <button class="btn is-icon" type="button" title="随机一个没被占用的本地端口" @click="randomLocalPort">
              <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><rect x="2.5" y="2.5" width="11" height="11" rx="2.5"/><circle cx="5.8" cy="5.8" r="0.9" fill="currentColor"/><circle cx="10.2" cy="10.2" r="0.9" fill="currentColor"/><circle cx="8" cy="8" r="0.9" fill="currentColor"/></svg>
            </button>
          </div>
          <p class="hint" style="width: 100%">
            域名入口用的就是这个端口：HTTPS 填 443、HTTP 填 80，浏览器里才不用带端口。
          </p>
        </el-form-item>
        <el-form-item label="远端">
          <div style="display: flex; gap: 8px; width: 100%">
            <el-input v-model="forwardForm.remote_host" placeholder="服务器视角的地址" style="flex: 1" />
            <el-input-number
              v-model="forwardForm.remote_port"
              :min="1"
              :max="65535"
              controls-position="right"
              style="width: 130px"
              @update:model-value="followRemotePort"
            />
          </div>
        </el-form-item>
        <el-form-item label="启用">
          <el-switch v-model="forwardForm.enabled" size="small" />
          <span class="hint" style="margin-left: 8px">保存后立即连接；隧道未启用时不会启动</span>
        </el-form-item>
      </el-form>
      <template #footer>
        <div class="dialog-footer">
          <el-button size="small" :loading="forwardTesting" @click="testForward">测试连接</el-button>
          <div>
            <el-button size="small" @click="forwardDialog = false">取消</el-button>
            <el-button
              size="small"
              type="primary"
              :loading="forwardSaving"
              :disabled="invalidCombo()"
              @click="saveForward"
            >
              保存
            </el-button>
          </div>
        </div>
      </template>
    </el-dialog>
</template>

<style scoped>
.chip {
  appearance: none;
}

.sw {
  appearance: none;
  flex-shrink: 0;
}

/* 弹窗底部：左边放次要动作（测试连接），右边是取消 / 保存 */
.dialog-footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
}
</style>

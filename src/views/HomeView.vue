<script setup lang="ts">
// 一个页面装两层：隧道（一条 SSH 连接，自带登录信息）与其下的端口转发。
// 界面上没有「凭据」这一层——用户名和密钥在新建隧道时一起填。
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue';
import { ElMessage } from 'element-plus';
import { useTunnelsStore } from '../models/tunnels';
import { useServersStore } from '../models/servers';
import { withTimeout } from '../models/asyncAction';
import { matchesQuery, type ForwardLike } from '../models/forwardView';
import { applySchemeChange, type Scheme } from '../models/forwardForm';
import {
  defaultHeaderRewrite,
  isDefaultRewrite,
  previewRewrite,
  toDtoRewrite,
  toFormRewrite,
  type HeaderRewriteForm,
} from '../models/headerRewrite';
import { formatRate, useTrafficStore } from '../models/traffic';
import ForwardCard from '../components/ForwardCard.vue';
import InfoTip from '../components/InfoTip.vue';

import { useSelectionStore } from '../models/selection';
import {
  importIcon,
  isTauri,
  knownHostsFingerprint,
  listSshKeys,
  readLocalPrivateKey,
} from '../api/native';
import type { CredentialKind, CreateServerRequest, HeaderRewrite } from '../api/types';

const forwards = useTunnelsStore();
const tunnels = useServersStore();
// 隧道列表在侧栏（App.vue），选中态必须共享，否则点左边右边不动。
const selection = useSelectionStore();

const props = defineProps<{
  /** 从「应用」「所有转发」跳转过来：滚动到这张卡并打开它的编辑弹窗。 */
  focusForward?: { id: string; nonce: number } | null;
}>();

const emit = defineEmits<{
  (e: 'focus-handled'): void;
  /** 打开服务器管理页（卡片上的「管理」）：交给 App 切页，带隧道的 server_id。 */
  (e: 'manage-server', serverId: string): void;
}>();

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

function onSchemeChanged(value: Scheme) {
  // 规则（写回协议 + 端口跟着协议走）在 models/forwardForm 里，纯函数好测：
  // 之前换成原生 select 时漏了「写回 scheme」，界面就变成「选了没反应」。
  applySchemeChange(forwardForm.value, value);
}

/** 原生 select 的 change：收窄回 `Scheme` 再交给统一逻辑（端口跟着协议走）。 */
function onNativeSchemeChange(event: Event) {
  onSchemeChanged((event.target as HTMLSelectElement).value as Scheme);
}

/** 远端协议：空串 = 跟随本地。 */
function onRemoteSchemeChange(event: Event) {
  forwardForm.value.remote_scheme = (event.target as HTMLSelectElement).value as
    | ''
    | 'http'
    | 'https';
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
    /*
     * 判据是「表单里有没有新凭据」，不是「有没有 id」。
     *
     * 编辑已保存的连接时 secret 是空的（密码不回显），用户重新粘了私钥或填了密码，
     * 就该拿这份新值去探。原来看 id 就一律用库里的凭据——而导入来的连接库里
     * 根本没有凭据，于是"明明贴了私钥，却报这条隧道还没填密码"。
     */
    const draft = !form.id || form.secret.trim() !== '';
    const report = draft
      ? await tunnels.probeDraft({
          host: form.host,
          port: form.port,
          username: form.username,
          credential_kind: form.kind,
          secret: form.secret,
          passphrase: form.passphrase === '' ? null : form.passphrase,
          host_key_fingerprint: null,
        })
      : await tunnels.probeServer(form.id as string);
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
    // 连带删除交给服务端一次做完：本端只发一个请求。
    // 以前是这里 for 循环逐条删转发，40 条转发就是 40 次请求 + 40 次写盘 +
    // 40 次 hosts 同步，删一条连接要几秒。
    await tunnels.removeServer(row.id, true);
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
  /** 是否改写 HTTP 头（Host / X-Forwarded-* / Location）；默认关。 */
  rewrite_http_headers: boolean;
  /** 改写细则：Host 三态、两个开关、自定义头 */
  header_rewrite: HeaderRewriteForm;
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
  if (tunnels.isLocal(form.server_id)) {
    ElMessage.info('本机不需要测试连接：转发开启后直接连本机端口');
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

/**
 * 头改写只在能看到明文的入口有意义：① http→http 明文直通、② https→http 本地解密。
 * ③ https 透传读到的是 TLS 密文；tcp/rdp/ssh 没有 HTTP 语义——开关置灰。
 */
const canRewriteHeaders = computed(() => {
  const form = forwardForm.value;
  if (!usesDomain.value || !isWebScheme(form.scheme)) {
    return false;
  }
  // 远端留空 = 跟随本地
  return (form.remote_scheme || form.scheme) === 'http';
});

/** 开关置灰时说明原因，避免用户以为是界面坏了。 */
const rewriteUnavailableHint = computed(() => {
  if (!usesDomain.value) {
    return '没填访问域名时是纯字节管道，不改写任何头。';
  }
  return 'HTTPS→HTTPS 是端到端透传，读到的是密文，改不了。';
});

/** 开关真正生效（入口看得见明文且开关打开）。关掉时细则只读。 */
const rewriteEditable = computed(
  () => canRewriteHeaders.value && forwardForm.value.rewrite_http_headers,
);

/** 「这次会改成什么」的对照表，随表单实时算；开关关掉时如实显示「原样透传」。 */
const rewritePreview = computed(() =>
  previewRewrite(forwardForm.value.header_rewrite, {
    enabled: rewriteEditable.value,
    entryHost:
      forwardForm.value.domain.trim() || forwardForm.value.local_host || '127.0.0.1',
    entryPort: forwardForm.value.local_port,
    scheme: forwardForm.value.scheme === 'https' ? 'https' : 'http',
    remoteScheme:
      forwardForm.value.remote_scheme ||
      (forwardForm.value.scheme === 'https' ? 'https' : 'http'),
    remoteHost: forwardForm.value.remote_host || '127.0.0.1',
    remotePort: forwardForm.value.remote_port,
  }),
);

/** 全默认时不给「还原」按钮，免得点了没变化。 */
const rewriteIsDefault = computed(() => isDefaultRewrite(forwardForm.value.header_rewrite));

function resetHeaderRewrite() {
  forwardForm.value.header_rewrite = defaultHeaderRewrite();
}

function addHeaderRow() {
  forwardForm.value.header_rewrite.extra.push({ name: '', value: '' });
}

function removeHeaderRow(index: number) {
  forwardForm.value.header_rewrite.extra.splice(index, 1);
}

// 静态说明一律进「?」气泡：表单里只保留状态与错误，不铺大段解释。
const entryHelp =
  '这是浏览器访问的入口。\n' +
  'HTTP：明文直通；HTTPS：核心用本机 CA 终止 TLS（可回源到 http 或 https）；RDP / SSH / TCP：纯端口转发。\n' +
  '域名模式下地址由核心分配（127.0.0.2–254，同域名共用同一个地址），' +
  '端口建议 HTTPS 443、HTTP 80，浏览器里才不用带端口。';
const remoteHelp =
  '这是目标程序在服务器上监听的地址与协议。\n' +
  '远端协议留空＝跟随本地：HTTPS→HTTPS 端到端透传（核心看不到内容，也就改不了 HTTP 头）。';
const domainHelp =
  '留空就只用本机地址访问，与以前完全一致。\n' +
  '填了才额外分配 127.0.0.2–254 里的域名入口，并把域名写进本机 hosts（首次写入需要一次 UAC）。';
const rewriteHelp =
  '打开后把 Host 换成远端地址（80/443 不带端口）、补 X-Forwarded-Proto/Host，' +
  '并把远端指回自己地址的 Location 换回域名地址。\n' +
  '默认关闭＝原样转发，远端按自己的 server_name 匹配。';
const iconHelp = '导入后缩成 128px 保存到本地配置，离线也能显示；留空则按入口自动取 favicon。';

/**
 * 表单分区折叠状态。
 *
 * 「访问方式」与「更多设置」默认收起，把首屏压到 6 行（名称/隧道/协议/本地/远端/启用）；
 * 编辑已有转发时按内容自动展开，免得隐藏用户已经填过的值。
 */
const forwardDomainOpen = ref(false);
const forwardMoreOpen = ref(false);

/** 折叠标题右侧的摘要：一眼看出这条转发有没有域名、改不改头。 */
const domainSummary = computed(() => {
  const domain = forwardForm.value.domain.trim();
  if (!domain) {
    return '未使用域名';
  }
  // 只有真正会生效时才写「改写」：③ 透传即便开关留着 true 也不生效
  return canRewriteHeaders.value && forwardForm.value.rewrite_http_headers
    ? `${domain} · 改写 HTTP 头`
    : domain;
});

/** 「更多设置」里已经填了哪些，收起时也留个线索。 */
const moreSummary = computed(() => {
  const form = forwardForm.value;
  const filled: string[] = [];
  if (form.note.trim()) {
    filled.push('备注');
  }
  if (form.group.trim()) {
    filled.push('分组');
  }
  if (form.icon) {
    filled.push('图标');
  }
  if (!form.in_app) {
    filled.push('不在应用页');
  }
  return filled.length ? filled.join(' · ') : '默认';
});

/** 归一化域名用于比较（与后端 `validate_domain` 同口径：去空格、小写、去末尾点）。 */
function normalizeDomain(raw: string): string {
  return raw.trim().toLowerCase().replace(/\.$/, '');
}

/**
 * 同一个域名的其它转发。
 *
 * 域名不是独占的：`git.zly.m` 的 443 走网页、22 走 SSH 是合法配置，它们共用同一个
 * 回环地址，所以唯一要避开的是**端口**。表单在这里把已占端口挑出来提示。
 */
const domainSiblings = computed(() => {
  const domain = normalizeDomain(forwardForm.value.domain);
  if (!domain) {
    return [];
  }
  const selfId = forwardForm.value.id;
  return forwards.items.filter(
    (item) => item.definition.id !== selfId && normalizeDomain(item.definition.domain ?? '') === domain,
  );
});

/**
 * 域名入口会落在哪个回环地址：按后端 `domain_host_for` 的三条分支原样推断，
 * 让表单直接显示结果，而不是「保存后自动分配」这种被截断的占位符。
 *
 * 1. 同域名已有别的转发 → 共用它的地址；
 * 2. 编辑本条且域名没改 → 保持原地址；
 * 3. 否则取池里第一个没被占用的（`domain_host` 与 `local_host` 都算占用）。
 */
const predictedDomainHost = computed(() => {
  const domain = normalizeDomain(forwardForm.value.domain);
  if (!domain) {
    return '';
  }
  const selfId = forwardForm.value.id;
  const sibling = domainSiblings.value.find((item) => item.definition.domain_host);
  if (sibling) {
    return sibling.definition.domain_host;
  }
  const self = forwards.items.find((item) => item.definition.id === selfId)?.definition;
  if (self?.domain_host && normalizeDomain(self.domain ?? '') === domain) {
    return self.domain_host;
  }
  const used = new Set<string>();
  for (const item of forwards.items) {
    if (item.definition.id === selfId) {
      continue;
    }
    for (const value of [item.definition.domain_host, item.definition.local_host]) {
      if (value) {
        used.add(value);
      }
    }
  }
  for (let n = 2; n <= 254; n += 1) {
    const candidate = `127.0.0.${n}`;
    if (!used.has(candidate)) {
      return candidate;
    }
  }
  return '';
});

/** 本地地址框：域名模式下显示推断出的回环地址（只读），其余情况直接编辑。 */
const localHostField = computed({
  get: () => (usesDomain.value ? predictedDomainHost.value : forwardForm.value.local_host),
  set: (value: string) => {
    forwardForm.value.local_host = value;
  },
});

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
    rewrite_http_headers: false,
    header_rewrite: defaultHeaderRewrite(),
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
    rewrite_http_headers?: boolean;
    header_rewrite?: HeaderRewrite | null;
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
      rewrite_http_headers: row.rewrite_http_headers === true,
      header_rewrite: toFormRewrite(row.header_rewrite),
      icon: row.icon ?? '',
      group: row.group ?? '',
      in_app: row.in_app !== false,
    };
  } else {
    forwardForm.value = emptyForward(serverId ?? tunnels.servers[0]?.id ?? '');
  }
  // 已经填过的分区直接展开，折叠只用来藏「还没用上的东西」
  forwardDomainOpen.value = Boolean(forwardForm.value.domain.trim());
  forwardMoreOpen.value = Boolean(
    forwardForm.value.note.trim() ||
      forwardForm.value.group.trim() ||
      forwardForm.value.icon ||
      !forwardForm.value.in_app,
  );
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
            rewrite_http_headers: form.rewrite_http_headers,
            header_rewrite: toDtoRewrite(form.header_rewrite),
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
            rewrite_http_headers: form.rewrite_http_headers,
            header_rewrite: toDtoRewrite(form.header_rewrite),
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

/** 当前选中的是不是内置「本机」：它没有会话与管理页，卡片要收起那几处入口。 */
const selectedIsLocal = computed(() => (selected.value ? tunnels.isLocal(selected.value.server.id) : false));

const serverDesc = computed(() => {
  const row = selected.value;
  if (!row) {
    return '';
  }
  if (selectedIsLocal.value) {
    return `本机程序：域名入口直接连本机端口，不经过 SSH。${row.children.length} 条转发。`;
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
  if (tunnels.isLocal(row.server.id)) {
    return '本机恒已连接';
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
            <span class="eyebrow">{{ selectedIsLocal ? 'THIS PC · 本机' : 'SSH SERVER' }}</span>
            <span class="pill" :class="tunnelPill(selected)">
              <span class="dot" :class="tunnelDot(selected)" />
              {{ tunnelLabel(selected) }}
            </span>
          </div>

          <div class="server-addr mono">
            {{
              selectedIsLocal
                ? '本机程序 · 转发直接连本机端口'
                : `${selected.server.username}@${selected.server.host}:${selected.server.port}`
            }}
          </div>

          <p class="server-desc">{{ serverDesc }}</p>

          <div class="tags">
            <span v-if="!selectedIsLocal" class="tag"><svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><rect x="3.5" y="7" width="9" height="6.5" rx="1.5"/><path d="M5.5 7V5.2a2.5 2.5 0 0 1 5 0V7"/></svg> 保活 {{ selected.server.keepalive_secs }}s</span>
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
          <!-- 本机是内置条目：没有会话可连、没有管理页、也不能改配置，这些入口一律不显示 -->
          <template v-if="!selectedIsLocal">
            <!-- 管理页入口：借这条隧道的会话去看服务器本身（系统信息/进程/容器…） -->
            <button
              class="btn"
              title="管理这台服务器：系统信息、进程、容器，以及后续的终端与文件管理"
              @click="emit('manage-server', selected.server.id)"
            >
              <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M2.5 4.5h11v3h-11zM2.5 9.5h11v3h-11z"/><path d="M5 6h.01M5 11h.01"/></svg> 管理
            </button>
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
          </template>
          <span v-else class="tag">内置条目 · 恒已连接</span>
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
        <el-form-item label="所属隧道">
          <el-select v-model="forwardForm.server_id" style="width: 100%">
            <el-option
              v-for="s in tunnels.servers"
              :key="s.id"
              :label="s.kind === 'local' ? '本机（跑在这台电脑上的程序）' : `${s.name}（${s.username}@${s.host}）`"
              :value="s.id"
            />
          </el-select>
        </el-form-item>
        <el-form-item label="本地">
          <div class="field-row">
            <!-- 协议拼在地址前面：一行就是「本地 <协议>://<地址>:<端口>」 -->
            <el-input
              v-model="localHostField"
              :readonly="usesDomain"
              :placeholder="usesDomain ? '自动分配' : '127.0.0.1'"
              style="flex: 1"
            >
              <template #prepend>
                <!-- 原生 select：只有 5 个固定项，不需要搜索；外观自己画，不跟 Element 内部类较劲 -->
                <select
                  class="prepend-select"
                  :value="forwardForm.scheme"
                  @change="onNativeSchemeChange"
                >
                  <option value="http">HTTP</option>
                  <option value="https">HTTPS</option>
                  <option value="rdp">RDP</option>
                  <option value="ssh">SSH</option>
                  <option value="tcp">TCP</option>
                </select>
              </template>
            </el-input>
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
            <InfoTip :content="entryHelp" />
          </div>
        </el-form-item>
        <el-form-item label="远端">
          <div class="field-row">
            <el-input v-model="forwardForm.remote_host" placeholder="服务器视角的地址" style="flex: 1">
              <template #prepend>
                <select
                  v-if="isWebScheme(forwardForm.scheme)"
                  class="prepend-select"
                  :value="forwardForm.remote_scheme"
                  @change="onRemoteSchemeChange"
                >
                  <option value="">跟随本地</option>
                  <option value="http">HTTP</option>
                  <option value="https">HTTPS</option>
                </select>
                <!-- 纯 TCP 没有远端协议，但保留同宽占位，两行输入框能对齐 -->
                <span v-else class="prepend-text">TCP</span>
              </template>
            </el-input>
            <el-input-number
              v-model="forwardForm.remote_port"
              :min="1"
              :max="65535"
              controls-position="right"
              style="width: 130px"
              @update:model-value="followRemotePort"
            />
            <!-- 与「本地」行的随机端口按钮等宽：两行的地址框、端口框才能对齐 -->
            <span class="port-tool-spacer" aria-hidden="true"></span>
            <InfoTip :content="remoteHelp" />
          </div>
          <p
            v-if="invalidCombo()"
            class="hint"
            style="width: 100%; color: var(--el-color-danger)"
          >
            远端是 https 时本地必须也是 https。
          </p>
        </el-form-item>
        <el-form-item label="启用">
          <el-switch v-model="forwardForm.enabled" size="small" />
          <span class="hint" style="margin-left: 8px">保存后立即连接；隧道未启用时不会启动</span>
        </el-form-item>

        <!-- 分区：访问方式。默认折叠，标题右侧直接写当前值，不点开也知道设没设。 -->
        <div class="form-group" :class="{ 'is-open': forwardDomainOpen }">
          <button type="button" class="form-group-head" @click="forwardDomainOpen = !forwardDomainOpen">
            <span class="form-group-title">访问方式</span>
            <span class="form-group-summary">{{ domainSummary }}</span>
            <svg class="form-group-chevron" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M6 3.5 10.5 8 6 12.5" /></svg>
          </button>
          <div v-show="forwardDomainOpen" class="form-group-body">
            <el-form-item label="访问域名">
              <div class="field-row">
                <el-input v-model="forwardForm.domain" placeholder="例如 home.example.com">
                  <template #prepend>
                    <span class="prepend-proto">{{ forwardForm.scheme }}://</span>
                  </template>
                </el-input>
                <InfoTip :content="domainHelp" />
              </div>
              <p v-if="domainSiblings.length" class="hint" style="width: 100%">
                {{ forwardForm.domain.trim() }} 已由「{{
                  domainSiblings.map((item) => item.definition.name).join('、')
                }}」使用：同域名共用同一个回环地址，靠端口区分（已占 {{
                  domainSiblings.map((item) => item.definition.local_port).join('、')
                }}），本条请换一个端口（例如网页 443、SSH 22）。
              </p>
            </el-form-item>
            <!-- 打开路径紧跟访问域名：两者都是「浏览器怎么访问这条转发」 -->
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
            <!-- 只有 web 入口才谈得上 HTTP 头；tcp/rdp/ssh 连这一行都不显示 -->
            <el-form-item v-if="isWebScheme(forwardForm.scheme)" label="HTTP 头改写">
              <el-switch v-model="forwardForm.rewrite_http_headers" :disabled="!canRewriteHeaders" />
              <InfoTip v-if="canRewriteHeaders" :content="rewriteHelp" />
              <p v-if="!canRewriteHeaders" class="hint" style="width: 100%">
                {{ rewriteUnavailableHint }}
              </p>

              <!-- 细则与预览常显：开关关掉时也要能看清「远端会收到什么」，只是控件只读 -->
              <div
                v-if="canRewriteHeaders"
                class="rewrite-editor"
                :class="{ 'is-readonly': !rewriteEditable }"
              >
                <div class="rewrite-row">
                  <span class="rewrite-name">Host</span>
                  <el-select
                    v-model="forwardForm.header_rewrite.host_mode"
                    size="small"
                    style="width: 152px"
                    :disabled="!rewriteEditable"
                  >
                    <el-option label="自动（远端地址）" value="auto" />
                    <el-option label="保留浏览器原值" value="keep" />
                    <el-option label="自定义…" value="custom" />
                  </el-select>
                  <el-input
                    v-if="forwardForm.header_rewrite.host_mode === 'custom'"
                    v-model="forwardForm.header_rewrite.host"
                    size="small"
                    placeholder="例如 edge.example.com"
                    :disabled="!rewriteEditable"
                  />
                </div>

                <div class="rewrite-row">
                  <span class="rewrite-name">补充头</span>
                  <el-switch
                    v-model="forwardForm.header_rewrite.forward_headers"
                    size="small"
                    :disabled="!rewriteEditable"
                  />
                  <span class="hint">打开＝发送 X-Forwarded-Proto / X-Forwarded-Host</span>
                </div>

                <div class="rewrite-row">
                  <span class="rewrite-name">Location</span>
                  <el-switch
                    v-model="forwardForm.header_rewrite.rewrite_location"
                    size="small"
                    :disabled="!rewriteEditable"
                  />
                  <span class="hint">打开＝把远处的地址换回你访问的域名</span>
                </div>

                <div
                  v-for="(pair, index) in forwardForm.header_rewrite.extra"
                  :key="index"
                  class="rewrite-row"
                >
                  <el-input
                    v-model="pair.name"
                    size="small"
                    placeholder="头名，如 X-Real-IP"
                    style="width: 160px"
                    :disabled="!rewriteEditable"
                  />
                  <el-input
                    v-model="pair.value"
                    size="small"
                    placeholder="值"
                    :disabled="!rewriteEditable"
                  />
                  <button
                    class="btn is-icon"
                    type="button"
                    title="删除这个头"
                    :disabled="!rewriteEditable"
                    @click="removeHeaderRow(index)"
                  >
                    <svg
                      viewBox="0 0 16 16"
                      fill="none"
                      stroke="currentColor"
                      stroke-width="1.6"
                      stroke-linecap="round"
                    >
                      <path d="M4 4l8 8M12 4l-8 8" />
                    </svg>
                  </button>
                </div>

                <div class="rewrite-row">
                  <button
                    class="btn"
                    type="button"
                    :disabled="!rewriteEditable"
                    @click="addHeaderRow"
                  >
                    + 自定义头
                  </button>
                  <button
                    v-if="!rewriteIsDefault"
                    class="btn"
                    type="button"
                    :disabled="!rewriteEditable"
                    @click="resetHeaderRewrite"
                  >
                    还原默认
                  </button>
                </div>

                <div class="rewrite-preview">
                  <div class="rewrite-preview-title">
                    {{
                      rewriteEditable
                        ? '这次会这样发给远端'
                        : '开关关闭：原样透传，远端会收到下面这些'
                    }}
                  </div>
                  <div
                    v-for="(line, index) in rewritePreview"
                    :key="`${line.label}-${index}`"
                    class="rewrite-preview-row"
                    :class="{ 'is-off': line.off, 'is-from': line.from !== '' }"
                  >
                    <span class="rewrite-preview-name">{{ line.label }}</span>
                    <span v-if="line.note" class="rewrite-preview-note">{{ line.note }}</span>
                    <template v-else>
                      <span v-if="line.from" class="rewrite-preview-from">{{ line.from }}</span>
                      <span v-if="line.from" class="rewrite-preview-arrow">→</span>
                      <span class="rewrite-preview-to">{{ line.to }}</span>
                    </template>
                  </div>
                </div>
              </div>
            </el-form-item>
          </div>
        </div>

        <!-- 分区：更多设置。不影响连通性，默认收起。 -->
        <div class="form-group" :class="{ 'is-open': forwardMoreOpen }">
          <button type="button" class="form-group-head" @click="forwardMoreOpen = !forwardMoreOpen">
            <span class="form-group-title">更多设置</span>
            <span class="form-group-summary">{{ moreSummary }}</span>
            <svg class="form-group-chevron" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M6 3.5 10.5 8 6 12.5" /></svg>
          </button>
          <div v-show="forwardMoreOpen" class="form-group-body">
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
                <InfoTip :content="iconHelp" />
              </div>
            </el-form-item>
            <el-form-item label="添加到应用">
              <el-switch v-model="forwardForm.in_app" />
              <span class="hint" style="margin-left: 10px">
                打开后出现在「应用」页，可以按分组摆放。
              </span>
            </el-form-item>
          </div>
        </div>
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

/* 输入框 + 行内「?」同排：气泡图标不能把控件挤到下一行 */
.field-row {
  display: flex;
  align-items: center;
  gap: 2px;
  width: 100%;
}

.field-row > .el-input {
  flex: 1;
  min-width: 0;
}

/* 端口框固定宽度：不参与 flex 收缩，否则「本地」「远端」两行会被末尾按钮挤成不同宽度 */
.field-row > .el-input-number {
  flex: 0 0 130px;
}

/* 占位：与 .btn.is-icon 的 32px 对齐 */
.port-tool-spacer {
  flex-shrink: 0;
  width: 32px;
}

/* 拼在地址输入框里的协议选择器：去掉它自己的边框/底色/阴影，和 prepend 融为一体 */
.field-row :deep(.el-input-group__prepend) {
  padding: 0;
  background: var(--el-fill-color-light);
  border-radius: var(--el-border-radius-base) 0 0 var(--el-border-radius-base);
}

.prepend-select {
  width: 96px;
  height: 30px;
  padding: 0 18px 0 10px;
  border: 0;
  border-radius: 0;
  background-color: transparent;
  background-image: url("data:image/svg+xml;charset=utf-8,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16' fill='none' stroke='%23909a9e' stroke-width='1.6' stroke-linecap='round' stroke-linejoin='round'%3E%3Cpath d='M4 6.5 8 10.5 12 6.5'/%3E%3C/svg%3E");
  background-repeat: no-repeat;
  background-position: right 5px center;
  background-size: 12px 12px;
  color: var(--el-text-color-regular);
  font: inherit;
  font-size: 12px;
  appearance: none;
  outline: none;
  cursor: pointer;
}

/* 键盘聚焦时要看得见（原生 select 去掉 outline 后没有焦点反馈） */
.prepend-select:focus-visible {
  outline: 2px solid var(--el-color-primary);
  outline-offset: -2px;
  border-radius: 2px;
}

/* 域名/路径这类静态前缀：只占内容宽度，不参与输入 */
.prepend-proto {
  padding: 0 8px;
  font-size: 12px;
  color: var(--el-text-color-secondary);
  white-space: nowrap;
}

/* 纯 TCP 时没有远端协议可选，但仍占同样宽度，两行输入框才对得上 */
.prepend-text {
  display: inline-block;
  width: 96px;
  font-size: 12px;
  line-height: 30px;
  color: var(--el-text-color-secondary);
  text-align: center;
}

/* 端口弹窗的可选分区：标题一行可点，右侧直接写当前值，收起时也不瞎 */
.form-group {
  margin: 2px 0 14px;
}

.form-group-head {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  padding: 7px 10px;
  border: 0;
  border-radius: 6px;
  background: var(--el-fill-color-light);
  color: var(--el-text-color-regular);
  font: inherit;
  font-size: 12px;
  text-align: left;
  cursor: pointer;
}

.form-group-head:hover {
  background: var(--el-fill-color);
}

.form-group-title {
  font-weight: 600;
}

.form-group-summary {
  margin-left: auto;
  overflow: hidden;
  color: var(--el-text-color-secondary);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.form-group-chevron {
  flex-shrink: 0;
  width: 12px;
  height: 12px;
  color: var(--el-text-color-secondary);
  transition: transform 0.15s ease;
}

.form-group.is-open .form-group-chevron {
  transform: rotate(90deg);
}

.form-group-body {
  padding-top: 12px;
}

/* 头改写细则：几行控件 + 一张「浏览器 → 远端」对照表 */
.rewrite-editor {
  width: 100%;
  margin-top: 8px;
  padding: 10px;
  border: 1px solid var(--el-border-color-lighter);
  border-radius: 6px;
  background: var(--el-fill-color-lighter);
}

.rewrite-row {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-bottom: 8px;
}

/* 开关关闭时细则只读：控件变灰，但预览保持可读（用户就是要看这个） */
.rewrite-editor.is-readonly {
  background: var(--el-fill-color-light);
}

/* 标签列放得下「Location」：窄了会折成 Locatio / n */
.rewrite-name {
  flex-shrink: 0;
  width: 68px;
  font-size: 12px;
  white-space: nowrap;
  color: var(--el-text-color-secondary);
}

.rewrite-preview {
  margin-top: 4px;
  padding-top: 8px;
  border-top: 1px dashed var(--el-border-color);
  font-size: 12px;
}

.rewrite-preview-title {
  margin-bottom: 6px;
  color: var(--el-text-color-secondary);
}

.rewrite-preview-row {
  display: flex;
  align-items: baseline;
  gap: 6px;
  padding: 2px 0;
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
}

.rewrite-preview-name {
  flex-shrink: 0;
  width: 150px;
  color: var(--el-text-color-regular);
  white-space: nowrap;
}

.rewrite-preview-from {
  color: var(--el-text-color-secondary);
}

/* 原本就有的值才划掉；「—」表示浏览器没发这个头，划它没意义 */
.rewrite-preview-row.is-from .rewrite-preview-from {
  text-decoration: line-through;
}

.rewrite-preview-arrow {
  color: var(--el-text-color-placeholder);
}

.rewrite-preview-to {
  color: var(--el-color-primary);
  word-break: break-all;
}

/* 不发送/不改写的行：整行变淡，用一句话说清楚，不画箭头 */
.rewrite-preview-row.is-off {
  opacity: 0.6;
}

.rewrite-preview-note {
  color: var(--el-text-color-secondary);
}
</style>

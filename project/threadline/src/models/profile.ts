/**
 * 配置包（`.threadline`）的结构与读写。
 *
 * 分工：这一层只管**归档格式**——勾选收集、序列化、图标校验、包大小限制。
 * 「这条能不能建、端口归谁」是 Core 的规则（`profile/plan`），这里不重复判断，
 * 否则必然出现"预览说能建、落地却失败"。
 */

import type { CredentialKind, ServerDto, TunnelDto } from '../api/types';
import { zip, unzip, type ZipEntry } from './zip';

export const PROFILE_KIND = 'threadline.profile';
export const PROFILE_VERSION = 1;
export const PROFILE_EXT = '.threadline';

/** 单张图标上限。Core 的 `normalize_icon` 只接受 200KB 字符，base64 后约 1.33 倍。 */
export const MAX_ICON_BYTES = 128 * 1024;
/** 整包上限（读取侧），防止误选一个大文件把界面卡住。 */
export const MAX_PACKAGE_BYTES = 10 * 1024 * 1024;
const MAX_ENTRIES = 200;
const MAX_TOTAL_BYTES = 20 * 1024 * 1024;
const ICON_MIME = /^data:image\/(png|jpeg|webp|svg\+xml);base64,/;

export interface ProfileForward {
  id: string;
  name: string;
  local_host: string;
  local_port: number;
  remote_host: string;
  remote_port: number;
  scheme: string;
  domain: string;
  remote_scheme?: string | null;
  open_path: string;
  note: string;
  group: string;
  in_app: boolean;
  auto_start: boolean;
  /** 包内路径（`icons/xxx.png`）或外链；空 = 无自定义图标。 */
  icon?: string;
}

export interface ProfileServer {
  id: string;
  name: string;
  host: string;
  port: number;
  username: string;
  credential_kind: CredentialKind;
  keepalive_secs: number;
  host_key_fingerprint?: string | null;
  forwards: ProfileForward[];
}

export interface ProfilePreferences {
  theme: string;
  hotkey: string;
}

export interface ProfileDocument {
  kind: typeof PROFILE_KIND;
  version: number;
  exported_at: string;
  app_version: string;
  servers: ProfileServer[];
  preferences?: ProfilePreferences;
}

export interface BuiltPackage {
  document: ProfileDocument;
  /** 包内路径 → 文件字节。 */
  icons: Map<string, Uint8Array>;
}

export interface ParsedPackage {
  document: ProfileDocument;
  /** 包内路径 → data URI（图标已被校验与限流）。 */
  icons: Map<string, string>;
}

export interface BuildOptions {
  servers: ServerDto[];
  forwards: TunnelDto[];
  /** 被勾选的连接：整条带走。 */
  serverIds: string[];
  /** 被单独勾选的转发：自动补上所属连接。 */
  forwardIds: string[];
  includeIcons: boolean;
  includeFingerprint: boolean;
  preferences?: ProfilePreferences;
  appVersion?: string;
  now?: Date;
}

/**
 * 按勾选组装包内容。
 *
 * 勾连接 = 带它下面全部转发；单勾转发 = 只带这一条并补上所属连接。
 * 连接被单独勾中转发的场景下，`serverIds` 不含它，所以判据是「连接被勾 或 该转发被勾」。
 */
export function buildProfile(opts: BuildOptions): BuiltPackage {
  const icons = new Map<string, Uint8Array>();
  const selectedForwards = new Set(opts.forwardIds);
  const selectedServers = new Set(opts.serverIds);

  // 单勾的转发把所属连接也拉进来（去重靠 Set）
  for (const forward of opts.forwards) {
    if (selectedForwards.has(forward.id)) {
      selectedServers.add(forward.server_id);
    }
  }

  const servers = opts.servers
    .filter((server) => selectedServers.has(server.id))
    .map<ProfileServer>((server) => ({
      id: server.id,
      name: server.name,
      host: server.host,
      port: server.port,
      username: server.username,
      credential_kind: server.credential_kind ?? 'password',
      keepalive_secs: server.keepalive_secs,
      host_key_fingerprint: opts.includeFingerprint
        ? (server.host_key_fingerprint ?? null)
        : null,
      forwards: opts.forwards
        .filter(
          (forward) =>
            forward.server_id === server.id &&
            (selectedServers.has(server.id) || selectedForwards.has(forward.id)),
        )
        .map((forward) => toProfileForward(forward, opts.includeIcons, icons)),
    }));

  return {
    document: {
      kind: PROFILE_KIND,
      version: PROFILE_VERSION,
      exported_at: (opts.now ?? new Date()).toISOString(),
      app_version: opts.appVersion ?? '0.1.0',
      servers,
      ...(opts.preferences ? { preferences: opts.preferences } : {}),
    },
    icons,
  };
}

function toProfileForward(
  forward: TunnelDto,
  includeIcons: boolean,
  icons: Map<string, Uint8Array>,
): ProfileForward {
  return {
    id: forward.id,
    name: forward.name,
    local_host: forward.local_host ?? '127.0.0.1',
    local_port: forward.local_port,
    remote_host: forward.remote_host,
    remote_port: forward.remote_port,
    scheme: forward.scheme ?? 'tcp',
    domain: forward.domain ?? '',
    remote_scheme: forward.remote_scheme ?? null,
    open_path: forward.open_path ?? '',
    note: forward.note ?? '',
    group: forward.group ?? '',
    in_app: forward.in_app ?? true,
    auto_start: forward.auto_start,
    icon: includeIcons ? packIcon(forward.icon, icons) : '',
  };
}

/**
 * 图标两条路：
 * - `data:image/*`：拆成 `icons/<内容哈希>.<ext>` 文件，相同图标只存一份
 * - `http(s)`：保留原样（不占包体积，对方能访问就还能用）
 */
function packIcon(icon: string | undefined, icons: Map<string, Uint8Array>): string {
  if (!icon) {
    return '';
  }
  if (/^https?:\/\//.test(icon)) {
    return icon;
  }
  if (!icon.startsWith('data:image/')) {
    return '';
  }
  const bytes = decodeDataUri(icon);
  if (!bytes || bytes.length > MAX_ICON_BYTES) {
    return '';
  }
  const ext = icon.startsWith('data:image/png')
    ? 'png'
    : icon.startsWith('data:image/jpeg')
      ? 'jpg'
      : icon.startsWith('data:image/webp')
        ? 'webp'
        : 'svg';
  const path = `icons/${shortHash(icon)}.${ext}`;
  if (!icons.has(path)) {
    icons.set(path, bytes);
  }
  return path;
}

/** 打包成字节流。README 一并写入：别人解压后能看懂这是什么。 */
export function writePackage(built: BuiltPackage, now = new Date()): Uint8Array {
  const encoder = new TextEncoder();
  const entries: ZipEntry[] = [
    {
      name: 'profile.json',
      data: encoder.encode(JSON.stringify(built.document, null, 2)),
    },
  ];
  for (const [name, data] of built.icons) {
    entries.push({ name, data });
  }
  entries.push({ name: 'README.txt', data: encoder.encode(README) });
  return zip(entries, now);
}

const README = [
  'Threadline 配置文件',
  '',
  '这个包里只有「隧道（SSH 连接）」与「转发」的结构，以及可选的图标与偏好。',
  '不含任何密码、私钥或网关证书——导入后请自行填写登录材料。',
  '',
  '导入方式：Threadline → 设置 → 隧道管理 → 导入配置。',
].join('\r\n');

/** 读取并校验包。任何一项不合法都直接抛错，不返回半成品。 */
export async function readPackage(bytes: Uint8Array): Promise<ParsedPackage> {
  if (bytes.length > MAX_PACKAGE_BYTES) {
    throw new Error('文件太大，不是有效的配置包');
  }
  // 单条目上限放宽到 2MB（profile.json 可能很长），图标大小在 toDataUri 里再卡 128KB
  const files = await unzip(bytes, {
    maxEntries: MAX_ENTRIES,
    maxEntryBytes: 2 * 1024 * 1024,
    maxTotalBytes: MAX_TOTAL_BYTES,
  });

  const json = files.get('profile.json');
  if (!json) {
    throw new Error('这个包里没有 profile.json');
  }
  let document: ProfileDocument;
  try {
    document = JSON.parse(new TextDecoder().decode(json)) as ProfileDocument;
  } catch {
    throw new Error('profile.json 不是合法的 JSON');
  }
  if (document.kind !== PROFILE_KIND) {
    throw new Error('这不是 Threadline 配置文件');
  }
  if (document.version !== PROFILE_VERSION) {
    throw new Error(`配置文件版本不支持（${String(document.version)}）`);
  }
  if (!Array.isArray(document.servers)) {
    throw new Error('profile.json 结构不完整');
  }

  const icons = new Map<string, string>();
  for (const [name, data] of files) {
    if (!isIconPath(name)) {
      continue;
    }
    const uri = toDataUri(name, data);
    if (uri) {
      icons.set(name, uri);
    }
  }

  // 图标丢失（超限/格式不对）时把引用一并清掉，免得导入时指向不存在的文件
  for (const server of document.servers) {
    for (const forward of server.forwards ?? []) {
      if (forward.icon?.startsWith('icons/') && !icons.has(forward.icon)) {
        forward.icon = '';
      }
    }
  }

  return { document, icons };
}

function isIconPath(name: string): boolean {
  if (!name.startsWith('icons/')) {
    return false;
  }
  // 只接受扁平文件名：拒绝 `..`、绝对路径、嵌套目录
  const rest = name.slice('icons/'.length);
  return rest.length > 0 && !rest.includes('/') && !rest.includes('\\') && !rest.includes('..');
}

function toDataUri(name: string, data: Uint8Array): string | null {
  if (data.length === 0 || data.length > MAX_ICON_BYTES) {
    return null;
  }
  const mime = name.endsWith('.png')
    ? 'image/png'
    : name.endsWith('.jpg') || name.endsWith('.jpeg')
      ? 'image/jpeg'
      : name.endsWith('.webp')
        ? 'image/webp'
        : name.endsWith('.svg')
          ? 'image/svg+xml'
          : '';
  if (!mime) {
    return null;
  }
  return `data:${mime};base64,${bytesToBase64(data)}`;
}

function decodeDataUri(uri: string): Uint8Array | null {
  if (!ICON_MIME.test(uri)) {
    return null;
  }
  const comma = uri.indexOf(',');
  if (comma < 0) {
    return null;
  }
  try {
    const binary = atob(uri.slice(comma + 1));
    const out = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i += 1) {
      out[i] = binary.charCodeAt(i);
    }
    return out;
  } catch {
    return null;
  }
}

function bytesToBase64(data: Uint8Array): string {
  let binary = '';
  const step = 0x8000;
  for (let i = 0; i < data.length; i += step) {
    binary += String.fromCharCode(...data.subarray(i, i + step));
  }
  return btoa(binary);
}

/** FNV-1a 的 32 位版本，转 36 进制：只用来给图标起一个稳定短名。 */
export function shortHash(text: string): string {
  let hash = 0x811c9dc5;
  for (let i = 0; i < text.length; i += 1) {
    hash ^= text.charCodeAt(i);
    hash = Math.imul(hash, 0x01000193) >>> 0;
  }
  return hash.toString(36).padStart(7, '0').slice(0, 7);
}

/** 导出文件名：日期 + 固定后缀，便于按时间排序。 */
export function packageFileName(now = new Date()): string {
  const y = now.getFullYear();
  const m = String(now.getMonth() + 1).padStart(2, '0');
  const d = String(now.getDate()).padStart(2, '0');
  return `${y}-${m}-${d}-threadline${PROFILE_EXT}`;
}

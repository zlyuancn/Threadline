import { localUrl, loopbackHost } from './localUrl';
import type { LocalScheme } from '../api/types';

/** 转发卡片的纯展示逻辑：只依赖数据，不依赖组件上下文。 */

export interface ForwardLike {
  definition: {
    local_host: string;
    local_port: number;
    remote_host: string;
    remote_port: number;
    scheme?: string;
    open_path?: string;
    domain?: string;
    domain_host?: string;
  };
  status: { state: string; active_connections: number };
}

/**
 * 入口地址（卡片「本机」那一行）。
 *
 * 有域名时显示域名——那才是用户实际要访问的东西；`domain_host` 是内部实现的回环地址，
 * 只在 title 里提示。
 */
export function entryLabel(view: ForwardLike): string {
  const domain = (view.definition.domain ?? '').trim();
  if (domain) {
    return `${domain}:${view.definition.local_port}`;
  }
  return `${view.definition.local_host}:${view.definition.local_port}`;
}

/** 只有 http/https 才有"打开本地页面"这回事。 */
export function isWeb(view: ForwardLike): boolean {
  const scheme = schemeOf(view);
  return scheme === 'http' || scheme === 'https';
}

/**
 * 归一化后的本地入口协议。
 *
 * 老配置（没有 `scheme`）与手改文件里的脏值一律按 `tcp` 处理——
 * 界面只认这五种，别让一个拼错的字符串决定图标或按钮能不能点。
 */
export type { LocalScheme };

const SCHEMES: readonly LocalScheme[] = ['tcp', 'http', 'https', 'rdp', 'ssh'];

export function schemeOf(view: ForwardLike): LocalScheme {
  const raw = (view.definition.scheme ?? '').trim().toLowerCase();
  return (SCHEMES as readonly string[]).includes(raw) ? (raw as LocalScheme) : 'tcp';
}

/** 协议短标签（磁贴地址前的徽标）：HTTP / HTTPS / SSH / RDP / TCP。 */
export function schemeLabel(view: ForwardLike): string {
  return schemeOf(view).toUpperCase();
}

/** 能不能用系统默认程序直接打开：只有 http/https 可以。 */
export function canOpen(view: ForwardLike): boolean {
  return isWeb(view);
}

/**
 * 本地访问方式（展示用）。
 *
 * web 是完整 URL（根路径的尾斜杠去掉，看着更像一个入口）；其余协议只给
 * `host:port`——SSH / RDP / 裸 TCP 需要各自客户端，写 `ssh://` 反而会被
 * 误当成可以点的链接。
 */
export function endpointLabel(view: ForwardLike): string {
  if (isWeb(view)) {
    return urlOf(view).replace(/\/$/, '');
  }
  const def = view.definition;
  return `${loopbackHost((def.local_host ?? '').trim())}:${def.local_port}`;
}

/**
 * 自动图标：web 转发用目标站点自己的 favicon。
 *
 * 不把任何站点名或图标文件写进代码——图标来自用户自己配置的入口地址，
 * 运行时加载；拿不到时界面回退到协议图标。
 */
export function faviconUrl(view: ForwardLike): string {
  if (!isWeb(view)) {
    return '';
  }
  try {
    return `${new URL(urlOf(view)).origin}/favicon.ico`;
  } catch {
    return '';
  }
}

/** 这条转发是否处于「已经/正在跑」的状态：菜单据此在启动与停止间切换。 */
export function isActive(state: string): boolean {
  return (
    state === 'running' ||
    state === 'connecting' ||
    state === 'needs_trust' ||
    state === 'backoff' ||
    state === 'reconnecting'
  );
}

/**
 * 本地访问地址。
 *
 * 路径统一在写配置时归一到"以 / 开头"，这里只负责拼接——但仍然兜一次底，
 * 免得老数据（没有 open_path 的、或者手工改过文件的）拼出 `//` 或缺少分隔。
 */
export function urlOf(view: ForwardLike): string {
  const scheme = schemeOf(view) === 'https' ? 'https' : 'http';
  const domain = (view.definition.domain ?? '').trim();
  const base = domain
    ? withOmittedDefaultPort(domain, view.definition.local_port, scheme)
    : localUrl(view.definition.local_host, view.definition.local_port, scheme);
  const path = (view.definition.open_path ?? '').trim();
  if (!path || path === '/') {
    return base;
  }
  return base.replace(/\/$/, '') + (path.startsWith('/') ? path : `/${path}`);
}

/** 域名入口：80/443 是浏览器默认端口，写出来反而显得奇怪。 */
function withOmittedDefaultPort(host: string, port: number, scheme: 'http' | 'https'): string {
  const omit = (scheme === 'http' && port === 80) || (scheme === 'https' && port === 443);
  return omit ? `${scheme}://${host}` : `${scheme}://${host}:${port}`;
}

export function pillOf(state: string): string {
  if (state === 'running') {
    return 'is-on';
  }
  if (state === 'error') {
    return 'is-bad';
  }
  if (state === 'connecting' || state === 'backoff' || state === 'reconnecting') {
    return 'is-warn';
  }
  return 'is-off';
}

export function dotOf(state: string): string {
  if (state === 'running') {
    return 'is-on';
  }
  if (state === 'error') {
    return 'is-bad';
  }
  return '';
}

export function connLabel(view: ForwardLike): string {
  const n = view.status.active_connections;
  return n > 0 ? `累计 ${n} 个连接` : '暂无连接';
}

/** 搜索用：名称 / 备注 / 两个地址 / 两个端口都要能命中。 */
export function matchesQuery(view: ForwardLike, extra: string[], query: string): boolean {
  const q = query.trim().toLowerCase();
  if (!q) {
    return true;
  }
  const def = view.definition;
  const haystack = [
    def.local_host,
    def.domain ?? '',
    String(def.local_port),
    def.remote_host,
    String(def.remote_port),
    ...extra,
  ]
    .join(' ')
    .toLowerCase();
  return haystack.includes(q);
}

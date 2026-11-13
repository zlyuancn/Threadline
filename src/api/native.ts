// 桌面壳能力（只有 Tauri 环境可用）。浏览器里打开时这些调用会明确失败，
// 而不是静默什么都不做。

import { invoke } from '@tauri-apps/api/core';

export function isTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

/** 通过唤醒入口 `tl.exe` 把核心拉起来（核心已退出后 HTTP 不可用，只能走这条路）。 */
export async function startCore(): Promise<void> {
  if (!isTauri()) {
    throw new Error('启动服务只能在桌面程序里使用；浏览器模式请直接运行 tl.exe');
  }
  await invoke('start_core');
}

/**
 * 重新读取 `control.json` 并注入 `window.__THREADLINE__`。
 * 核心每次启动都会轮换 token，拉起来之后必须重新注入，否则旧 token 会被 401。
 */
/**
 * 由助手弹一次 UAC，以管理员身份跑 `tl-core --autostart on|off`。
 * 核心以用户身份运行时改不了 per-machine 任务计划，这是唯一路径。
 */
export async function setAutostartElevated(enabled: boolean): Promise<void> {
  if (!isTauri()) {
    throw new Error('提权只能在桌面程序里进行');
  }
  await invoke('set_autostart_elevated', { enabled });
}

export interface KnownHostLookup {
  found: boolean;
  hashed: boolean;
  fingerprint: string | null;
}

/** 读一次核心状态（与托盘同一判据，由 agent 的探活线程维护）。 */
export async function coreState(): Promise<boolean | null> {
  if (!isTauri()) {
    return null;
  }
  try {
    return await invoke<boolean>('core_state');
  } catch {
    return null;
  }
}

/**
 * 订阅核心状态变化。托盘与面板在同一进程里，状态由 agent 单点维护并推给 WebView，
 * 两边不会再各判一次而出现不一致。
 */
export async function onCoreStateChange(
  handler: (alive: boolean) => void,
): Promise<() => void> {
  if (!isTauri()) {
    return () => {};
  }
  const { listen } = await import('@tauri-apps/api/event');
  const unlisten = await listen<boolean>('core-state', (event) => handler(event.payload));
  return unlisten;
}

/** 查本机 `~/.ssh/known_hosts` 对某台主机的记录，作为确认指纹时的参照物。 */
export async function knownHostsFingerprint(
  host: string,
  port: number,
): Promise<KnownHostLookup> {
  if (!isTauri()) {
    return { found: false, hashed: false, fingerprint: null };
  }
  try {
    return await invoke<KnownHostLookup>('known_hosts_fingerprint', { host, port });
  } catch {
    return { found: false, hashed: false, fingerprint: null };
  }
}

/** 读取本机 `~/.ssh` 下的私钥内容（仅当用户点"导入"时调用）。 */
export async function readLocalPrivateKey(name?: string): Promise<{ path: string; pem: string }> {
  if (!isTauri()) {
    throw new Error('读取本机私钥只能在桌面程序里进行');
  }
  return await invoke<{ path: string; pem: string }>('read_local_private_key', {
    name: name ?? null,
  });
}

/** 让 Windows 标题栏跟随主题配色（Windows 11 起生效，Win10 会忽略）。 */
export async function setWindowCaption(caption: string, text: string): Promise<void> {
  if (!isTauri()) {
    return;
  }
  try {
    await invoke('set_window_caption', { caption, text });
  } catch {
    // 系统不支持时静默忽略，界面不该因此报错。
  }
}

/** 列出 `~/.ssh` 下可用的私钥文件（多把时让用户选）。 */
export async function listSshKeys(): Promise<{ name: string; path: string }[]> {
  if (!isTauri()) {
    return [];
  }
  try {
    return await invoke<{ name: string; path: string }[]>('list_local_private_keys');
  } catch {
    return [];
  }
}

/**
 * 主动拉取发现契约并写入 `window.__THREADLINE__`。
 *
 * **不要依赖 agent 的 eval 注入**：页面刷新（F5）会把注入的全局变量抹掉，
 * 于是所有请求退回到没有 baseUrl 的状态，界面就显示「核心不可达」。
 * 这里每次加载都自己取一次。
 */
export async function refreshBootstrap(): Promise<boolean> {
  if (!isTauri()) {
    return false;
  }
  try {
    const snapshot = await invoke<{ baseUrl: string; token: string } | null>(
      'bootstrap_snapshot',
    );
    if (!snapshot) {
      window.__THREADLINE__ = undefined;
      return false;
    }
    window.__THREADLINE__ = { baseUrl: snapshot.baseUrl, token: snapshot.token };
    return true;
  } catch {
    return false;
  }
}

/**
 * 用系统默认浏览器打开本机转发出来的地址。
 * 浏览器调试环境退化成新标签页——这两个场景的意图是一样的。
 */
export async function openExternal(url: string): Promise<void> {
  if (!isTauri()) {
    window.open(url, '_blank', 'noopener');
    return;
  }
  await invoke('open_external', { url });
}

export interface DataDirStatus {
  path: string;
  writable: boolean;
}

/** 配置目录是否可写；不可写时界面要先提示，而不是等保存失败。 */
export async function getDataDirStatus(): Promise<DataDirStatus | null> {
  if (!isTauri()) {
    return null;
  }
  try {
    return await invoke<DataDirStatus>('data_dir_status');
  } catch {
    return null;
  }
}

/** 弹一次 UAC，把配置目录的修改权限补给当前用户。 */
export async function fixDataDirAcl(): Promise<string> {
  return invoke<string>('fix_data_dir_acl');
}

/**
 * 按"是否开机自启"同步用户级登录启动项（HKCU\...\Run）。
 *
 * 开机自启只保证核心（SYSTEM 任务）在登录前起来；登录后的托盘需要这一条。
 * 由助手自己写：HKCU 是每用户的，SYSTEM 进程写不到用户的 hive。
 */
export async function syncLogonEntry(enabled: boolean): Promise<string> {
  if (!isTauri()) {
    return '';
  }
  return invoke<string>('sync_logon_entry', { enabled });
}

/**
 * 提权写一次 hosts（UAC）。
 *
 * `POST /hosts/sync` 返回 409（需要提权）后调用；成功与否以 Core 重新拉取的
 * `GET /hosts` 为准，不靠这里猜。
 */
export async function syncHostsElevated(): Promise<void> {
  if (!isTauri()) {
    throw new Error('写入 hosts 只能在桌面程序里进行');
  }
  await invoke('sync_hosts');
}

export interface ImportedIcon {
  mime: string;
  base64: string;
}

/**
 * 下载远程图标的原始字节。
 *
 * 不走 WebView 的 fetch/canvas：跨域图片会把 canvas 污染，`toDataURL` 直接抛错。
 * 桌面程序里由 Agent 下载；浏览器预览时退化成普通 fetch（可能受 CORS 限制）。
 */
export async function importIcon(url: string): Promise<ImportedIcon> {
  if (!isTauri()) {
    const response = await fetch(url);
    if (!response.ok) {
      throw new Error(`下载图标失败：HTTP ${response.status}`);
    }
    const blob = await response.blob();
    const base64 = await new Promise<string>((resolve, reject) => {
      const reader = new FileReader();
      reader.onload = () => resolve(String(reader.result).split(',')[1] ?? '');
      reader.onerror = () => reject(new Error('读取图标失败'));
      reader.readAsDataURL(blob);
    });
    return { mime: blob.type || 'image/png', base64 };
  }
  return invoke<ImportedIcon>('import_icon', { url });
}

export interface ProxyReport {
  ok: boolean;
  applied: string[];
  enabled: boolean | null;
  pac: boolean | null;
  message: string | null;
}

/**
 * 让 Agent（登录用户身份）按 reconcile 规则改写 `ProxyOverride` 并广播。
 *
 * `desired` / `managed` 从 `GET /hosts` 的 `proxy` 字段取；返回值转成
 * `ProxySyncResult` 回报 Core，管理集以它为准。
 */
export async function syncProxyOverride(
  desired: string[],
  managed: string[],
): Promise<ProxyReport> {
  if (!isTauri()) {
    throw new Error('代理例外只能在桌面程序里同步');
  }
  return invoke<ProxyReport>('sync_proxy_override', { desired, managed });
}

export interface InstallCertResult {
  ok: boolean;
  code: number | null;
  message: string | null;
}

/**
 * 把网关根证书装进当前用户根存储。
 *
 * 会弹 **Windows 系统安全警告**——调用前必须先让用户确认；用户点「否」时
 * `ok=false`、`code` 为 certutil 的退出码。
 *
 * `thumbprintSha1` 传 `GatewayCaStatus.thumbprint_sha1`：Agent 装完会查库确认，
 * 不只看退出码（退出码 0 但没进库的环境存在）。
 */
export async function installGatewayCa(
  thumbprintSha1?: string | null,
): Promise<InstallCertResult> {
  if (!isTauri()) {
    throw new Error('安装证书只能在桌面程序里进行');
  }
  return invoke<InstallCertResult>('install_gateway_ca', {
    thumbprintSha1: thumbprintSha1 ?? null,
  });
}

/**
 * 查当前用户根库里有没有这张根（Core 是 SYSTEM 时查不到）。
 *
 * 必须传 **SHA-1**：`certutil -store Root` 按 SHA-1 匹配，传 SHA-256 会 NOT_FOUND。
 */
export async function gatewayCaTrust(thumbprintSha1: string): Promise<boolean> {
  if (!isTauri()) {
    return false;
  }
  try {
    return await invoke<boolean>('gateway_ca_trust', { thumbprintSha1 });
  } catch {
    return false;
  }
}

/** 导出根证书 PEM：无可见桌面会话时装不了，退化成让用户自己导入。 */
export async function exportGatewayCa(): Promise<string | null> {
  if (!isTauri()) {
    return null;
  }
  try {
    return await invoke<string>('export_gateway_ca');
  } catch {
    return null;
  }
}

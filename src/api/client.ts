// 唯一 HTTP 出口。views 层禁止直连 fetch/axios（由 tools/msocheck 的 R3 拦截）。
//
// 端点地址来源优先级：
//   1. Tauri 壳注入的 `window.__THREADLINE__`（打包运行时的正常路径）
//   2. 开发模式：同源 `/api` 由 vite 代理到本机核心（token 从 localStorage 取，便于手工调试）

import type { ErrorBody } from './types';

export interface Bootstrap {
  baseUrl: string;
  token: string;
}

declare global {
  interface Window {
    __THREADLINE__?: Bootstrap;
  }
}

export class ApiFailure extends Error {
  readonly status: number;
  readonly code: string;
  readonly field?: string;

  constructor(status: number, code: string, message: string, field?: string) {
    super(message);
    this.status = status;
    this.code = code;
    this.field = field;
  }

  /** 需要一次提权（界面据此出横幅 + 引导）。 */
  get needsElevation(): boolean {
    return this.code === 'privilege_required';
  }

  /** 这条隧道还没填密码（导入的连接）：提示「先去编辑填一次」，不弹红色错误。 */
  get credentialMissing(): boolean {
    return this.code === 'credential_missing';
  }
}

const TOKEN_KEY = 'tl.token';

/** 单次请求的上界：本地回环正常在毫秒级，超过这个数说明核心卡住了。 */
const REQUEST_TIMEOUT_MS = 15_000;

export function bootstrap(): Bootstrap {
  if (typeof window !== 'undefined' && window.__THREADLINE__) {
    return window.__THREADLINE__;
  }
  return {
    baseUrl: '',
    token: localStorage.getItem(TOKEN_KEY) ?? '',
  };
}

export function setDevToken(token: string): void {
  localStorage.setItem(TOKEN_KEY, token);
}

export function hasToken(): boolean {
  return bootstrap().token.length > 0;
}

function url(path: string): string {
  const { baseUrl } = bootstrap();
  return `${baseUrl}/api/v1${path}`;
}

async function request<T>(
  method: string,
  path: string,
  body?: unknown,
  retriedAuth = false,
  /**
   * 这些状态码**不是错误**，照常返回响应体。
   *
   * 目前只有一处：`POST /hosts/sync` 用 409 表示「需要提权」，
   * 响应体里带着完整状态。若按普通错误抛出，前端就永远走不到「调 Agent 提权」
   * 那条分支，只能看到一个 409 错误横幅。
   */
  allowStatus: number[] = [],
  /**
   * 本次请求的超时。默认 15s；只有批量操作（导入配置）放宽——
   * 200 条逐条落地会超时，而那并不是"核心卡死"。
   */
  timeoutMs = REQUEST_TIMEOUT_MS,
): Promise<T> {
  const { token } = bootstrap();
  const headers: Record<string, string> = {};
  if (token) {
    headers.Authorization = `Bearer ${token}`;
  }
  if (body !== undefined) {
    headers['Content-Type'] = 'application/json';
  }

  const endpoint = url(path);
  let response: Response;
  try {
    response = await fetch(endpoint, {
      method,
      headers,
      body: body === undefined ? undefined : JSON.stringify(body),
      // 每个请求都有上界：核心卡死时不能把界面吊在半空。
      signal: AbortSignal.timeout(timeoutMs),
    });
  } catch (e) {
    // 带上真实请求地址：这样"契约没注入（baseUrl 为空）"与"核心不可达/CORS"能区分开。
    const hint = window.__THREADLINE__
      ? `核心不可达或跨源被拦（${endpoint}）`
      : `未注入发现契约（Agent 没读到 control.json），请求目标是 ${endpoint}`;
    throw new ApiFailure(0, 'network_error', `${hint}：${String(e)}`);
  }

  if (response.status === 204) {
    return undefined as T;
  }

  const text = await response.text();
  const payload = text ? safeJson(text) : undefined;

  // 核心每次启动都会轮换 token：401 时重新注入一次再重试，
  // 否则"停止服务/启动服务"之后整个界面会一直 401，只能按 F5。
  if (response.status === 401 && !retriedAuth) {
    const { refreshBootstrap } = await import('./native');
    if (await refreshBootstrap()) {
      return request<T>(method, path, body, true, allowStatus, timeoutMs);
    }
  }

  if (!response.ok && allowStatus.includes(response.status)) {
    return payload as T;
  }

  if (!response.ok) {
    const body = payload as ErrorBody | undefined;
    throw new ApiFailure(
      response.status,
      body?.error?.code ?? 'http_error',
      body?.error?.message ?? `请求失败（HTTP ${response.status}）`,
      body?.error?.field,
    );
  }

  return payload as T;
}

function safeJson(text: string): unknown {
  try {
    return JSON.parse(text);
  } catch {
    return undefined;
  }
}

export const http = {
  get: <T>(path: string) => request<T>('GET', path),
  post: <T>(path: string, body?: unknown, allowStatus?: number[]) =>
    request<T>('POST', path, body, false, allowStatus),
  put: <T>(path: string, body?: unknown) => request<T>('PUT', path, body),
  patch: <T>(path: string, body?: unknown) => request<T>('PATCH', path, body),
  del: <T>(path: string) => request<T>('DELETE', path),
  /**
   * 批量操作专用：默认 60s。
   *
   * 导入配置是逐条落地，200 条连接 + 转发不是"核心卡住"，
   * 用 15s 会把它掐断，界面只能看到一个超时。
   */
  postSlow: <T>(path: string, body?: unknown, timeoutMs = 60_000) =>
    request<T>('POST', path, body, false, [], timeoutMs),
};

/** 健康探活（免鉴权），返回核心的 `instance_id` 以便与 `control.json` 比对。 */
export async function healthz(): Promise<{ status: string; version: string; instance_id: string }> {
  return http.get('/healthz');
}

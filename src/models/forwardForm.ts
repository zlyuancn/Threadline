/**
 * 端口弹窗里「切协议」的规则。
 *
 * 抽成纯函数的原因：原生 `<select>` 是 `:value` 单向绑定，写回状态这一步一旦漏掉，
 * 表现就是「下拉选了没反应，界面还停在旧协议」——这种 bug 靠肉眼看代码容易漏，
 * 放到单测里钉住。
 */
import type { LocalScheme } from '../api/types';

/** 本机用哪种协议访问；rdp/ssh 本质是 TCP，只是默认端口不同。 */
export type Scheme = LocalScheme;

/**
 * 本地协议变了：入口端口按格给默认值（① 80 / ② 443 / ③ 远端端口）。
 *
 * 不能统一用 remote_port：② 的远端是 80，默认填 80 会让 `https://域名` 直接连不上。
 */
export const DEFAULT_LOCAL_PORT: Record<Scheme, number | null> = {
  http: 80,
  https: 443,
  rdp: 3389,
  ssh: 22,
  tcp: null,
};

/** 远端端口的协议默认值：选 RDP 就该是 3389，选 SSH 就该是 22。 */
export const DEFAULT_REMOTE_PORT: Partial<Record<Scheme, number>> = {
  http: 8080,
  https: 443,
  rdp: 3389,
  ssh: 22,
};

/** 这些值视为「没改过」，切协议时允许被覆盖；用户填的其它端口一律保留。 */
export const COMMON_REMOTE_PORTS = new Set([22, 80, 443, 3389, 8080]);

/** 切换协议至少要动这三个字段。 */
export interface SchemeEditable {
  scheme: Scheme;
  local_port: number;
  remote_port: number;
}

/**
 * 切换本地协议：先写回协议本身，再按协议给端口默认值。
 *
 * - **协议必须写回**：原生 select 没有 `v-model`，漏了这一步界面就「选了没反应」。
 * - 端口跟着协议走：HTTP→HTTPS 就是 80→443，不能因为之前手填过就保留旧值，
 *   否则 `https://域名:80` 会直接连不上。
 */
export function applySchemeChange<T extends SchemeEditable>(form: T, value: Scheme): void {
  form.scheme = value;
  const remotePreset = DEFAULT_REMOTE_PORT[value];
  if (remotePreset !== undefined && COMMON_REMOTE_PORTS.has(form.remote_port)) {
    form.remote_port = remotePreset;
  }
  const localPreset = DEFAULT_LOCAL_PORT[value];
  form.local_port = localPreset ?? form.remote_port;
}

/**
 * 「HTTP 头改写」的表单模型与预览。
 *
 * 后端只认 `HeaderRewrite`（契约里的结构）；界面要展示「把什么改成了什么」，
 * 这层负责把表单值翻译成一句一句可读的对照，纯函数、可测。
 */
import type { HeaderPair, HeaderRewrite } from '../api/types';

/** Host 的三种处理方式。用显式字段存：从值反推会把「自定义但还没填」误判成「保留原值」。 */
export type HostMode = 'auto' | 'keep' | 'custom';

/** 表单里的改写细则。 */
export interface HeaderRewriteForm {
  host_mode: HostMode;
  /** 仅 `host_mode === 'custom'` 时有意义 */
  host: string | null;
  forward_headers: boolean;
  rewrite_location: boolean;
  extra: HeaderPair[];
}

export function defaultHeaderRewrite(): HeaderRewriteForm {
  return {
    host_mode: 'auto',
    host: null,
    forward_headers: true,
    rewrite_location: true,
    extra: [],
  };
}

/** 由契约值反推三态：`null` = 自动，空串 = 保留，其它 = 自定义。 */
export function hostModeOf(form: HeaderRewriteForm): HostMode {
  if (form.host_mode) {
    return form.host_mode;
  }
  if (form.host === null) {
    return 'auto';
  }
  return form.host === '' ? 'keep' : 'custom';
}

/** 契约 → 表单。老配置缺字段时按默认值补齐。 */
export function toFormRewrite(raw?: HeaderRewrite | null): HeaderRewriteForm {
  const host = raw?.host ?? null;
  return {
    host_mode: host === null ? 'auto' : host === '' ? 'keep' : 'custom',
    host,
    forward_headers: raw?.forward_headers !== false,
    rewrite_location: raw?.rewrite_location !== false,
    extra: (raw?.extra ?? []).map((pair) => ({
      name: pair.name ?? '',
      value: pair.value ?? '',
    })),
  };
}

/** 表单 → 契约。空名的自定义头是「点了添加还没填」，直接丢掉。 */
export function toDtoRewrite(form: HeaderRewriteForm): HeaderRewrite {
  const mode = hostModeOf(form);
  // 选了「自定义」却没填：按自动处理，别把空串（＝保留原值）悄悄存进去
  const custom = form.host?.trim() ?? '';
  return {
    host: mode === 'auto' || (mode === 'custom' && !custom) ? null : mode === 'keep' ? '' : custom,
    forward_headers: form.forward_headers,
    rewrite_location: form.rewrite_location,
    extra: form.extra
      .map((pair) => ({ name: pair.name.trim(), value: pair.value.trim() }))
      .filter((pair) => pair.name.length > 0),
  };
}

/** 是不是全默认（用来在界面上判断「要不要显示还原按钮」）。 */
export function isDefaultRewrite(form: HeaderRewriteForm): boolean {
  const mode = hostModeOf(form);
  return (
    (mode === 'auto' || (mode === 'custom' && !form.host?.trim())) &&
    form.forward_headers &&
    form.rewrite_location &&
    form.extra.every((pair) => pair.name.trim() === '')
  );
}

export interface PreviewContext {
  /** 浏览器访问的主机名（填了域名就是域名，否则是回环地址） */
  entryHost: string;
  entryPort: number;
  /** 入口协议（浏览器那一侧，也是回写 Location 用的协议） */
  scheme: string;
  remoteScheme: string;
  remoteHost: string;
  remotePort: number;
  /**
   * 改写开关是否生效。缺省 `true`。
   *
   * 关掉时引擎完全不动头，预览也必须照实说「原样透传」——否则用户关了开关就不知道
   * 远端到底会收到什么，只能靠猜。
   */
  enabled?: boolean;
}

export interface PreviewLine {
  label: string;
  /** 浏览器发出去的；空串＝浏览器不会发这个头，不画箭头 */
  from: string;
  /** 远端会收到的 */
  to: string;
  /** 该项被关掉了（界面上标灰） */
  off: boolean;
  /**
   * 用一句话代替「from → to」。
   *
   * 「不发」这种状态用箭头表达会误导（看着像远端收到空值），直接写清楚更省事。
   */
  note?: string;
}

/** 默认端口不写出来，与引擎的 authority 规则一致。 */
function authority(host: string, port: number, scheme: string): string {
  const fallback = scheme === 'https' ? 443 : 80;
  return port === fallback ? host : `${host}:${port}`;
}

/**
 * 这次改写会把哪几个头变成什么。
 *
 * 顺序固定：Host、X-Forwarded-Proto、X-Forwarded-Host、Location，最后是自定义头。
 * `off` 表示该条已被用户关掉（不发送/不改写）。
 */
export function previewRewrite(form: HeaderRewriteForm, ctx: PreviewContext): PreviewLine[] {
  const entryAuth = authority(ctx.entryHost, ctx.entryPort, ctx.scheme);
  const remoteAuth = authority(ctx.remoteHost, ctx.remotePort, ctx.remoteScheme);

  // 开关关掉：什么头都不动，远端收到的就是浏览器发来的
  if (ctx.enabled === false) {
    return [
      { label: 'Host', from: '', to: '', off: false, note: `原样透传 · ${entryAuth}（浏览器必带）` },
      { label: 'X-Forwarded-Proto', from: '', to: '', off: true, note: '不加 · 浏览器本来也不发' },
      { label: 'X-Forwarded-Host', from: '', to: '', off: true, note: '不加 · 浏览器本来也不发' },
      { label: 'Location', from: '', to: '', off: true, note: '不改写 · 远端给什么就回什么' },
    ];
  }

  const mode = hostModeOf(form);
  const customHost = form.host?.trim() ?? '';
  // 「自定义」留空按自动算，与 toDtoRewrite 的口径一致，预览才不会骗人
  const hostTarget =
    mode === 'keep' ? entryAuth : mode === 'custom' && customHost ? customHost : remoteAuth;

  const lines: PreviewLine[] = [{ label: 'Host', from: entryAuth, to: hostTarget, off: false }];
  if (form.forward_headers) {
    // 浏览器不发这两个头，是我们补的：所以左边留空，只显示远端会收到的值
    lines.push({ label: 'X-Forwarded-Proto', from: '', to: ctx.scheme, off: false });
    lines.push({ label: 'X-Forwarded-Host', from: '', to: entryAuth, off: false });
  } else {
    lines.push({
      label: 'X-Forwarded-Proto',
      from: '',
      to: '',
      off: true,
      note: '不加 · 浏览器本来也不发',
    });
    lines.push({
      label: 'X-Forwarded-Host',
      from: '',
      to: '',
      off: true,
      note: '不加 · 浏览器本来也不发',
    });
  }
  lines.push(
    form.rewrite_location
      ? { label: 'Location', from: `${remoteAuth}/…`, to: `${entryAuth}/…`, off: false }
      : { label: 'Location', from: '', to: '', off: true, note: '不改写 · 远端给什么就回什么' },
  );

  for (const pair of form.extra) {
    const name = pair.name.trim();
    if (name) {
      lines.push({ label: name, from: '', to: pair.value.trim(), off: false });
    }
  }
  return lines;
}

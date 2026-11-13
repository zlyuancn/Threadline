import { computed, ref, watch } from 'vue';
import { faviconUrl, isWeb, type ForwardLike, type LocalScheme } from './forwardView';
import type { TunnelView } from '../api/types';

/**
 * 协议图标：没有自定义图标、也拿不到站点 favicon 时的兜底。
 *
 * 卡片与磁贴共用同一份——各写一套的话，同一条 RDP 转发会在首页画成「两条线」、
 * 在应用页画成显示器。新增协议只改这里。
 */
export const PROTOCOL_GLYPHS: Record<LocalScheme, string> = {
  http: '<svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><circle cx="10" cy="10" r="7"/><path d="M3 10h14M10 3c2 2 3 4.3 3 7s-1 5-3 7c-2-2-3-4.3-3-7s1-5 3-7z"/></svg>',
  https:
    '<svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><circle cx="10" cy="10" r="7"/><path d="M3 10h14M10 3c2 2 3 4.3 3 7s-1 5-3 7c-2-2-3-4.3-3-7s1-5 3-7z"/></svg>',
  ssh: '<svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><rect x="2.5" y="3.5" width="15" height="13" rx="2.5"/><path d="M6 8l2.6 2.4L6 12.8M10.8 13h3.4"/></svg>',
  rdp: '<svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><rect x="2.5" y="3.5" width="15" height="10.5" rx="2"/><path d="M7 17h6M10 14v3"/></svg>',
  tcp: '<svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M3.5 7h13M3.5 13h13"/><circle cx="7.5" cy="7" r="1.8"/><circle cx="12.5" cy="13" r="1.8"/></svg>',
};

/** 协议图标；未知协议已被 `schemeOf` 归一成 tcp。 */
export function protocolGlyph(scheme: LocalScheme): string {
  return PROTOCOL_GLYPHS[scheme];
}

/**
 * 转发图标的取图规则：自定义图标 → 站点 favicon → 空（调用方回退协议图标）。
 *
 * 卡片与磁贴共用这一份，否则两个页面的图标优先级迟早会各走各的。
 */
export function resolveIconSrc(
  icon: string | undefined,
  web: boolean,
  running: boolean,
  favicon: string,
): string {
  const custom = (icon ?? '').trim();
  if (custom) {
    return custom;
  }
  // 停止时不去请求 favicon：必然失败，白白在控制台刷错误。
  return running && web ? favicon : '';
}

/**
 * 图标的响应式封装。
 *
 * `running` 由调用方给（卡片看 state，磁贴看是否真能打开），
 * 只影响「要不要去取站点 favicon」；自定义图标与状态无关。
 * 图片加载失败后置 `broken`，回退到协议图标；换了图标或状态重新给一次机会。
 */
export function useForwardIcon(view: () => TunnelView, running: () => boolean) {
  const broken = ref(false);

  const src = computed(() => {
    if (broken.value) {
      return '';
    }
    const current = view();
    const asForward = current as unknown as ForwardLike;
    return resolveIconSrc(
      current.definition.icon,
      isWeb(asForward),
      running(),
      faviconUrl(asForward),
    );
  });

  watch(
    [
      () => view().definition.id,
      () => view().definition.icon,
      () => view().definition.scheme,
      running,
    ],
    () => {
      broken.value = false;
    },
  );

  return { src, broken };
}

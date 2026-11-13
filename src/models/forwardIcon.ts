import { computed, ref, watch } from 'vue';
import { faviconUrl, isWeb, type ForwardLike } from './forwardView';
import type { TunnelView } from '../api/types';

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

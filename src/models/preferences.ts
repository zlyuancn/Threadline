/**
 * 可分享的本机偏好（主题、呼出快捷键）。
 *
 * 为什么放在前端：主题在 WebView 的 `localStorage`，快捷键在 Agent 的 `agent.json`，
 * Core 两头都拿不到，也不该为了导出包去读这些。
 */

import type { ProfilePreferences } from './profile';
import { useHotkeyStore } from './hotkey';
import { THEMES, applyTheme, readStoredTheme } from './theme';

export interface PreferencesDiffLine {
  key: 'theme' | 'hotkey';
  /** 直接给界面显示的一行，如「主题 墨玉 → 深蓝」 */
  label: string;
  from: string;
  to: string;
}

function themeName(id: string): string {
  return THEMES.find((theme) => theme.id === id)?.name ?? id;
}

/** 当前本机偏好。`hotkey` 取 store 里已生效的那份。 */
export function readPreferences(): ProfilePreferences {
  const hotkey = useHotkeyStore();
  return { theme: readStoredTheme(), hotkey: hotkey.saved };
}

/**
 * 与包里偏好比较。
 *
 * 只列**会变**的项：导入面板要能让用户一眼看出"这个包会改我哪两项设置"，
 * 没变化的项列出来只会变成噪音。
 */
export function diffPreferences(
  current: ProfilePreferences,
  incoming?: ProfilePreferences,
): PreferencesDiffLine[] {
  if (!incoming) {
    return [];
  }
  const lines: PreferencesDiffLine[] = [];
  if (incoming.theme && incoming.theme !== current.theme) {
    lines.push({
      key: 'theme',
      label: `主题 ${themeName(current.theme)} → ${themeName(incoming.theme)}`,
      from: current.theme,
      to: incoming.theme,
    });
  }
  if (incoming.hotkey && incoming.hotkey !== current.hotkey) {
    lines.push({
      key: 'hotkey',
      label: `呼出快捷键 ${current.hotkey} → ${incoming.hotkey}`,
      from: current.hotkey,
      to: incoming.hotkey,
    });
  }
  return lines;
}

/** 写回偏好的依赖，测试时注入假实现。 */
export interface PreferenceApplier {
  applyTheme(id: string): void;
  currentHotkey(): string;
  /** 成功返回 null，失败返回原因。 */
  rebindHotkey(spec: string): Promise<string | null>;
}

export function defaultApplier(): PreferenceApplier {
  return {
    applyTheme: (id) => applyTheme(id),
    currentHotkey: () => useHotkeyStore().saved,
    rebindHotkey: (spec) => useHotkeyStore().rebind(spec),
  };
}

/**
 * 应用包里的偏好，返回失败原因（空数组 = 全部成功）。
 *
 * 主题只接受本机已知的 id；快捷键失败（被占用）时保留原值——
 * 一个导入动作不该把用户的呼出键改成一个按不出来的键。
 */
export async function applyPreferences(
  incoming: ProfilePreferences,
  applier: PreferenceApplier = defaultApplier(),
): Promise<string[]> {
  const failures: string[] = [];

  if (incoming.theme) {
    if (THEMES.some((theme) => theme.id === incoming.theme)) {
      applier.applyTheme(incoming.theme);
    } else {
      failures.push(`主题「${incoming.theme}」本机没有，已跳过`);
    }
  }

  if (incoming.hotkey && incoming.hotkey !== applier.currentHotkey()) {
    const error = await applier.rebindHotkey(incoming.hotkey);
    if (error) {
      failures.push(`呼出快捷键没改：${error}`);
    }
  }

  return failures;
}

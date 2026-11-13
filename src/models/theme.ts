import { defineStore } from 'pinia';
import { ref } from 'vue';
import { setWindowCaption } from '../api/native';

/**
 * 主题：只是换一套色值，不动布局。
 *
 * 色值的唯一来源是 `styles.css` 里的 `[data-theme=...]` 选择器；
 * 这里只保存"选了哪个"并把它写到 <html data-theme> 上。
 * 之所以不在 JS 里也存一份颜色：两处都写迟早会不一致。
 */
export interface ThemeOption {
  id: string;
  name: string;
  hint: string;
  /** 暗色 / 亮色：设置页按这个分组。 */
  mode: 'dark' | 'light';
  /** 预览小卡用的三个代表色：底色、主强调、次强调。 */
  bg: string;
  /** 该主题的正文色，用于原生标题栏文字（DWM 不认 CSS 变量）。 */
  text: string;
  accent: string;
  accent2: string;
}

export const THEMES: ThemeOption[] = [
  {
    id: 'jade',
    name: '墨玉',
    hint: '深墨绿 · 薄荷',
    mode: 'dark',
    bg: '#071316',
    text: '#eaf5f3',
    accent: '#34d399',
    accent2: '#22d3ee',
  },
  {
    id: 'ocean',
    name: '深海',
    hint: '冷蓝 · 靛紫',
    mode: 'dark',
    bg: '#061220',
    text: '#e6f1fb',
    accent: '#38bdf8',
    accent2: '#818cf8',
  },
  {
    id: 'violet',
    name: '紫夜',
    hint: '深紫 · 品红',
    mode: 'dark',
    bg: '#0d0a17',
    text: '#f0eafb',
    accent: '#a78bfa',
    accent2: '#f472b6',
  },
  {
    id: 'ember',
    name: '炭火',
    hint: '暖褐 · 琥珀',
    mode: 'dark',
    bg: '#140f0c',
    text: '#f7ece4',
    accent: '#fb923c',
    accent2: '#f59e0b',
  },
  {
    id: 'celadon',
    name: '青瓷',
    hint: '墨青 · 青瓷绿',
    mode: 'dark',
    bg: '#0a1416',
    text: '#e6f4f2',
    accent: '#2dd4bf',
    accent2: '#7dd3fc',
  },
  {
    id: 'parchment',
    name: '宣纸',
    hint: '暖白 · 朱红',
    mode: 'light',
    bg: '#f6f2e9',
    text: '#2f2920',
    accent: '#c2410c',
    accent2: '#0f766e',
  },
  {
    id: 'sky',
    name: '天青',
    hint: '冷白 · 靛蓝',
    mode: 'light',
    bg: '#f3f6f9',
    text: '#1f2a37',
    accent: '#0284c7',
    accent2: '#4f46e5',
  },
  {
    id: 'graphite',
    name: '石墨',
    hint: '中性灰 · 天青',
    mode: 'dark',
    bg: '#0e1012',
    text: '#e8edf2',
    accent: '#7dd3fc',
    accent2: '#cbd5e1',
  },
  {
    id: 'wine',
    name: '酒红',
    hint: '深酒 · 玫红',
    mode: 'dark',
    bg: '#140a0e',
    text: '#fbe9ee',
    accent: '#fb7185',
    accent2: '#f0abfc',
  },
  {
    id: 'forest',
    name: '森野',
    hint: '深林 · 黄绿',
    mode: 'dark',
    bg: '#0b1410',
    text: '#e8f5ec',
    accent: '#4ade80',
    accent2: '#a3e635',
  },
  {
    id: 'mintlight',
    name: '薄荷',
    hint: '白 · 青绿',
    mode: 'light',
    bg: '#f2f8f4',
    text: '#1d2b24',
    accent: '#059669',
    accent2: '#0d9488',
  },
  {
    id: 'blossom',
    name: '樱粉',
    hint: '粉白 · 玫红',
    mode: 'light',
    bg: '#fdf4f6',
    text: '#3b2430',
    accent: '#be123c',
    accent2: '#a21caf',
  },
  {
    id: 'slate',
    name: '雾灰',
    hint: '冷灰 · 石墨',
    mode: 'light',
    bg: '#f4f5f7',
    text: '#22272e',
    accent: '#475569',
    accent2: '#0f766e',
  },
  {
    id: 'cream',
    name: '奶油',
    hint: '奶油 · 琥珀',
    mode: 'light',
    bg: '#fdf8ec',
    text: '#3a2f1b',
    accent: '#b45309',
    accent2: '#0f766e',
  },
];

const STORAGE_KEY = 'threadline.theme';
const DEFAULT_THEME = 'jade';

function isKnown(id: string | null): id is string {
  return !!id && THEMES.some((theme) => theme.id === id);
}

/** 读上次选择；localStorage 不可用（隐私模式）时退回默认，不抛错。 */
export function readStoredTheme(): string {
  try {
    const saved = localStorage.getItem(STORAGE_KEY);
    return isKnown(saved) ? saved : DEFAULT_THEME;
  } catch {
    return DEFAULT_THEME;
  }
}

/** 应用到 <html> 上；样式表里所有 `[data-theme=...]` 立刻生效。 */
export function applyTheme(id: string): void {
  document.documentElement.dataset.theme = id;
  const option = THEMES.find((theme) => theme.id === id);
  const mode = option?.mode ?? 'dark';
  // Element Plus 的暗色变量挂在 `html.dark` 上；亮色主题必须摘掉它，
  // 否则输入框、下拉、弹窗仍是深色。
  document.documentElement.classList.toggle('dark', mode === 'dark');
  void syncWindowTheme(mode);
  if (option) {
    // 原生标题栏不认 CSS 变量：把该主题的背景/文字色直接交给 DWM。
    void setWindowCaption(option.bg, option.text);
  }
  try {
    localStorage.setItem(STORAGE_KEY, id);
  } catch {
    // 存不了就只在本次会话生效，不必打扰用户。
  }
}

/**
 * 同步 Windows 标题栏的深浅。
 *
 * WebView 里的 CSS 管不到原生标题栏；不调这个，切到亮色主题后顶栏仍是黑的。
 */
async function syncWindowTheme(mode: 'dark' | 'light'): Promise<void> {
  try {
    const { getCurrentWindow } = await import('@tauri-apps/api/window');
    await getCurrentWindow().setTheme(mode);
  } catch {
    // 浏览器调试或运行时不支持，忽略。
  }
}

export const useThemeStore = defineStore('theme', () => {
  const current = ref<string>(readStoredTheme());

  function set(id: string): void {
    if (!isKnown(id)) {
      return;
    }
    current.value = id;
    applyTheme(id);
  }

  return { current, set };
});

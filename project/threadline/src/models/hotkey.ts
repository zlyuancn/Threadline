import { defineStore } from 'pinia';
import { computed, ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { isTauri } from '../api/native';

/**
 * 全局呼出快捷键。
 *
 * 真正的注册在 Rust 侧（user32 RegisterHotKey）：这里只负责读、改、以及把
 * "改不了"的原因原样带给界面（被别的程序占用 / 缺修饰键 / 格式不认识）。
 */
export const useHotkeyStore = defineStore('hotkey', () => {
  const spec = ref('Ctrl+Alt+T');
  const saved = ref('Ctrl+Alt+T');

  /** 侧栏用小键帽分开展示，不直接渲染一整串。 */
  const keys = computed(() => spec.value.split('+').map((part) => part.trim()).filter(Boolean));
  const dirty = computed(() => spec.value !== saved.value);

  async function load() {
    if (!isTauri()) {
      return;
    }
    try {
      spec.value = await invoke<string>('get_hotkey');
      saved.value = spec.value;
    } catch {
      // 取不到就用默认值：界面不该因为一个偏好读失败而空白
    }
  }

  /** 成功返回 null，失败返回给用户看的原因。 */
  async function rebind(next: string): Promise<string | null> {
    if (!isTauri()) {
      return '只有桌面端能改快捷键';
    }
    try {
      const applied = await invoke<string>('set_hotkey', { spec: next });
      spec.value = applied;
      saved.value = applied;
      return null;
    } catch (e) {
      return e instanceof Error ? e.message : String(e);
    }
  }

  return { spec, saved, keys, dirty, load, rebind };
});

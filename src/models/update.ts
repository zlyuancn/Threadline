import { defineStore } from 'pinia';
import { computed, ref } from 'vue';
import {
  checkUpdate,
  downloadUpdate,
  installUpdate,
  isTauri,
  onUpdateProgress,
} from '../api/native';
import type { UpdateInfo } from '../api/native';

/** 用户点过「跳过此版本」的版本号，只存本机。 */
const SKIP_KEY = 'threadline.update.skip';

/**
 * 版本更新。
 *
 * 判断「要不要提示」的规则集中在这里：有新版、且不是用户跳过的那个版本。
 * 网络失败一律静默——这个应用大多跑在能上内网但不一定通 GitHub 的机器上，
 * 每次启动弹一个「检查更新失败」比不检查更烦人。
 */
export const useUpdateStore = defineStore('update', () => {
  const info = ref<UpdateInfo | null>(null);
  const checking = ref(false);
  const downloading = ref(false);
  const downloaded = ref(0);
  const total = ref(0);
  const installerPath = ref('');
  const error = ref('');
  /** 本次会话是否已自动查过，避免每次进设置页都打接口 */
  const autoChecked = ref(false);

  const available = computed(() => info.value?.available === true);
  const percent = computed(() =>
    total.value > 0 ? Math.min(100, Math.round((downloaded.value / total.value) * 100)) : 0,
  );
  const skipped = computed(() => localStorage.getItem(SKIP_KEY) ?? '');
  /** 有新版、且不在跳过名单里——只有它为真才该打扰用户。 */
  const shouldNotify = computed(
    () => available.value && info.value?.latest !== skipped.value,
  );

  function skip() {
    if (info.value) {
      localStorage.setItem(SKIP_KEY, info.value.latest);
    }
  }

  async function check(): Promise<boolean> {
    if (checking.value || !isTauri()) {
      return false;
    }
    checking.value = true;
    error.value = '';
    try {
      info.value = await checkUpdate();
      return info.value.available;
    } catch (e) {
      error.value = e instanceof Error ? e.message : String(e);
      return false;
    } finally {
      checking.value = false;
    }
  }

  async function download(): Promise<boolean> {
    const current = info.value;
    if (!current?.url || downloading.value) {
      return false;
    }
    downloading.value = true;
    downloaded.value = 0;
    total.value = current.size;
    error.value = '';
    const off = await onUpdateProgress((got, all) => {
      downloaded.value = got;
      if (all > 0) {
        total.value = all;
      }
    });
    try {
      installerPath.value = await downloadUpdate(current.url, current.latest);
      return true;
    } catch (e) {
      error.value = e instanceof Error ? e.message : String(e);
      return false;
    } finally {
      off();
      downloading.value = false;
    }
  }

  async function install(): Promise<boolean> {
    if (!installerPath.value) {
      return false;
    }
    try {
      await installUpdate(installerPath.value);
      return true;
    } catch (e) {
      error.value = e instanceof Error ? e.message : String(e);
      return false;
    }
  }

  return {
    info,
    checking,
    downloading,
    downloaded,
    total,
    installerPath,
    error,
    autoChecked,
    available,
    percent,
    skipped,
    shouldNotify,
    skip,
    check,
    download,
    install,
  };
});

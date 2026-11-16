// 桌面壳层的界面偏好：只影响"窗口怎么表现"与"应用列表怎么筛"，与隧道配置无关。
//
// 存 localStorage（与主题同一层）。其中"打开后隐藏""显示全部"两项会随配置包导出导入，
// 因为它们决定了用起来的手感，换台机器不该重新摸一遍。

import { defineStore } from 'pinia';
import { ref } from 'vue';

const SUMMON_KEY = 'threadline.summon-to-apps';
const HIDE_ON_LAUNCH_KEY = 'threadline.hide-on-launch';
const SHOW_ALL_APPS_KEY = 'threadline.show-all-apps';

function readBool(key: string, fallback: boolean): boolean {
  try {
    const saved = localStorage.getItem(key);
    return saved === null ? fallback : saved === '1';
  } catch {
    return fallback;
  }
}

function writeBool(key: string, value: boolean): void {
  try {
    localStorage.setItem(key, value ? '1' : '0');
  } catch {
    // 存不了就只在本次会话生效，不必为它报错。
  }
}

/** 不经过 store 的读取：给非组件代码（打开链接的那一刻）用。 */
export function readHideOnLaunch(): boolean {
  return readBool(HIDE_ON_LAUNCH_KEY, false);
}

export function readShowAllApps(): boolean {
  return readBool(SHOW_ALL_APPS_KEY, true);
}

export const useDesktopPrefsStore = defineStore('desktopPrefs', () => {
  /**
   * 每次唤起窗口（热键 / 托盘 / 再次双击启动）是否回到「应用」页。
   *
   * 默认开：「应用」就是首页，唤起后该看到能直接点的入口，
   * 而不是上次停留的日志或设置页。关掉后保留当前页，适合边看日志边盯终端的人。
   */
  const summonToApps = ref(readBool(SUMMON_KEY, true));

  /**
   * 点开一个应用（在浏览器里打开转发地址）之后，是否把面板收起来。
   *
   * 默认关：收起窗口是"少了一步"的事，没打招呼就消失会让人以为程序崩了；
   * 但用惯了的人（打开就走）希望它别占着屏幕——所以做成开关而不是默认行为。
   */
  const hideOnLaunch = ref(readHideOnLaunch());

  /**
   * 应用列表是否显示全部转发（否则只看随隧道自启的那几条）。
   *
   * 默认开：新用户第一次进来，"全都看不见"比"看得有点多"更让人困惑。
   */
  const showAllApps = ref(readShowAllApps());

  function setSummonToApps(value: boolean): void {
    summonToApps.value = value;
    writeBool(SUMMON_KEY, value);
  }

  function setHideOnLaunch(value: boolean): void {
    hideOnLaunch.value = value;
    writeBool(HIDE_ON_LAUNCH_KEY, value);
  }

  function setShowAllApps(value: boolean): void {
    showAllApps.value = value;
    writeBool(SHOW_ALL_APPS_KEY, value);
  }

  return {
    summonToApps,
    hideOnLaunch,
    showAllApps,
    setSummonToApps,
    setHideOnLaunch,
    setShowAllApps,
  };
});

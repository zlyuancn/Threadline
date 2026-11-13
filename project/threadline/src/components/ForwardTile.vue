<script setup lang="ts">
// 「所有转发」页的应用入口磁贴。
//
// 与首页的 ForwardCard 分工不同：那边是"管理一条转发"（启停、映射、编辑、删除全摆在外面），
// 这边是导航页式的应用入口——图标 + 名称 + 本地地址，能打开的直接点；管理动作收进「…」菜单。
//
// 图标不在代码里写死任何站点：运行时从用户自己配置的入口地址取 favicon，失败回退协议图标。
import { computed } from 'vue';
import { ElMessage } from 'element-plus';
import { useTunnelsStore } from '../models/tunnels';
import { forwardBlockedReason } from '../models/forwardStatus';
import {
  canOpen,
  endpointLabel,
  isActive,
  pillOf,
  schemeOf,
  urlOf,
  type ForwardLike,
  type LocalScheme,
} from '../models/forwardView';
import { useForwardIcon } from '../models/forwardIcon';
import { openExternal } from '../api/native';
import type { TunnelView } from '../api/types';

const props = defineProps<{
  view: TunnelView;
  /** 这条转发所属隧道；用来算 blockedReason 与提示。 */
  server?: { id: string; name: string; enabled: boolean } | null;
  busyKey: string | null;
}>();

const emit = defineEmits<{
  (e: 'toggle', enabled: boolean): void;
  (e: 'edit'): void;
  (e: 'remove'): void;
}>();

const forwards = useTunnelsStore();

const def = computed(() => props.view.definition);
const asForward = computed(() => props.view as unknown as ForwardLike);
const state = computed(() => props.view.status.state);
const scheme = computed(() => schemeOf(asForward.value));
const address = computed(() => endpointLabel(asForward.value));
const openable = computed(() => canOpen(asForward.value));
/** 只有运行中的 http/https 才真的能打开：停止时点了只会得到一个错误页。 */
const clickable = computed(() => openable.value && state.value === 'running');
const active = computed(() => isActive(state.value));
const stateText = computed(() => forwards.stateLabel(state.value));
const blocked = computed(() => forwardBlockedReason(props.view.status, props.server ?? undefined));

/** 图标优先级：用户自定义 → 站点 favicon（仅运行中）→ 协议图标；规则与卡片共用。 */
const { src: iconSrc, broken: iconBroken } = useForwardIcon(() => props.view, () => clickable.value);

const ICONS: Record<LocalScheme, string> = {
  http: '<svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><circle cx="10" cy="10" r="7"/><path d="M3 10h14M10 3c2 2 3 4.3 3 7s-1 5-3 7c-2-2-3-4.3-3-7s1-5 3-7z"/></svg>',
  https: '<svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><circle cx="10" cy="10" r="7"/><path d="M3 10h14M10 3c2 2 3 4.3 3 7s-1 5-3 7c-2-2-3-4.3-3-7s1-5 3-7z"/></svg>',
  ssh: '<svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><rect x="2.5" y="3.5" width="15" height="13" rx="2.5"/><path d="M6 8l2.6 2.4L6 12.8M10.8 13h3.4"/></svg>',
  rdp: '<svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><rect x="2.5" y="3.5" width="15" height="10.5" rx="2"/><path d="M7 17h6M10 14v3"/></svg>',
  tcp: '<svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M3.5 7h13M3.5 13h13"/><circle cx="7.5" cy="7" r="1.8"/><circle cx="12.5" cy="13" r="1.8"/></svg>',
};

const icon = computed(() => ICONS[scheme.value]);

const addressTitle = computed(() => {
  if (!openable.value) {
    if (scheme.value === 'ssh') {
      return `SSH 转发：用 SSH 客户端连接 ${address.value}`;
    }
    if (scheme.value === 'rdp') {
      return `RDP 转发：用远程桌面客户端连接 ${address.value}`;
    }
    return `TCP 转发：用对应客户端连接 ${address.value}`;
  }
  return clickable.value ? `在浏览器打开 ${urlOf(asForward.value)}` : `${stateText.value}，启动后才能打开`;
});

function openLocal() {
  if (!clickable.value) {
    return;
  }
  openExternal(urlOf(asForward.value)).catch((e: unknown) => {
    ElMessage.error(e instanceof Error ? e.message : String(e));
  });
}
</script>

<template>
  <article
    class="tile"
    :class="{ 'is-running': state === 'running', 'is-openable': clickable }"
    :tabindex="clickable ? 0 : undefined"
    :role="clickable ? 'link' : undefined"
    :aria-label="clickable ? `打开 ${def.name}` : def.name"
    @click="openLocal"
    @keydown.enter="openLocal"
    @keydown.space.prevent="openLocal"
  >
    <div class="tile-avatar">
      <div class="tile-icon" :class="[`is-${scheme}`, { 'has-image': iconSrc }]">
        <img v-if="iconSrc" :src="iconSrc" alt="" @error="iconBroken = true" />
        <span v-else class="tile-glyph" v-html="icon" />
      </div>
      <span class="dot tile-dot" :class="pillOf(state)" :title="blocked || stateText" />
    </div>

    <div class="tile-name" :title="def.name">{{ def.name }}</div>
    <div v-if="def.note" class="tile-note" :title="def.note">{{ def.note }}</div>
    <div class="tile-addr mono" :title="addressTitle">{{ address }}</div>
    <div v-if="blocked" class="tile-warn" :title="blocked">{{ blocked }}</div>

    <div class="tile-more-wrap" @click.stop>
      <el-dropdown trigger="click" placement="bottom-end">
        <button class="tile-more" title="更多操作" aria-label="更多操作">
          <svg viewBox="0 0 16 16" fill="currentColor"><circle cx="3.5" cy="8" r="1.2" /><circle cx="8" cy="8" r="1.2" /><circle cx="12.5" cy="8" r="1.2" /></svg>
        </button>
        <template #dropdown>
          <el-dropdown-menu>
            <el-dropdown-item :disabled="busyKey !== null || active" @click="emit('toggle', true)">
              启动
            </el-dropdown-item>
            <el-dropdown-item :disabled="busyKey !== null || !active" @click="emit('toggle', false)">
              停止
            </el-dropdown-item>
            <el-dropdown-item divided :disabled="busyKey !== null" @click="emit('edit')">
              编辑
            </el-dropdown-item>
            <el-dropdown-item :disabled="busyKey !== null" @click="emit('remove')">
              删除
            </el-dropdown-item>
          </el-dropdown-menu>
        </template>
      </el-dropdown>
    </div>
  </article>
</template>

<style scoped>
.tile {
  position: relative;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 2px;
  min-width: 0;
  padding: 12px 6px 9px;
  border: 1px solid transparent;
  border-radius: var(--r-md);
  transition:
    background 140ms ease,
    border-color 140ms ease;
}

.tile.is-openable {
  cursor: pointer;
}

.tile.is-openable:hover {
  border-color: var(--line);
  background: var(--tint);
}

/* 键盘导航才出现的焦点环：不填充底色，避免窗口一打开就像整块被选中 */
.tile.is-openable:focus-visible {
  outline: 1px solid var(--mint);
  outline-offset: 1px;
}

.tile-avatar {
  position: relative;
  margin-bottom: 4px;
}

.tile-icon {
  display: grid;
  place-items: center;
  width: 64px;
  height: 64px;
  overflow: hidden;
  border: 1px solid var(--line);
  border-radius: 16px;
  background: color-mix(in srgb, var(--bg) 38%, transparent);
  color: var(--mint);
  transition:
    filter 160ms ease,
    opacity 160ms ease;
}

/* 站点自己的图标自带底色，再套一层描边只会显得脏 */
.tile-icon.has-image {
  border-color: transparent;
  background: transparent;
}

.tile-icon img {
  width: 100%;
  height: 100%;
  object-fit: contain;
}

.tile-glyph {
  display: grid;
  place-items: center;
}

.tile-glyph :deep(svg) {
  width: 30px;
  height: 30px;
}

.tile-icon.is-ssh {
  color: var(--cyan);
}

.tile-icon.is-rdp {
  color: var(--warn);
}

.tile-icon.is-tcp {
  color: var(--text-2);
}

/* 没在跑的服务图标压暗：一眼分清"可用"与"只是配过" */
.tile:not(.is-running) .tile-icon {
  filter: grayscale(0.55);
  opacity: 0.55;
}

.tile-name {
  max-width: 100%;
  font-size: 12.5px;
  font-weight: 650;
  line-height: 1.35;
  text-align: center;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.tile-note {
  max-width: 100%;
  font-size: 10px;
  line-height: 1.3;
  color: var(--text-3);
  text-align: center;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.tile-addr {
  max-width: 100%;
  font-size: 10px;
  line-height: 1.4;
  color: var(--text-3);
  text-align: center;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.tile.is-openable:hover .tile-addr {
  color: var(--mint);
}

.tile-warn {
  max-width: 100%;
  margin-top: 2px;
  padding: 2px 5px;
  border-radius: 4px;
  background: var(--warn-soft);
  font-size: 9.5px;
  line-height: 1.35;
  color: var(--warn-text);
  text-align: center;
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
}

/* 状态角标：贴在图标右下角，和图标是一个整体；全局 .dot 只管颜色 */
.tile-dot {
  position: absolute;
  right: 0;
  bottom: 0;
  border: 2px solid var(--bg);
}

.tile-more-wrap {
  position: absolute;
  top: 4px;
  right: 2px;
  opacity: 0;
  transition: opacity 120ms ease;
}

.tile:hover .tile-more-wrap,
.tile:focus-within .tile-more-wrap {
  opacity: 1;
}

.tile-more {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 24px;
  height: 24px;
  padding: 0;
  border: none;
  border-radius: 6px;
  background: transparent;
  color: var(--text-3);
  cursor: pointer;
}

.tile-more:hover {
  background: var(--tint-strong);
  color: var(--text);
}

.tile-more svg {
  width: 14px;
  height: 14px;
}
</style>

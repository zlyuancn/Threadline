<script setup lang="ts">
// 端口转发卡片。首页与「所有转发」页共用同一份，避免两处样式与行为漂移。
import { computed } from 'vue';
import { ElMessage } from 'element-plus';
import { useTunnelsStore } from '../models/tunnels';
import { forwardBlockedReason } from '../models/forwardStatus';
import {
  connLabel,
  dotOf,
  entryLabel,
  isWeb,
  pillOf,
  schemeOf,
  urlOf,
  type ForwardLike,
} from '../models/forwardView';
import { formatRate, useTrafficStore } from '../models/traffic';
import { protocolGlyph, useForwardIcon } from '../models/forwardIcon';
import { openExternal } from '../api/native';
import type { TunnelView } from '../api/types';

const props = defineProps<{
  view: TunnelView;
  /** 这条转发所属隧道；「所有转发」页需要它来算 blockedReason。 */
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
const state = computed(() => props.view.status.state);
const web = computed(() => isWeb(props.view as unknown as ForwardLike));
const traffic = useTrafficStore();
const upRate = computed(() => formatRate(traffic.speedOf(props.view.definition.id).up));
const downRate = computed(() => formatRate(traffic.speedOf(props.view.definition.id).down));

/** 协议决定回退图标与配色，和「应用」页磁贴同一套（见 models/forwardIcon）。 */
const scheme = computed(() => schemeOf(props.view as unknown as ForwardLike));
const glyph = computed(() => protocolGlyph(scheme.value));

/** 图标规则与「应用」页磁贴共用：自定义 → 站点 favicon（运行中）→ 协议图标。 */
const { src: iconSrc, broken: iconBroken } = useForwardIcon(
  () => props.view,
  () => state.value === 'running',
);

const blocked = computed(() =>
  forwardBlockedReason(props.view.status, props.server ?? undefined),
);

function openLocal() {
  openExternal(urlOf(props.view as unknown as ForwardLike)).catch((e: unknown) => {
    ElMessage.error(e instanceof Error ? e.message : String(e));
  });
}

async function copyUrl() {
  try {
    await navigator.clipboard.writeText(urlOf(props.view as unknown as ForwardLike));
    ElMessage.success('地址已复制');
  } catch {
    ElMessage.error('复制失败，请手动选中地址');
  }
}
</script>

<template>
  <article
    class="fcard"
    :class="{ 'is-live': state === 'running' }"
    :data-forward-id="def.id"
  >
    <div class="fcard-head">
      <span class="pill" :class="pillOf(state)">
        <span class="dot" :class="dotOf(state)" />
        {{ forwards.stateLabel(state) }}
      </span>
      <!-- 不写文字标签：这个开关就是"这条转发开不开"，旁边再挂"自启"会让人
           以为是另一个开关（比如"要不要随隧道启动"）。语义靠 title 说清。 -->
      <button
        class="sw sw-standalone"
        :class="{ 'is-on': def.auto_start }"
        :disabled="busyKey !== null"
        :title="def.auto_start ? '已启用：隧道连上就自动启动，下次打开也保持' : '已停用：不会启动这条转发'"
        :aria-label="def.auto_start ? '停用这条转发' : '启用这条转发'"
        @click="emit('toggle', !def.auto_start)"
      />
    </div>

    <div class="fcard-id">
      <div class="fcard-icon" :class="[`is-${scheme}`, { 'has-image': iconSrc }]">
        <img v-if="iconSrc" :src="iconSrc" alt="" @error="iconBroken = true" />
        <span v-else class="fcard-glyph" v-html="glyph" />
      </div>
      <div class="fcard-idtext">
        <div class="fcard-name">
          <span :title="def.name">{{ def.name }}</span>
          <span
            v-if="def.domain"
            class="fcard-note"
            :title="`域名入口：${def.domain_host}`"
          >{{ def.domain }}</span>
          <span v-if="def.note" class="fcard-note" :title="def.note">{{ def.note }}</span>
        </div>
        <div class="fcard-sub">
          {{ connLabel(view as unknown as ForwardLike) }}
          <!-- 速率只在有流量时出现：一串 0 B/s 反而更吵 -->
          <span v-if="upRate" class="fcard-speed rate-up">↑ {{ upRate }}</span>
          <span v-if="downRate" class="fcard-speed rate-down">↓ {{ downRate }}</span>
        </div>
      </div>
    </div>

    <!-- 本机在前：读起来是"访问本机这个入口 → 指向远端那个服务" -->
    <div class="map">
      <div class="map-side">
        <div class="map-k">本机</div>
        <div class="map-v mono">{{ entryLabel(view as unknown as ForwardLike) }}</div>
      </div>
      <span class="map-arrow">→</span>
      <div class="map-side">
        <div class="map-k">远端</div>
        <div class="map-v mono">{{ def.remote_host }}:{{ def.remote_port }}</div>
      </div>
    </div>

    <div v-if="web" class="url-row">
      <span class="mono">{{ urlOf(view as unknown as ForwardLike) }}</span>
      <button class="copy-btn" title="复制地址" @click="copyUrl">
        <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><rect x="6" y="6" width="7" height="7" rx="1.5"/><path d="M10 6V4.5A1.5 1.5 0 0 0 8.5 3h-4A1.5 1.5 0 0 0 3 4.5v4A1.5 1.5 0 0 0 4.5 10H6"/></svg>
      </button>
    </div>
    <div v-else class="tcp-hint">
      纯 TCP 转发 · 用客户端连接 <span class="mono">127.0.0.1:{{ def.local_port }}</span>
    </div>

    <div v-if="blocked" class="fcard-reason" :title="blocked">{{ blocked }}</div>

    <div class="fcard-foot">
      <button v-if="web" class="btn is-primary" @click="openLocal">
        <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M9.5 2.5h4v4M13 3l-5.5 5.5"/><path d="M12 9.5v3A1.5 1.5 0 0 1 10.5 14h-7A1.5 1.5 0 0 1 2 12.5v-7A1.5 1.5 0 0 1 3.5 4h3"/></svg>
        打开本地页面
      </button>
      <span v-else class="fcard-foot-spacer" />
      <button class="icon-btn" title="编辑" :disabled="busyKey !== null" @click="emit('edit')">
        <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M11.2 2.3l2.5 2.5-8 8L3 13.5l.7-2.7 7.5-8.5z"/></svg>
      </button>
      <button
        class="icon-btn is-danger"
        title="删除"
        :disabled="busyKey !== null"
        @click="emit('remove')"
      >
        <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M3 4.5h10M6.5 4.5V3h3v1.5M4.5 4.5l.6 8.2h5.8l.6-8.2"/></svg>
      </button>
    </div>
  </article>
</template>

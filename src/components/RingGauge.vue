<script setup lang="ts">
// 环形用量表：一圈弧 + 正中间的百分比。
//
// 为什么不用横条：概览上要并排看 CPU、内存、磁盘，横条得占满一整行宽度，
// 环只占一个方块，百分比还能放在圆心——一眼就能读。
//
// 颜色是**语义**的：< 70 用主题色，70–90 转黄，≥ 90 转红。
// 阈值与 `levelClass()` 保持一致，免得同一个数字在两个地方显示成两种颜色。

import { computed } from 'vue';

const props = withDefaults(
  defineProps<{
    /** 0–100 的用量百分比（超出范围会被夹住），同时决定圆心数字。 */
    value: number;
    /** 直径，像素。 */
    size?: number;
    /** 圆心数字下面的一行小字，例如「已用」。 */
    label?: string;
    /**
     * 分段模式：把一圈按比例切成几段（例如内存的已用 / 缓存 / Buffers）。
     *
     * 不传就是单段（按阈值变色）。传了之后颜色由各段自己决定——
     * 此时"整圈的等级色"没有意义：一半缓存占了 60% 不代表内存告急。
     */
    segments?: { value: number; color: string }[];
  }>(),
  { size: 96, label: '', segments: undefined },
);

const RADIUS = 42;
const CIRCUMFERENCE = 2 * Math.PI * RADIUS;

const clamped = computed(() =>
  Math.max(0, Math.min(100, Number.isFinite(props.value) ? props.value : 0)),
);
const level = computed(() =>
  clamped.value >= 90 ? 'is-bad' : clamped.value >= 70 ? 'is-warn' : 'is-ok',
);
const dashOffset = computed(() => CIRCUMFERENCE * (1 - clamped.value / 100));
/** 小环里塞不下 18px 的字。 */
const valueStyle = computed(() => ({ fontSize: props.size <= 56 ? '11px' : '18px' }));

/** 每段的弧长与起始偏移：一路累加，段与段首尾相接。 */
const arcs = computed(() => {
  let consumed = 0;
  return (props.segments ?? [])
    .filter((segment) => segment.value > 0)
    .map((segment) => {
      const length = (CIRCUMFERENCE * Math.min(100, segment.value)) / 100;
      const arc = {
        length,
        rest: Math.max(0, CIRCUMFERENCE - length),
        offset: -consumed,
        color: segment.color,
      };
      consumed += length;
      return arc;
    });
});
</script>

<template>
  <div class="ring" :class="level" :style="{ width: `${size}px` }">
    <svg viewBox="0 0 100 100" aria-hidden="true">
      <circle class="ring-track" cx="50" cy="50" :r="RADIUS" />
      <!-- 分段模式：每段一个圆，靠 dashoffset 错开起始位置 -->
      <template v-if="arcs.length">
        <circle
          v-for="(arc, index) in arcs"
          :key="index"
          class="ring-seg"
          cx="50"
          cy="50"
          :r="RADIUS"
          :stroke-dasharray="`${arc.length} ${arc.rest}`"
          :stroke-dashoffset="arc.offset"
          :style="{ stroke: arc.color }"
        />
      </template>
      <circle
        v-else
        class="ring-fill"
        cx="50"
        cy="50"
        :r="RADIUS"
        :stroke-dasharray="CIRCUMFERENCE"
        :stroke-dashoffset="dashOffset"
      />
    </svg>
    <div class="ring-text">
      <span class="ring-value" :style="valueStyle">{{ Math.round(clamped) }}%</span>
      <span v-if="label && size > 56" class="ring-label">{{ label }}</span>
    </div>
  </div>
</template>

<style scoped>
.ring {
  position: relative;
  flex-shrink: 0;
}

/* 从 12 点方向开始画弧 */
.ring svg {
  display: block;
  width: 100%;
  height: auto;
  transform: rotate(-90deg);
}

.ring-track {
  fill: none;
  stroke: var(--line-strong);
  stroke-width: 8;
}

.ring-fill {
  fill: none;
  stroke: var(--mint);
  stroke-width: 8;
  stroke-linecap: round;
  transition:
    stroke-dashoffset 260ms ease,
    stroke 200ms ease;
}

/* 分段：圆头会让相邻两段互相压住，这里用平头 */
.ring-seg {
  fill: none;
  stroke-width: 8;
  stroke-linecap: butt;
}

.ring.is-warn .ring-fill {
  stroke: var(--warn);
}

.ring.is-bad .ring-fill {
  stroke: var(--bad);
}

.ring-text {
  position: absolute;
  inset: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 2px;
  pointer-events: none;
}

.ring-value {
  font-weight: 700;
  font-variant-numeric: tabular-nums;
  line-height: 1;
  color: var(--text);
}

.ring-label {
  font-size: 10px;
  color: var(--text-3);
}
</style>

<script setup lang="ts">
// 卡片背景的分时图：把最近一分钟的采样画成一条面积曲线。
//
// 它只回答"刚才有没有抖"，不标刻度也不带坐标轴——精确值在环和文字里，
// 这里再画一遍反而是噪音。所以压低不透明度，当背景纹用。
//
// X 轴按**固定槽位**排：最后一个点永远贴右边，数据不满一分钟时左边留空。
// 若按"有多少点就铺满多宽"，刚打开页面的头几秒会画成一条满格的线，那是假的。

import { computed } from 'vue';

const props = withDefaults(
  defineProps<{
    /** 时间正序的采样值（旧 → 新）。 */
    points: number[];
    /** 分母。不给就取窗口内最大值（适合磁盘 IO 这种没有上限的量）。 */
    max?: number;
    /** 一分钟对应的槽位数：1 秒一采 = 60，5 秒一采 = 12。 */
    slots?: number;
  }>(),
  { max: 0, slots: 60 },
);

const W = 100;
const H = 100;

const ceiling = computed(() => {
  if (props.max > 0) {
    return props.max;
  }
  return Math.max(1, ...props.points);
});

/** 点：x 按槽位从右往左排，y 按 ceiling 归一化。 */
const line = computed(() => {
  const values = props.points;
  if (values.length < 2) {
    return '';
  }
  const step = W / Math.max(1, props.slots - 1);
  const offset = Math.max(0, props.slots - values.length);
  return values
    .map((value, index) => {
      const x = (offset + index) * step;
      const ratio = Math.min(1, Math.max(0, value / ceiling.value));
      return `${x.toFixed(2)},${(H - ratio * H).toFixed(2)}`;
    })
    .join(' ');
});

const area = computed(() => {
  if (!line.value) {
    return '';
  }
  const values = props.points;
  const step = W / Math.max(1, props.slots - 1);
  const offset = Math.max(0, props.slots - values.length);
  const left = (offset * step).toFixed(2);
  return `${left},${H} ${line.value} ${W},${H}`;
});
</script>

<template>
  <svg class="spark" viewBox="0 0 100 100" preserveAspectRatio="none" aria-hidden="true">
    <polygon class="spark-area" :points="area" />
    <polyline class="spark-line" :points="line" vector-effect="non-scaling-stroke" />
  </svg>
</template>

<style scoped>
.spark {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
  color: var(--mint);
  opacity: 0.42;
  pointer-events: none;
}

.spark-line {
  fill: none;
  stroke: currentColor;
  stroke-width: 1.6;
  stroke-linejoin: round;
  stroke-linecap: round;
}

.spark-area {
  fill: currentColor;
  opacity: 0.16;
  stroke: none;
}
</style>

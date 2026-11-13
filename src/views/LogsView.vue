<script setup lang="ts">
import { onMounted, onUnmounted, ref } from 'vue';
import { runtimeApi } from '../api';
import { ApiFailure } from '../api/client';

const lines = ref<string[]>([]);
const truncated = ref(false);
const error = ref('');
const loading = ref(false);
const tail = ref(200);

let timer: number | undefined;

async function load() {
  loading.value = true;
  try {
    const res = await runtimeApi.logs(tail.value);
    lines.value = res.lines;
    truncated.value = res.truncated;
    error.value = '';
  } catch (e) {
    error.value = e instanceof ApiFailure ? e.message : String(e);
  } finally {
    loading.value = false;
  }
}

onMounted(async () => {
  await load();
  timer = window.setInterval(load, 3000);
});

onUnmounted(() => {
  if (timer) window.clearInterval(timer);
});
</script>

<template>
  <div class="panel">
    <h3 class="panel-title">
      核心日志
      <span>
        <el-select v-model="tail" size="small" style="width: 120px" @change="load">
          <el-option :value="100" label="最近 100 行" />
          <el-option :value="200" label="最近 200 行" />
          <el-option :value="1000" label="最近 1000 行" />
        </el-select>
        <el-button size="small" :loading="loading" @click="load">刷新</el-button>
      </span>
    </h3>

    <el-alert v-if="error" type="error" :closable="false" show-icon :title="error" />

    <div class="log-view">{{ lines.length ? lines.join('\n') : '（暂无日志）' }}</div>
    <p class="hint">
      {{ truncated ? '已截断，仅显示尾部' : '已显示全部' }} · 每 3 秒自动刷新
    </p>
  </div>
</template>

<script setup lang="ts">
/**
 * 行内「?」提示：把静态说明从表单里挪走，只在鼠标停上去时冒出来。
 *
 * 只装**解释性**文字。状态类反馈（端口冲突、非法组合、开关为什么置灰）
 * 必须一直可见，仍然用表单里的 `hint` 段落。
 */
defineProps<{
  /** 提示正文；`\n` 会换行。 */
  content: string;
}>();
</script>

<template>
  <el-tooltip
    :content="content"
    placement="top"
    :show-after="150"
    :popper-style="{ maxWidth: '300px', whiteSpace: 'pre-line', lineHeight: '1.55' }"
  >
    <span class="info-tip" role="img" aria-label="说明" tabindex="0">
      <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round">
        <circle cx="8" cy="8" r="6.2" />
        <path d="M6.35 6.15a1.7 1.7 0 1 1 2.25 1.62c-.4.16-.6.5-.6.92v.3" />
        <circle cx="8" cy="11.35" r="0.72" fill="currentColor" stroke="none" />
      </svg>
    </span>
  </el-tooltip>
</template>

<style scoped>
.info-tip {
  display: inline-flex;
  flex-shrink: 0;
  align-items: center;
  justify-content: center;
  width: 16px;
  height: 16px;
  margin-left: 6px;
  color: var(--el-text-color-placeholder);
  vertical-align: middle;
  cursor: help;
}

.info-tip:hover,
.info-tip:focus-visible {
  color: var(--el-color-primary);
  outline: none;
}

.info-tip svg {
  width: 14px;
  height: 14px;
}
</style>

<script setup lang="ts">
/**
 * 统一图标组件。几何数据见 ./paths.ts，本组件只负责渲染与统一的描边规范。
 *
 * 用法：<Icon name="folder" :size="16" />
 * 颜色继承 CSS 的 color（currentColor），因此 hover / active / disabled 态自动生效。
 */
import { computed } from 'vue'
import { ICONS, type IconName } from './paths'

const props = withDefaults(
  defineProps<{
    name: IconName
    /** 数字按 px 处理，也可传 '1.2em' 之类的相对值 */
    size?: number | string
    strokeWidth?: number
  }>(),
  { size: 16, strokeWidth: 1.7 },
)

const paths = computed<readonly string[]>(() => ICONS[props.name])
const dimension = computed(() =>
  typeof props.size === 'number' ? `${props.size}px` : props.size,
)
</script>

<template>
  <svg
    class="icon"
    :style="{ width: dimension, height: dimension }"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    :stroke-width="strokeWidth"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
    focusable="false"
  >
    <path v-for="(d, i) in paths" :key="i" :d="d" />
  </svg>
</template>

<style scoped>
.icon {
  display: inline-block;
  flex: none;
  vertical-align: -0.15em;
}
</style>

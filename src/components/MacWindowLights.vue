// macOS 弹窗左上角的三色窗口按钮：
// 红点执行关闭；黄 / 绿为视觉装饰（应用内弹窗不是独立原生窗口）。
<script setup lang="ts">
import { IS_MACOS } from '../utils/platform'

defineProps<{ disabled?: boolean; closeOnly?: boolean }>()
const emit = defineEmits<{ (e: 'close'): void }>()
</script>

<template>
  <div v-if="IS_MACOS" class="mac-lights" role="group" aria-label="窗口控制">
    <button
      type="button"
      class="light red"
      :disabled="disabled"
      title="关闭"
      aria-label="关闭"
      @click.stop="emit('close')"
    >
      <svg viewBox="0 0 8 8" aria-hidden="true">
        <path d="M2 2l4 4M6 2L2 6" />
      </svg>
    </button>
    <template v-if="!closeOnly">
      <span class="light yellow" aria-hidden="true"></span>
      <span class="light green" aria-hidden="true"></span>
    </template>
  </div>
</template>

<style scoped>
.mac-lights {
  display: flex;
  align-items: center;
  gap: 8px;
  flex: none;
}
.light {
  display: grid;
  place-items: center;
  width: 12px;
  height: 12px;
  padding: 0;
  border: none;
  border-radius: 50%;
  box-shadow: inset 0 0 0 0.5px rgb(0 0 0 / 16%);
}
.light.red {
  background: #ff5f57;
}
.light.yellow {
  background: #febc2e;
}
.light.green {
  background: #28c840;
}
.light svg {
  width: 8px;
  height: 8px;
  fill: none;
  stroke: rgb(0 0 0 / 55%);
  stroke-width: 1.2;
  stroke-linecap: round;
  opacity: 0;
  transition: opacity 0.12s ease;
}
.mac-lights:hover .light.red svg,
.light.red:focus-visible svg {
  opacity: 1;
}
.light.red:disabled {
  opacity: 0.45;
  cursor: default;
}
</style>

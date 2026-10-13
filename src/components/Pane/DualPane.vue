<script setup lang="ts">
// 双窗格容器：左右各一个 Pane + 可拖拽分隔条（WS-D）
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import Pane from './Pane.vue'
import { usePaneStore } from '../../stores/pane'

const paneStore = usePaneStore()
const rootRef = ref<HTMLElement | null>(null)
const dragging = ref(false)

const columns = computed(() => {
  if (paneStore.singlePane) return '1fr'
  const r = paneStore.splitRatio
  return `${r}fr 1px ${1 - r}fr`
})

function onMove(ev: MouseEvent) {
  const el = rootRef.value
  if (!el) return
  const rect = el.getBoundingClientRect()
  if (rect.width <= 0) return
  paneStore.setSplitRatio((ev.clientX - rect.left) / rect.width)
}

function onUp() {
  dragging.value = false
  window.removeEventListener('mousemove', onMove)
  window.removeEventListener('mouseup', onUp)
}

function startDrag(ev: MouseEvent) {
  ev.preventDefault()
  dragging.value = true
  window.addEventListener('mousemove', onMove)
  window.addEventListener('mouseup', onUp)
}

function resetRatio() {
  paneStore.setSplitRatio(0.5)
}

onMounted(() => {
  void paneStore.init()
})

onBeforeUnmount(() => {
  window.removeEventListener('mousemove', onMove)
  window.removeEventListener('mouseup', onUp)
})
</script>

<template>
  <div ref="rootRef" class="dual-pane" :class="{ dragging }" :style="{ gridTemplateColumns: columns }">
    <Pane v-if="!paneStore.singlePane || paneStore.activePaneId === 'left'" pane-id="left" />
    <div v-if="!paneStore.singlePane" class="divider" title="拖拽调整宽度，双击均分" @mousedown="startDrag" @dblclick="resetRatio" />
    <Pane v-if="!paneStore.singlePane || paneStore.activePaneId === 'right'" pane-id="right" />
  </div>
</template>

<style scoped>
.dual-pane {
  flex: 1;
  display: grid;
  min-height: 0;
  overflow: hidden;
  gap: 0;
  padding: 0;
}
.dual-pane.dragging {
  cursor: col-resize;
  user-select: none;
}
/* 两个文件窗格之间：1px 细分割线（贯穿整列） */
.divider {
  position: relative;
  z-index: 1;
  background: var(--border);
  cursor: col-resize;
  transition: background 0.12s ease;
}
/* 1px 太窄不好拖，用不可见的伪元素把热区撑到 7px（不改变视觉宽度） */
.divider::after {
  content: '';
  position: absolute;
  top: 0;
  bottom: 0;
  left: -3px;
  right: -3px;
}
.divider:hover,
.dual-pane.dragging .divider {
  background: var(--border-strong);
}
</style>

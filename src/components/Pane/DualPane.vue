<script setup lang="ts">
// 双窗格容器：左右各一个 Pane + 可拖拽分隔条（WS-D）
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import Pane from './Pane.vue'
import { usePaneStore } from '../../stores/pane'

const paneStore = usePaneStore()
const rootRef = ref<HTMLElement | null>(null)
const dragging = ref(false)

const columns = computed(() => {
  const r = paneStore.splitRatio
  return `${r}fr 5px ${1 - r}fr`
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
    <Pane pane-id="left" />
    <div class="divider" title="拖拽调整宽度，双击均分" @mousedown="startDrag" @dblclick="resetRatio">
      <span class="grip" />
    </div>
    <Pane pane-id="right" />
  </div>
</template>

<style scoped>
.dual-pane {
  flex: 1;
  display: grid;
  min-height: 0;
  overflow: hidden;
}
.dual-pane.dragging {
  cursor: col-resize;
  user-select: none;
}
.divider {
  position: relative;
  background: var(--border);
  cursor: col-resize;
  display: flex;
  align-items: center;
  justify-content: center;
}
.divider:hover,
.dual-pane.dragging .divider {
  background: var(--accent);
}
.grip {
  width: 3px;
  height: 28px;
  border-radius: 2px;
  background: var(--border-strong);
}
.divider:hover .grip,
.dual-pane.dragging .grip {
  background: var(--accent-fg);
}
</style>

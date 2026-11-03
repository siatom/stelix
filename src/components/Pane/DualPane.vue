<script setup lang="ts">
// 双窗格容器：左右各一个 Pane + 可拖拽分隔条（WS-D）
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { getCurrentWindow } from '@tauri-apps/api/window'
import Pane from './Pane.vue'
import { usePaneStore } from '../../stores/pane'

const paneStore = usePaneStore()
const rootRef = ref<HTMLElement | null>(null)
const dragging = ref(false)
const dropTargetPaneId = ref<string | null>(null)
let unlistenDragEvents: UnlistenFn[] = []
let dragEventSequence = 0
let internalDragOperation: 'move' | 'copy' | null = null

function onFileDragStart(event: Event) {
  const detail = (event as CustomEvent<{ operation?: 'move' | 'copy' }>).detail
  internalDragOperation = detail?.operation === 'copy' ? 'copy' : 'move'
}

function onFileDragEnd() {
  internalDragOperation = null
}

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
  window.addEventListener('stelix:file-drag-start', onFileDragStart)
  window.addEventListener('stelix:file-drag-end', onFileDragEnd)
  void paneStore.init()
  const handleDragPosition = async (
    payload: { paths?: string[]; position: { x: number; y: number } },
    isDrop: boolean,
  ) => {
    const dragOperation = isDrop ? internalDragOperation : null
    const sequence = ++dragEventSequence
    try {
      const scale = await getCurrentWindow().scaleFactor()
      if (sequence !== dragEventSequence) return
      const pointX = payload.position.x / scale
      const pointY = payload.position.y / scale
      const paneEl = [...(rootRef.value?.querySelectorAll<HTMLElement>('[data-pane-id]') ?? [])]
        .find((el) => {
          const rect = el.getBoundingClientRect()
          return pointX >= rect.left && pointX <= rect.right && pointY >= rect.top && pointY <= rect.bottom
        })
      const paneId = paneEl?.dataset.paneId
      const owner = paneId ? paneStore.panes.find((pane) => pane.id === paneId) : undefined
      const tab = owner?.tabs.find((item) => item.id === owner.activeTabId)
      if (
        !paneId ||
        !tab ||
        tab.kind === 'remote' ||
        tab.kind === 'search' ||
        tab.kind === 'terminal' ||
        tab.kind === 'trash'
      ) {
        dropTargetPaneId.value = null
        return
      }
      dropTargetPaneId.value = paneId
      if (!isDrop || !payload.paths?.length) return
      const op = {
        op: dragOperation ?? 'copy',
        sources: payload.paths,
        target: tab.path,
      }
      try {
        await paneStore.runFs(paneId, op)
      } finally {
        dropTargetPaneId.value = null
        internalDragOperation = null
      }
    } catch (error) {
      dropTargetPaneId.value = null
      console.error('无法处理外部文件拖放', error)
    }
  }
  void Promise.all([
    listen<{ paths?: string[]; position: { x: number; y: number } }>('tauri://drag-enter', (event) => {
      void handleDragPosition(event.payload, false)
    }),
    listen<{ position: { x: number; y: number } }>('tauri://drag-over', (event) => {
      void handleDragPosition(event.payload, false)
    }),
    listen<{ paths: string[]; position: { x: number; y: number } }>('tauri://drag-drop', (event) => {
      void handleDragPosition(event.payload, true)
    }),
    listen('tauri://drag-leave', () => {
      dragEventSequence += 1
      dropTargetPaneId.value = null
    }),
  ]).then((unlisteners) => { unlistenDragEvents = unlisteners })
})

onBeforeUnmount(() => {
  window.removeEventListener('stelix:file-drag-start', onFileDragStart)
  window.removeEventListener('stelix:file-drag-end', onFileDragEnd)
  unlistenDragEvents.forEach((unlisten) => unlisten())
  window.removeEventListener('mousemove', onMove)
  window.removeEventListener('mouseup', onUp)
})
</script>

<template>
  <div ref="rootRef" class="dual-pane" :class="{ dragging }" :style="{ gridTemplateColumns: columns }">
    <Pane
      v-if="!paneStore.singlePane || paneStore.activePaneId === 'left'"
      pane-id="left"
      :drop-active="dropTargetPaneId === 'left'"
    />
    <div v-if="!paneStore.singlePane" class="divider" title="拖拽调整宽度，双击均分" @mousedown="startDrag" @dblclick="resetRatio" />
    <Pane
      v-if="!paneStore.singlePane || paneStore.activePaneId === 'right'"
      pane-id="right"
      :drop-active="dropTargetPaneId === 'right'"
    />
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

<script setup lang="ts">
// 双窗格容器：左右各一个 Pane + 可拖拽分隔条（WS-D）
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { getCurrentWindow } from '@tauri-apps/api/window'
import Pane from './Pane.vue'
import { usePaneStore } from '../../stores/pane'
import { useUiStore } from '../../stores/ui'

const paneStore = usePaneStore()
const ui = useUiStore()
const rootRef = ref<HTMLElement | null>(null)
const dragging = ref(false)
let unlistenDragEvents: UnlistenFn[] = []
let dragEventSequence = 0
interface InternalDrag {
  operation: 'move' | 'copy'
  paneId: string | null
  modifier: boolean
  paths: string[]
}

let internalDrag: InternalDrag | null = null
let dragEndTimer: number | undefined
/** 原生拖拽线程报来的松手位置，等 `stelix:drag-end` 确认落下方可使用。 */
let pendingRelease: { x: number; y: number } | null = null
/** 缩放比例在拖拽期间复用，避免每个位置事件都走一次 IPC。 */
let cachedScale: number | null = null

async function currentScale(): Promise<number> {
  if (cachedScale === null) cachedScale = await getCurrentWindow().scaleFactor()
  return cachedScale
}

/** 内部拖拽的路径集合是否与本次落点一致（用于区分应用内拖拽和外部文件拖入）。 */
function samePaths(a: string[], b: string[]): boolean {
  if (!a.length || a.length !== b.length) return false
  const normalize = (path: string) => path.replace(/\//g, '\\').toLowerCase()
  const expected = new Set(b.map(normalize))
  return a.every((path) => expected.has(normalize(path)))
}

function onFileDragStart(event: Event) {
  const detail = (
    event as CustomEvent<{ operation?: 'move' | 'copy'; paneId?: string; modifier?: boolean; paths?: string[] }>
  ).detail
  internalDrag = {
    operation: detail?.operation === 'copy' ? 'copy' : 'move',
    paneId: detail?.paneId ?? null,
    modifier: detail?.modifier === true,
    paths: detail?.paths ?? [],
  }
  if (dragEndTimer !== undefined) {
    window.clearTimeout(dragEndTimer)
    dragEndTimer = undefined
  }
  pendingRelease = null
  cachedScale = null
}

/**
 * 原生 drop 事件可能晚于拖拽结束回调到达，因此延迟清理拖拽信息；
 * 下一次拖拽开始会立即覆盖，落点再用路径比对，避免把外部拖入误判为应用内拖拽。
 */
function onFileDragEnd() {
  if (dragEndTimer !== undefined) window.clearTimeout(dragEndTimer)
  dragEndTimer = window.setTimeout(() => {
    internalDrag = null
    dragEndTimer = undefined
  }, 2000)
}

/** AI 助手分屏时，右侧只保留当前活动窗格（与 AI 对话栏左右均分）。 */
const aiSplit = computed(() => !ui.sidebarCollapsed && ui.activePanel === 'ai')
const singleView = computed(() => paneStore.singlePane || aiSplit.value)

const columns = computed(() => {
  if (paneStore.singlePane || aiSplit.value) return '1fr'
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

/** 把当前拖拽悬停的文件夹广播给文件列表，用于行级高亮。 */
function broadcastDragHover(path: string | null) {
  window.dispatchEvent(new CustomEvent('stelix:drag-hover', { detail: { path } }))
}

onMounted(() => {
  window.addEventListener('stelix:file-drag-start', onFileDragStart)
  window.addEventListener('stelix:file-drag-end', onFileDragEnd)
  void paneStore.init()
  const handleDragPosition = async (
    payload: { paths?: string[]; position: { x: number; y: number } },
    isDrop: boolean,
  ) => {
    const dragPaths = payload.paths ?? []
    const internal = internalDrag && samePaths(dragPaths, internalDrag.paths) ? internalDrag : null
    const dragOperation = isDrop ? internal?.operation ?? null : null
    const sequence = ++dragEventSequence
    try {
      const scale = await currentScale()
      if (sequence !== dragEventSequence) return
      const pointX = payload.position.x / scale
      const pointY = payload.position.y / scale
      const dropElement = document.elementFromPoint(pointX, pointY)?.closest<HTMLElement>('[data-drop-path]')
      const folderPath = dropElement?.dataset.dropDir === 'true' ? dropElement.dataset.dropPath : undefined
      broadcastDragHover(folderPath ?? null)
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
        broadcastDragHover(null)
        return
      }
      // 同窗格拖到空白处视为取消；拖到具体文件夹时按当前拖拽模式执行。
      if (internal?.paneId === paneId && !folderPath && isDrop) {
        broadcastDragHover(null)
        return
      }
      if (!isDrop || !payload.paths?.length) return
      const sourcePaneId = internal?.paneId ?? paneId
      const op = {
        op: dragOperation ?? 'copy',
        sources: payload.paths,
        target: folderPath ?? tab.path,
      }
      try {
        await paneStore.runFs(sourcePaneId, op)
        // runFs 只刷新发起操作的窗格；跨窗格拖拽时再刷新落点窗格。
        if (sourcePaneId !== paneId) await paneStore.refresh(paneId)
        else if (folderPath) await paneStore.reloadByPath(folderPath)
      } finally {
        broadcastDragHover(null)
        internalDrag = null
      }
    } catch (error) {
      broadcastDragHover(null)
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
      // 应用内拖拽的落下统一由 stelix:drag-end 处理：后台线程发起的 OLE 拖拽
      // 不一定能把 Drop 送到 WebView2，两条路都走会移动两次。
      if (internalDrag && samePaths(event.payload.paths ?? [], internalDrag.paths)) return
      void handleDragPosition(event.payload, true)
    }),
    listen('tauri://drag-leave', () => {
      dragEventSequence += 1
      broadcastDragHover(null)
    }),
    // 原生拖拽线程轮询光标报来的位置：应用内拖拽靠它做落点高亮，
    // 不依赖 WebView2 是否收到 drag-over。
    listen<{ x: number; y: number }>('stelix:drag-position', (event) => {
      if (!internalDrag) return
      void handleDragPosition({ paths: internalDrag.paths, position: event.payload }, false)
    }),
    listen<{ x: number; y: number }>('stelix:drag-release', (event) => {
      pendingRelease = event.payload
    }),
    listen<{ outcome: string }>('stelix:drag-end', (event) => {
      const point = pendingRelease
      pendingRelease = null
      if (!internalDrag) return
      if (event.payload.outcome !== 'dropped' || !point) {
        dragEventSequence += 1
        broadcastDragHover(null)
        return
      }
      void handleDragPosition({ paths: internalDrag.paths, position: point }, true)
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
    <!-- AI 分屏时用 v-show 隐藏非活动窗格，保留窗格内部状态（滚动位置、终端缓冲等） -->
    <Pane
      v-if="!paneStore.singlePane || paneStore.activePaneId === 'left'"
      v-show="!aiSplit || paneStore.activePaneId === 'left'"
      pane-id="left"
    />
    <div v-if="!singleView" class="divider" title="拖拽调整宽度，双击均分" @mousedown="startDrag" @dblclick="resetRatio" />
    <Pane
      v-if="!paneStore.singlePane || paneStore.activePaneId === 'right'"
      v-show="!aiSplit || paneStore.activePaneId === 'right'"
      pane-id="right"
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

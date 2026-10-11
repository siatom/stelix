<script setup lang="ts">
// 根布局 —— 由 Phase 0 拥有并冻结。
// 各 workstream 只实现自己被挂载的组件，不要修改本文件的组件树与挂载点。
import { useUiStore } from './stores/ui'
import ActivityBar from './components/Sidebar/ActivityBar.vue'
import SidebarPanel from './components/Sidebar/SidebarPanel.vue'
import DualPane from './components/Pane/DualPane.vue'
import AiPanel from './components/AiPanel.vue'
import CommandPalette from './components/CommandPalette.vue'
import SettingsDialog from './components/SettingsDialog.vue'
import Icon from './components/icons/Icon.vue'
import { onBeforeUnmount, ref } from 'vue'

const ui = useUiStore()

const SIDEBAR_WIDTH_KEY = 'file-povit.sidebarWidth'
const AI_WIDTH_KEY = 'file-povit.aiWidth'
const SIDEBAR_WIDTH_MIN = 180
const SIDEBAR_WIDTH_MAX = 480
const AI_WIDTH_MIN = 240
const AI_WIDTH_MAX = 600

function savedWidth(key: string, fallback: number, min: number, max: number): number {
  try {
    const value = localStorage.getItem(key)
    const saved = value === null ? Number.NaN : Number(value)
    return Number.isFinite(saved) && saved > 0 ? Math.min(max, Math.max(min, saved)) : fallback
  } catch {
    return fallback
  }
}

const sidebarWidth = ref(savedWidth(SIDEBAR_WIDTH_KEY, 264, SIDEBAR_WIDTH_MIN, SIDEBAR_WIDTH_MAX))
const aiWidth = ref(savedWidth(AI_WIDTH_KEY, 360, AI_WIDTH_MIN, AI_WIDTH_MAX))
const resizing = ref<'sidebar' | 'ai' | null>(null)

function onResizeMove(ev: MouseEvent) {
  if (resizing.value === 'sidebar') {
    sidebarWidth.value = Math.min(
      SIDEBAR_WIDTH_MAX,
      Math.max(SIDEBAR_WIDTH_MIN, ev.clientX - 52),
    )
  } else if (resizing.value === 'ai') {
    aiWidth.value = Math.min(AI_WIDTH_MAX, Math.max(AI_WIDTH_MIN, window.innerWidth - ev.clientX))
  }
}

function stopResize() {
  try {
    if (resizing.value === 'sidebar') localStorage.setItem(SIDEBAR_WIDTH_KEY, String(sidebarWidth.value))
    if (resizing.value === 'ai') localStorage.setItem(AI_WIDTH_KEY, String(aiWidth.value))
  } catch {
    /* 忽略持久化失败 */
  }
  resizing.value = null
  window.removeEventListener('mousemove', onResizeMove)
  window.removeEventListener('mouseup', stopResize)
}

function startResize(ev: MouseEvent, target: 'sidebar' | 'ai') {
  ev.preventDefault()
  resizing.value = target
  window.addEventListener('mousemove', onResizeMove)
  window.addEventListener('mouseup', stopResize)
}

onBeforeUnmount(() => {
  window.removeEventListener('mousemove', onResizeMove)
  window.removeEventListener('mouseup', stopResize)
})
</script>

<template>
  <div
    class="app-root"
    :class="{ resizing: resizing !== null }"
    :style="{ '--sidebar-w': `${sidebarWidth}px`, '--ai-w': `${aiWidth}px` }"
  >
    <ActivityBar class="area-activity" />
    <SidebarPanel v-if="!ui.sidebarCollapsed" class="area-sidebar" />
    <div
      v-if="!ui.sidebarCollapsed"
      class="resize-handle sidebar-resize"
      role="separator"
      aria-orientation="vertical"
      :aria-valuenow="sidebarWidth"
      aria-label="调整左侧展开栏宽度"
      title="拖动调整侧栏宽度"
      @mousedown="startResize($event, 'sidebar')"
    />

    <div class="area-main">
      <DualPane />
    </div>

    <AiPanel v-if="ui.aiVisible" class="area-ai" />
    <div
      v-if="ui.aiVisible"
      class="resize-handle ai-resize"
      role="separator"
      aria-orientation="vertical"
      :aria-valuenow="aiWidth"
      aria-label="调整 AI 助手宽度"
      title="拖动调整 AI 助手宽度"
      @mousedown="startResize($event, 'ai')"
    />
    <button
      v-else
      type="button"
      class="area-ai ai-rail"
      title="展开 AI 助手"
      aria-label="展开 AI 助手"
      @click="ui.toggleAi()"
    >
      <Icon name="bot" :size="16" />
    </button>

    <CommandPalette />
    <SettingsDialog />
  </div>
</template>

<style scoped>
.app-root {
  position: relative;
}
.resize-handle {
  position: absolute;
  z-index: 20;
  top: 0;
  bottom: 0;
  width: 8px;
  cursor: col-resize;
  touch-action: none;
}
.resize-handle::after {
  content: '';
  position: absolute;
  top: 0;
  bottom: 0;
  left: 3px;
  width: 2px;
  background: transparent;
}
.resize-handle:hover::after,
.resizing .resize-handle::after {
  background: var(--border-strong);
}
.sidebar-resize {
  left: calc(var(--activity-bar-w) + var(--sidebar-w) - 4px);
}
.ai-resize {
  right: calc(var(--ai-w) - 4px);
}
.resizing {
  cursor: col-resize;
  user-select: none;
}
/* AI 面板收起时，右侧保留一个窄条作为展开入口 */
.ai-rail {
  width: 30px;
  height: 100%;
  display: flex;
  align-items: flex-start;
  justify-content: center;
  padding-top: 8px;
  background: var(--bg-panel);
  border: 1px solid var(--border);
  color: var(--fg-muted);
  transition:
    background 0.12s ease,
    color 0.12s ease;
}
.ai-rail:hover {
  background: var(--bg-hover);
  color: var(--accent);
}
</style>

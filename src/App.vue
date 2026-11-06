<script setup lang="ts">
// 根布局 —— 由 Phase 0 拥有并冻结。
// 各 workstream 只实现自己被挂载的组件，不要修改本文件的组件树与挂载点。
import { useUiStore } from './stores/ui'
import { usePaneStore } from './stores/pane'
import { useAiStore } from './stores/ai'
import { events } from './api'
import { broadcastFsChanged } from './shell/dirWatch'
import { match, isCapturingShortcut } from './stores/shortcuts'
import ActivityBar from './components/Sidebar/ActivityBar.vue'
import SidebarPanel from './components/Sidebar/SidebarPanel.vue'
import DualPane from './components/Pane/DualPane.vue'
import TransferPanel from './components/Pane/TransferPanel.vue'
import TitlebarActions from './components/TitlebarActions.vue'
import CommandPalette from './components/CommandPalette.vue'
import SettingsDialog from './components/SettingsDialog.vue'
import FeedbackDialog from './components/FeedbackDialog.vue'
import ConflictDialog from './components/ConflictDialog.vue'
import SyncDialog from './components/Pane/SyncDialog.vue'
import PreviewOverlay from './components/PreviewOverlay.vue'
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { LogicalSize, PhysicalPosition, PhysicalSize } from '@tauri-apps/api/dpi'

const ui = useUiStore()
const paneStore = usePaneStore()
const aiStore = useAiStore()
const appWindow = getCurrentWindow()
const isMacos = typeof navigator !== 'undefined' && /Mac/i.test(navigator.userAgent)
const isWindows = typeof navigator !== 'undefined' && /Windows/i.test(navigator.userAgent)
const isMaximized = ref(false)
const titlebarMenu = ref<{ x: number; y: number } | null>(null)
const titlebarActionMode = ref<'move' | 'size' | null>(null)
const titlebarModeHint = ref('')
let unlistenResize: (() => void) | undefined
let unlistenFsChanged: (() => void) | undefined
let unlistenDirSize: (() => void) | undefined
let unlistenAiToken: (() => void) | undefined
let unlistenAiStep: (() => void) | undefined
let unlistenRemoteTransfer: (() => void) | undefined
let unlistenFsProgress: (() => void) | undefined
let titlebarPointer: { x: number; y: number } | null = null
let actionPosition: PhysicalPosition | null = null
let actionSize: PhysicalSize | null = null

async function syncMaximizedState() {
  isMaximized.value = await appWindow.isMaximized()
}

async function toggleMaximize() {
  if (isMaximized.value) {
    await appWindow.unmaximize()
    await appWindow.setSize(new LogicalSize(1100, 700))
    await appWindow.center()
  } else {
    await appWindow.maximize()
  }
  await syncMaximizedState()
}

function stopTrackingTitlebarDrag() {
  titlebarPointer = null
  window.removeEventListener('mousemove', onTitlebarMouseMove)
  window.removeEventListener('mouseup', stopTrackingTitlebarDrag)
}

function onTitlebarMouseMove(event: MouseEvent) {
  if (!titlebarPointer) return
  const dx = event.screenX - titlebarPointer.x
  const dy = event.screenY - titlebarPointer.y
  if (dx * dx + dy * dy < 36) return
  stopTrackingTitlebarDrag()
  void appWindow.startDragging()
}

function startWindowDrag(event: MouseEvent) {
  if (event.button !== 0 || (event.target as HTMLElement).closest('button')) return
  titlebarPointer = { x: event.screenX, y: event.screenY }
  window.addEventListener('mousemove', onTitlebarMouseMove)
  window.addEventListener('mouseup', stopTrackingTitlebarDrag)
}

function toggleMaximizeFromTitlebar(event: MouseEvent) {
  if ((event.target as HTMLElement).closest('button')) return
  void toggleMaximize()
}

function openTitlebarMenu(event: MouseEvent) {
  if ((event.target as HTMLElement).closest('.titlebar-controls')) return
  event.preventDefault()
  const menuWidth = 190
  const menuHeight = isMaximized.value ? 190 : 230
  titlebarMenu.value = {
    x: Math.max(8, Math.min(event.clientX, window.innerWidth - menuWidth - 8)),
    y: Math.max(8, Math.min(event.clientY, window.innerHeight - menuHeight - 8)),
  }
}

function closeTitlebarMenu() {
  titlebarMenu.value = null
}

function onAltF4(event: KeyboardEvent) {
  if (isCapturingShortcut()) return // 正在设置里捕获新键位，不要真的关窗
  if (match(event) !== 'close-window') return
  event.preventDefault()
  event.stopPropagation()
  closeTitlebarMenu()
  void appWindow.close()
}

/** Ctrl+F：打开侧边栏搜索面板并聚焦输入框 */
function onSearchShortcut(event: KeyboardEvent) {
  if (isCapturingShortcut()) return // 正在设置里捕获新键位
  if (ui.settingsOpen || ui.feedbackOpen) return // 弹窗打开时不抢焦点
  if (event.target instanceof HTMLElement && event.target.closest('.terminal-view')) return // 终端内按键交给 xterm
  if (match(event) !== 'search') return
  event.preventDefault()
  event.stopPropagation()
  closeTitlebarMenu()
  ui.openSearch()
}

function dismissTitlebarMenu(event: MouseEvent) {
  if (!(event.target as HTMLElement).closest('.titlebar-context-menu')) closeTitlebarMenu()
}

async function beginTitlebarAction(mode: 'move' | 'size') {
  closeTitlebarMenu()
  titlebarActionMode.value = mode
  if (mode === 'move') {
    actionPosition = await appWindow.outerPosition()
    titlebarModeHint.value = '使用方向键移动窗口，Enter 确认，Esc 取消'
  } else {
    actionSize = await appWindow.outerSize()
    titlebarModeHint.value = '使用方向键调整窗口大小，Enter 确认，Esc 取消'
  }
}

async function onTitlebarActionKeydown(event: KeyboardEvent) {
  const mode = titlebarActionMode.value
  if (!mode) {
    if (event.key === 'Escape') closeTitlebarMenu()
    return
  }

  if (event.key === 'Escape') {
    event.preventDefault()
    if (mode === 'move' && actionPosition) await appWindow.setPosition(actionPosition)
    if (mode === 'size' && actionSize) await appWindow.setSize(actionSize)
    titlebarActionMode.value = null
    titlebarModeHint.value = ''
    return
  }
  if (event.key === 'Enter') {
    event.preventDefault()
    titlebarActionMode.value = null
    titlebarModeHint.value = ''
    return
  }
  if (!event.key.startsWith('Arrow')) return

  event.preventDefault()
  const step = event.shiftKey ? 1 : 10
  if (mode === 'move' && actionPosition) {
    const dx = event.key === 'ArrowLeft' ? -step : event.key === 'ArrowRight' ? step : 0
    const dy = event.key === 'ArrowUp' ? -step : event.key === 'ArrowDown' ? step : 0
    actionPosition = new PhysicalPosition(actionPosition.x + dx, actionPosition.y + dy)
    await appWindow.setPosition(actionPosition)
  } else if (mode === 'size' && actionSize) {
    const dw = event.key === 'ArrowLeft' ? -step : event.key === 'ArrowRight' ? step : 0
    const dh = event.key === 'ArrowUp' ? -step : event.key === 'ArrowDown' ? step : 0
    actionSize = new PhysicalSize(Math.max(640, actionSize.width + dw), Math.max(480, actionSize.height + dh))
    await appWindow.setSize(actionSize)
  }
}

onMounted(async () => {
  await syncMaximizedState()
  // 挂载时窗口可能仍在完成最大化，重新取一次宽度，保证 AI 分屏按真实宽度均分。
  onWindowResize()
  unlistenResize = await appWindow.onResized(syncMaximizedState)
  window.addEventListener('mousedown', dismissTitlebarMenu)
  window.addEventListener('keydown', onTitlebarActionKeydown)
  window.addEventListener('keydown', onAltF4, true)
  window.addEventListener('keydown', onSearchShortcut)
  window.addEventListener('resize', onWindowResize)
  void ui.startIndexStatusWatch()
  // 目录变更：刷新所有打开该目录的标签（跨窗格 / 后台标签），并广播给左侧目录树。
  unlistenFsChanged = await events.onFsChanged(({ dirs }) => {
    if (!dirs?.length) return
    broadcastFsChanged(dirs)
    for (const dir of dirs) void paneStore.reloadByPath(dir)
    // 目录内容变化 → 相关文件夹的大小缓存失效并重算。
    paneStore.invalidateDirSizes(dirs)
  })
  // 文件操作进度（按 opId 归属到队列项，供右下角进度面板展示）。
  unlistenFsProgress = await events.onFsProgress((progress) => {
    paneStore.applyFsProgress(progress)
  })
  // 文件夹大小统计结果（详情视图按需计算）回填。
  unlistenDirSize = await events.onDirSize(({ path, size }) => {
    paneStore.applyDirSize(path, size)
  })
  // AI 流式增量与工具步骤（按 requestId 路由到发起该请求的消息）。
  unlistenAiToken = await events.onAiToken(({ requestId, delta }) => {
    aiStore.applyToken(requestId, delta)
  })
  unlistenAiStep = await events.onAiStep(({ requestId, step }) => {
    aiStore.applyStep(requestId, step)
  })
  // 远程上传 / 下载进度：状态栏显示当前条目（传输本身在后台线程执行）。
  unlistenRemoteTransfer = await events.onRemoteTransfer((progress) => {
    const verb = progress.direction === 'download' ? '下载' : '上传'
    const index = Math.min(progress.done + 1, progress.total)
    const name = progress.current.split(/[\\/]/).filter(Boolean).pop() ?? progress.current
    ui.setStatusText(`正在${verb} ${index}/${progress.total}：${name}`)
  })
})

const SIDEBAR_WIDTH_KEY = 'stelix.sidebarWidth'
const SIDEBAR_WIDTH_MIN = 180
const SIDEBAR_WIDTH_MAX = 480

// AI 助手打开时：左屏为对话栏，右屏为当前活动窗格，拖动分隔线调整比例。
const AI_SPLIT_KEY = 'stelix.aiSplitRatio'
const AI_SPLIT_DEF = 0.5
const AI_PANEL_MIN = 300
const AI_MAIN_MIN = 340
const ACTIVITY_BAR_W = 52

function savedWidth(key: string, fallback: number, min: number, max: number): number {
  try {
    const value = localStorage.getItem(key)
    const saved = value === null ? Number.NaN : Number(value)
    return Number.isFinite(saved) && saved > 0 ? Math.min(max, Math.max(min, saved)) : fallback
  } catch {
    return fallback
  }
}

function readAiSplitRatio(): number {
  try {
    const saved = Number(localStorage.getItem(AI_SPLIT_KEY))
    return Number.isFinite(saved) && saved > 0 ? Math.min(1, saved) : AI_SPLIT_DEF
  } catch {
    return AI_SPLIT_DEF
  }
}

const sidebarWidth = ref(savedWidth(SIDEBAR_WIDTH_KEY, 264, SIDEBAR_WIDTH_MIN, SIDEBAR_WIDTH_MAX))
const resizing = ref<'sidebar' | null>(null)
const aiSplitRatio = ref(readAiSplitRatio())
const viewportWidth = ref(window.innerWidth)
const aiSplit = computed(() => !ui.sidebarCollapsed && ui.activePanel === 'ai')
const availableWidth = computed(() => Math.max(240, viewportWidth.value - ACTIVITY_BAR_W))

/** AI 模式下侧栏（左屏）宽度 = 可用宽度 × 比例，并保证两侧都有可用最小宽度。 */
const effectiveSidebarWidth = computed(() => {
  if (!aiSplit.value) return sidebarWidth.value
  const available = availableWidth.value
  const minSide = Math.min(AI_PANEL_MIN, Math.round(available * 0.5))
  const maxSide = Math.max(minSide, available - AI_MAIN_MIN)
  return Math.min(maxSide, Math.max(minSide, Math.round(available * aiSplitRatio.value)))
})

function persistAiSplit() {
  try {
    localStorage.setItem(AI_SPLIT_KEY, String(aiSplitRatio.value))
  } catch {
    /* 忽略持久化失败 */
  }
}

function onWindowResize() {
  viewportWidth.value = window.innerWidth
}

function onResizeMove(ev: MouseEvent) {
  if (resizing.value !== 'sidebar') return
  if (aiSplit.value) {
    const available = availableWidth.value
    const minSide = Math.min(AI_PANEL_MIN, Math.round(available * 0.5))
    const maxSide = Math.max(minSide, available - AI_MAIN_MIN)
    const side = Math.min(maxSide, Math.max(minSide, ev.clientX - ACTIVITY_BAR_W))
    aiSplitRatio.value = side / available
    return
  }
  sidebarWidth.value = Math.min(
    SIDEBAR_WIDTH_MAX,
    Math.max(SIDEBAR_WIDTH_MIN, ev.clientX - ACTIVITY_BAR_W),
  )
}

function stopResize() {
  try {
    if (resizing.value === 'sidebar') {
      if (aiSplit.value) persistAiSplit()
      else localStorage.setItem(SIDEBAR_WIDTH_KEY, String(sidebarWidth.value))
    }
  } catch {
    /* 忽略持久化失败 */
  }
  resizing.value = null
  window.removeEventListener('mousemove', onResizeMove)
  window.removeEventListener('mouseup', stopResize)
}

/** 双击分隔线：AI 分屏恢复均分 */
function resetSplit() {
  if (!aiSplit.value) return
  aiSplitRatio.value = AI_SPLIT_DEF
  persistAiSplit()
}

function startResize(ev: MouseEvent, target: 'sidebar') {
  ev.preventDefault()
  resizing.value = target
  window.addEventListener('mousemove', onResizeMove)
  window.addEventListener('mouseup', stopResize)
}

onBeforeUnmount(() => {
  unlistenResize?.()
  unlistenFsChanged?.()
  unlistenDirSize?.()
  unlistenAiToken?.()
  unlistenAiStep?.()
  unlistenRemoteTransfer?.()
  unlistenFsProgress?.()
  stopTrackingTitlebarDrag()
  window.removeEventListener('mousedown', dismissTitlebarMenu)
  window.removeEventListener('keydown', onTitlebarActionKeydown)
  window.removeEventListener('keydown', onAltF4, true)
  window.removeEventListener('keydown', onSearchShortcut)
  window.removeEventListener('resize', onWindowResize)
  window.removeEventListener('mousemove', onResizeMove)
  window.removeEventListener('mouseup', stopResize)
})
</script>

<template>
  <div
    class="app-root"
    :class="{ resizing: resizing !== null, 'ai-split': aiSplit, macos: isMacos, 'sidebar-collapsed': ui.sidebarCollapsed }"
    :style="{ '--sidebar-w': `${effectiveSidebarWidth}px` }"
  >
    <img
      v-if="ui.settingsBackgroundImage"
      class="app-background-image"
      :src="ui.settingsBackgroundImage"
      alt=""
      aria-hidden="true"
    />
    <header
      class="app-titlebar"
      @mousedown="startWindowDrag"
      @dblclick="toggleMaximizeFromTitlebar"
      @contextmenu="openTitlebarMenu"
    >
      <div v-if="isWindows" class="titlebar-brand">
        <img src="/brand-mark.svg" alt="" draggable="false" />
        <span>Stelix 星索</span>
      </div>
      <div class="titlebar-center">
        <TitlebarActions />
      </div>
      <div class="titlebar-side titlebar-right">
        <div v-if="!isMacos" class="titlebar-controls">
          <button class="titlebar-button" type="button" aria-label="最小化" title="最小化" @click="appWindow.minimize()">
            <svg viewBox="0 0 12 12" aria-hidden="true"><path d="M2 6h8" /></svg>
          </button>
          <button class="titlebar-button" type="button" :aria-label="isMaximized ? '还原' : '最大化'" :title="isMaximized ? '还原' : '最大化'" @click="toggleMaximize()">
            <svg v-if="isMaximized" viewBox="0 0 12 12" aria-hidden="true"><path d="M4 2.5h5.5V8M8 4H2.5v5.5H8z" /></svg>
            <svg v-else viewBox="0 0 12 12" aria-hidden="true"><rect x="2.5" y="2.5" width="7" height="7" rx=".5" /></svg>
          </button>
          <button class="titlebar-button close-button" type="button" aria-label="关闭" title="关闭" @click="appWindow.close()">
            <svg viewBox="0 0 12 12" aria-hidden="true"><path d="m3 3 6 6M9 3 3 9" /></svg>
          </button>
        </div>
      </div>
    </header>
    <div
      v-if="titlebarMenu"
      class="titlebar-context-menu"
      role="menu"
      aria-label="窗口菜单"
      :style="{ left: `${titlebarMenu.x}px`, top: `${titlebarMenu.y}px` }"
      @mousedown.stop
      @contextmenu.prevent.stop
    >
      <button type="button" role="menuitem" :disabled="!isMaximized" @click="toggleMaximize">还原</button>
      <button type="button" role="menuitem" :disabled="isMaximized" @click="beginTitlebarAction('move')">移动</button>
      <button type="button" role="menuitem" :disabled="isMaximized" @click="beginTitlebarAction('size')">大小</button>
      <div class="titlebar-menu-separator" />
      <button type="button" role="menuitem" @click="appWindow.minimize(); closeTitlebarMenu()">最小化</button>
      <button type="button" role="menuitem" :disabled="isMaximized" @click="appWindow.maximize(); closeTitlebarMenu()">最大化</button>
      <div class="titlebar-menu-separator" />
      <button type="button" role="menuitem" class="titlebar-menu-close" @click="appWindow.close()">
        <span>关闭</span><span class="titlebar-menu-shortcut">Alt+F4</span>
      </button>
    </div>
    <div v-if="titlebarActionMode" class="titlebar-mode-hint" role="status">{{ titlebarModeHint }}</div>
    <ActivityBar class="area-activity" />
    <SidebarPanel v-if="!ui.sidebarCollapsed" class="area-sidebar" />
    <div
      v-if="!ui.sidebarCollapsed"
      class="resize-handle sidebar-resize"
      role="separator"
      aria-orientation="vertical"
      :aria-valuenow="effectiveSidebarWidth"
      :aria-label="aiSplit ? '调整 AI 助手与文件窗格的宽度' : '调整左侧展开栏宽度'"
      :title="aiSplit ? '拖动调整宽度，双击均分' : '拖动调整侧栏宽度'"
      @mousedown="startResize($event, 'sidebar')"
      @dblclick="resetSplit"
    />

    <div class="area-main">
      <DualPane />
      <TransferPanel />
    </div>

    <CommandPalette />
    <SettingsDialog />
    <FeedbackDialog />
    <ConflictDialog
      v-if="paneStore.conflictPrompt"
      :items="paneStore.conflictPrompt.items"
      :kind="paneStore.conflictPrompt.kind"
      @resolve="paneStore.resolveConflict"
    />
    <SyncDialog v-if="ui.syncOpen" />
    <PreviewOverlay />
  </div>
</template>

<style scoped>
.app-root {
  position: relative;
  isolation: isolate;
}
.app-background-image {
  position: absolute;
  z-index: -1;
  inset: 0;
  width: 100%;
  height: 100%;
  object-fit: cover;
  pointer-events: none;
  user-select: none;
}
.app-titlebar {
  grid-area: titlebar;
  position: relative;
  display: grid;
  /* 与下方根布局保持同一套列宽：图标栏 | 侧栏 | 主区（两个文件窗格）。 */
  grid-template-columns: var(--activity-bar-w) var(--sidebar-w) 1fr;
  align-items: center;
  /* 通栏标题栏：品牌在左，工具按钮在主区居中，窗口控制在最右。 */
  height: 40px;
  font-family: 'Microsoft YaHei', '微软雅黑', sans-serif;
  color: var(--fg-muted);
  background: var(--bg-app);
  border-bottom: 1px solid var(--border);
  user-select: none;
}
/* 收起侧栏后第二列在根布局中消失，标题栏同步收窄，保持与文件窗格对齐。 */
.app-root.sidebar-collapsed .app-titlebar {
  grid-template-columns: var(--activity-bar-w) 0 1fr;
}
.titlebar-side {
  display: flex;
  align-items: center;
  min-width: 0;
}
.titlebar-brand {
  position: absolute;
  left: 12px;
  display: flex;
  align-items: center;
  gap: 8px;
  color: var(--fg);
  font-size: 14px;
  font-weight: 600;
  white-space: nowrap;
}
.titlebar-brand img {
  width: 22px;
  height: 22px;
}
.titlebar-center {
  grid-column: 3;
  justify-self: center;
  display: flex;
  align-items: center;
  justify-content: center;
}
.titlebar-right {
  position: absolute;
  top: 0;
  right: 10px;
  bottom: 0;
  justify-content: flex-end;
}
.titlebar-controls {
  display: flex;
  align-items: center;
  gap: 6px;
}
.titlebar-button {
  display: grid;
  place-items: center;
  width: 34px;
  height: 30px;
  border: none;
  border-radius: 8px;
  background: transparent;
  color: var(--fg-muted);
  box-shadow: none;
  transition:
    background 0.16s ease,
    color 0.16s ease,
    border-color 0.16s ease,
    box-shadow 0.16s ease,
    transform 0.16s ease;
}
.titlebar-button:hover {
  background: transparent;
  color: var(--fg);
  transform: translateY(-1px);
}
.titlebar-button:active {
  box-shadow: none;
  transform: translateY(0) scale(0.96);
}
.titlebar-button svg {
  width: 14px;
  height: 14px;
  fill: none;
  stroke: currentColor;
  stroke-width: 1.35;
  stroke-linecap: round;
  stroke-linejoin: round;
}
.titlebar-button.close-button:hover {
  color: #d94b4b;
  background: transparent;
  box-shadow: none;
}
.titlebar-context-menu {
  position: fixed;
  z-index: 1000;
  width: 190px;
  padding: 5px;
  border: 1px solid var(--border);
  border-radius: 9px;
  background: var(--bg-elevated);
  box-shadow: var(--shadow-md);
  color: var(--fg);
}
.titlebar-context-menu button {
  display: flex;
  align-items: center;
  width: 100%;
  min-height: 26px;
  padding: 0 10px;
  border-radius: 5px;
  text-align: left;
  font-size: 12px;
}
.titlebar-context-menu button:hover:not(:disabled) {
  background: var(--bg-hover);
}
.titlebar-menu-close {
  justify-content: space-between;
}
.titlebar-menu-shortcut {
  color: var(--fg-dim);
  font-size: 11px;
}
.titlebar-context-menu button:disabled {
  color: var(--fg-dim);
  opacity: 0.55;
}
.titlebar-menu-separator {
  height: 1px;
  margin: 3px 5px;
  background: var(--border);
}
.titlebar-mode-hint {
  position: fixed;
  z-index: 999;
  top: 50px;
  left: 50%;
  padding: 7px 12px;
  transform: translateX(-50%);
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--bg-elevated);
  box-shadow: var(--shadow-sm);
  color: var(--fg-muted);
  font-size: 12px;
  pointer-events: none;
}
.resize-handle {
  position: absolute;
  z-index: 20;
  top: 40px;
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
/* AI 分屏：常态显示细分割线，与左右文件窗格观感一致 */
.ai-split .resize-handle::after {
  background: var(--border);
}
.resize-handle:hover::after,
.resizing .resize-handle::after {
  background: var(--border-strong);
}
.sidebar-resize {
  left: calc(var(--activity-bar-w) + var(--sidebar-w) - 4px);
}
.resizing {
  cursor: col-resize;
  user-select: none;
}
</style>

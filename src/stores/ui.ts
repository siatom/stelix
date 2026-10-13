import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { api, events } from '../api'
import { resetAllShortcuts } from './shortcuts'
import type { IndexProgress } from '../types'

/** 左侧栏面板类型 */
export type PanelKey = 'files' | 'tasks' | 'search' | 'ai'

/** 设置面板分类 */
export type SettingsTab = 'general' | 'index' | 'shortcuts' | 'ai' | 'about'

/** 主题模式 */
export type ThemeMode = 'system' | 'dark' | 'light'

// ---- 本地偏好持久化（localStorage）----
const LS_THEME = 'stelix.theme'
const LS_HIDE_PROTECTED = 'stelix.hideProtected'
const LS_HIDE_FILE_EXTENSIONS = 'stelix.hideFileExtensions'
const LS_SIDEBAR_COLLAPSED = 'stelix.sidebarCollapsed'
const LS_FONT_SIZE = 'stelix.fontSize'
const LS_SIDEBAR_FONT_SIZE = 'stelix.sidebarFontSize'
const LS_FILE_FONT_SIZE = 'stelix.fileFontSize'
const LS_SIDEBAR_ROW_H = 'stelix.sidebarRowHeight'
const LS_FILE_ROW_H = 'stelix.fileRowHeight'

// 行高可选范围与默认值（px）。默认值与 tokens.css 的 --sidebar-row-h / --file-row-h 保持一致。
const SIDEBAR_ROW_MIN = 20
const SIDEBAR_ROW_MAX = 32
const SIDEBAR_ROW_DEF = 26
const FILE_ROW_MIN = 22
const FILE_ROW_MAX = 40
const FILE_ROW_DEF = 26
const SIDEBAR_FONT_SIZE_DEF = 13
const FILE_FONT_SIZE_DEF = 13

// 视图设置默认值（「恢复默认」回到这些值）
const FONT_SIZE_MIN = 11
const FONT_SIZE_MAX = 16
const FONT_SIZE_DEF = 12
const HIDE_PROTECTED_DEF = true
const HIDE_FILE_EXTENSIONS_DEF = false

function readFontSize(key = LS_FONT_SIZE, fallback = FONT_SIZE_DEF): number {
  try {
    const saved = Number(localStorage.getItem(key))
    return Number.isInteger(saved) && saved >= FONT_SIZE_MIN && saved <= FONT_SIZE_MAX ? saved : fallback
  } catch {
    return fallback
  }
}

function readRowHeight(key: string, min: number, max: number, def: number): number {
  try {
    const saved = Number(localStorage.getItem(key))
    return Number.isInteger(saved) && saved >= min && saved <= max ? saved : def
  } catch {
    return def
  }
}

function readBool(key: string, fallback: boolean): boolean {
  try {
    const v = localStorage.getItem(key)
    return v === null ? fallback : v === '1'
  } catch {
    return fallback
  }
}

function readTheme(): ThemeMode {
  try {
    const saved = localStorage.getItem(LS_THEME)
    return saved === 'dark' || saved === 'light' ? saved : 'system'
  } catch {
    return 'system'
  }
}

function systemTheme(): 'dark' | 'light' {
  return window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light'
}

function applyTheme(mode: ThemeMode) {
  document.documentElement.setAttribute('data-theme', mode === 'system' ? systemTheme() : mode)
}

function applyFontSize(size: number) {
  document.documentElement.style.setProperty('--ui-font-size', `${size}px`)
}

function applySidebarFontSize(size: number) {
  document.documentElement.style.setProperty('--sidebar-font-size', `${size}px`)
}

function applyFileFontSize(size: number) {
  document.documentElement.style.setProperty('--file-font-size', `${size}px`)
}

function applyRowHeights(sidebar: number, file: number) {
  const style = document.documentElement.style
  style.setProperty('--sidebar-row-h', `${sidebar}px`)
  style.setProperty('--file-row-h', `${file}px`)
}

/**
 * UI 全局状态 —— 由 **WS-C (feat/ui-shell)** 实现。
 * ⚠️ App.vue（Foundation 拥有）依赖以下字段名，WS-C 不得改名：
 * `sidebarCollapsed`
 */
export const useUiStore = defineStore('ui', () => {
  const sidebarCollapsed = ref(readBool(LS_SIDEBAR_COLLAPSED, false))
  const theme = ref<ThemeMode>(readTheme())
  const fontSize = ref(readFontSize())
  const sidebarFontSize = ref(readFontSize(LS_SIDEBAR_FONT_SIZE, SIDEBAR_FONT_SIZE_DEF))
  const fileFontSize = ref(readFontSize(LS_FILE_FONT_SIZE, FILE_FONT_SIZE_DEF))
  const sidebarRowHeight = ref(readRowHeight(LS_SIDEBAR_ROW_H, SIDEBAR_ROW_MIN, SIDEBAR_ROW_MAX, SIDEBAR_ROW_DEF))
  const fileRowHeight = ref(readRowHeight(LS_FILE_ROW_H, FILE_ROW_MIN, FILE_ROW_MAX, FILE_ROW_DEF))
  const resolvedTheme = computed(() => theme.value === 'system' ? systemTheme() : theme.value)
  const hideProtected = ref(readBool(LS_HIDE_PROTECTED, HIDE_PROTECTED_DEF))
  const hideFileExtensions = ref(readBool(LS_HIDE_FILE_EXTENSIONS, HIDE_FILE_EXTENSIONS_DEF))
  // 每次启动都落在「文件导航」面板（不持久化上次停留的面板）
  const activePanel = ref<PanelKey>('files')
  const searchScope = ref<'global' | 'current'>('global')
  const searchCurrentRequest = ref(0)
  /** 一次性标记：下一次搜索面板挂载/响应时应聚焦输入框（消费后清除） */
  const searchFocusPending = ref(false)
  const settingsOpen = ref(false)
  const settingsTab = ref<SettingsTab>('general')
  const feedbackOpen = ref(false)
  const statusText = ref('就绪')
  const indexTotal = ref(0)

  // 首次进入时应用已持久化的主题
  applyTheme(theme.value)
  applyFontSize(fontSize.value)
  applySidebarFontSize(sidebarFontSize.value)
  applyFileFontSize(fileFontSize.value)
  applyRowHeights(sidebarRowHeight.value, fileRowHeight.value)
  const systemThemeQuery = window.matchMedia('(prefers-color-scheme: dark)')
  systemThemeQuery.addEventListener('change', () => {
    if (theme.value === 'system') applyTheme('system')
  })

  /** 索引根目录列表（由设置面板维护） */
  const indexRoots = ref<string[]>([])
  /** 是否正在扫描 / 建立索引 */
  const scanning = ref(false)
  /** 最近一次索引已扫描条目数 */
  const scanned = ref(0)

  function toggleSidebar() {
    sidebarCollapsed.value = !sidebarCollapsed.value
    persistSidebar()
  }

  function setPanel(panel: PanelKey) {
    if (activePanel.value === panel && !sidebarCollapsed.value) {
      sidebarCollapsed.value = true
    } else {
      activePanel.value = panel
      sidebarCollapsed.value = false
      // 打开搜索面板时聚焦输入框（图标栏、命令面板、右键「搜索此文件夹」统一走这条）
      if (panel === 'search') searchFocusPending.value = true
    }
    persistSidebar()
  }

  function openCurrentDirectorySearch() {
    searchScope.value = 'current'
    searchCurrentRequest.value += 1
    searchFocusPending.value = true
    activePanel.value = 'search'
    sidebarCollapsed.value = false
    persistSidebar()
  }

  function persistSidebar() {
    try {
      localStorage.setItem(LS_SIDEBAR_COLLAPSED, sidebarCollapsed.value ? '1' : '0')
    } catch {
      /* ignore */
    }
  }

  function openSettings(tab: SettingsTab = 'general') {
    settingsTab.value = tab
    settingsOpen.value = true
  }

  function closeSettings() {
    settingsOpen.value = false
  }

  function openFeedback() {
    feedbackOpen.value = true
  }

  function closeFeedback() {
    feedbackOpen.value = false
  }

  function setTheme(mode: ThemeMode) {
    theme.value = mode
    applyTheme(mode)
    try {
      localStorage.setItem(LS_THEME, mode)
    } catch {
      /* ignore */
    }
  }

  function toggleTheme() {
    setTheme(theme.value === 'system' ? 'light' : theme.value === 'light' ? 'dark' : 'system')
  }

  function setFontSize(size: number) {
    if (!Number.isInteger(size) || size < FONT_SIZE_MIN || size > FONT_SIZE_MAX) return
    fontSize.value = size
    applyFontSize(size)
    try {
      localStorage.setItem(LS_FONT_SIZE, String(size))
    } catch {
      /* ignore */
    }
  }

  function setSidebarFontSize(size: number) {
    if (!Number.isInteger(size) || size < FONT_SIZE_MIN || size > FONT_SIZE_MAX) return
    sidebarFontSize.value = size
    applySidebarFontSize(size)
    try {
      localStorage.setItem(LS_SIDEBAR_FONT_SIZE, String(size))
    } catch {
      /* ignore */
    }
  }

  function setFileFontSize(size: number) {
    if (!Number.isInteger(size) || size < FONT_SIZE_MIN || size > FONT_SIZE_MAX) return
    fileFontSize.value = size
    applyFileFontSize(size)
    try {
      localStorage.setItem(LS_FILE_FONT_SIZE, String(size))
    } catch {
      /* ignore */
    }
  }

  function setSidebarRowHeight(size: number) {
    if (!Number.isInteger(size) || size < SIDEBAR_ROW_MIN || size > SIDEBAR_ROW_MAX) return
    sidebarRowHeight.value = size
    applyRowHeights(sidebarRowHeight.value, fileRowHeight.value)
    try {
      localStorage.setItem(LS_SIDEBAR_ROW_H, String(size))
    } catch {
      /* ignore */
    }
  }

  function setFileRowHeight(size: number) {
    if (!Number.isInteger(size) || size < FILE_ROW_MIN || size > FILE_ROW_MAX) return
    fileRowHeight.value = size
    applyRowHeights(sidebarRowHeight.value, fileRowHeight.value)
    try {
      localStorage.setItem(LS_FILE_ROW_H, String(size))
    } catch {
      /* ignore */
    }
  }

  function setHideProtected(value: boolean) {
    hideProtected.value = value
    try {
      localStorage.setItem(LS_HIDE_PROTECTED, value ? '1' : '0')
    } catch {
      /* ignore */
    }
  }

  function toggleHideProtected() {
    setHideProtected(!hideProtected.value)
  }

  function setHideFileExtensions(value: boolean) {
    hideFileExtensions.value = value
    try {
      localStorage.setItem(LS_HIDE_FILE_EXTENSIONS, value ? '1' : '0')
    } catch {
      /* ignore */
    }
  }

  /** 恢复「视图 / 快捷键」相关设置的默认值（不含索引根目录 / 开机自启动等系统级设置） */
  function resetViewDefaults() {
    try {
      localStorage.removeItem(LS_FONT_SIZE)
      localStorage.removeItem(LS_SIDEBAR_FONT_SIZE)
      localStorage.removeItem(LS_FILE_FONT_SIZE)
      localStorage.removeItem(LS_SIDEBAR_ROW_H)
      localStorage.removeItem(LS_FILE_ROW_H)
      localStorage.removeItem(LS_HIDE_PROTECTED)
      localStorage.removeItem(LS_HIDE_FILE_EXTENSIONS)
    } catch {
      /* ignore */
    }
    fontSize.value = FONT_SIZE_DEF
    sidebarFontSize.value = SIDEBAR_FONT_SIZE_DEF
    fileFontSize.value = FILE_FONT_SIZE_DEF
    sidebarRowHeight.value = SIDEBAR_ROW_DEF
    fileRowHeight.value = FILE_ROW_DEF
    hideProtected.value = HIDE_PROTECTED_DEF
    hideFileExtensions.value = HIDE_FILE_EXTENSIONS_DEF
    applyFontSize(fontSize.value)
    applySidebarFontSize(sidebarFontSize.value)
    applyFileFontSize(fileFontSize.value)
    applyRowHeights(sidebarRowHeight.value, fileRowHeight.value)
    resetAllShortcuts()
  }

  function displayFileName(name: string, isDir: boolean): string {
    if (!hideFileExtensions.value || isDir) return name
    const dot = name.lastIndexOf('.')
    return dot > 0 ? name.slice(0, dot) : name
  }

  function setIndexRoots(roots: string[]) {
    indexRoots.value = [...roots]
  }

  function setStatusText(text: string) {
    statusText.value = text
  }

  // ---- 索引状态：全局订阅（供侧边栏搜索面板的状态栏展示）----
  let indexWatchStarted = false

  async function refreshIndexStatus() {
    try {
      const s = await api.indexStatus()
      indexTotal.value = s.total
      scanning.value = s.scanning
      scanned.value = s.scanned
      if (!indexRoots.value.length && s.roots.length) indexRoots.value = [...s.roots]
    } catch {
      /* 索引状态暂不可用（例如后端未就绪） */
    }
  }

  /** 订阅索引进度事件，仅在首次调用时建立监听（幂等）。 */
  async function startIndexStatusWatch() {
    if (indexWatchStarted) return
    indexWatchStarted = true
    await refreshIndexStatus()
    await events.onIndexProgress((p) => {
      const prog = p as IndexProgress
      scanned.value = prog.scanned
      if (typeof prog.total === 'number' && prog.total > 0) indexTotal.value = prog.total
      scanning.value = !prog.done
    })
    await events.onIndexDone(() => {
      scanning.value = false
      void refreshIndexStatus()
    })
  }

  return {
    sidebarCollapsed,
    theme,
    fontSize,
    sidebarFontSize,
    fileFontSize,
    sidebarRowHeight,
    fileRowHeight,
    resolvedTheme,
    hideProtected,
    hideFileExtensions,
    activePanel,
    searchScope,
    searchCurrentRequest,
    searchFocusPending,
    settingsOpen,
    settingsTab,
    feedbackOpen,
    statusText,
    indexTotal,
    indexRoots,
    scanning,
    scanned,
    toggleSidebar,
    setPanel,
    openCurrentDirectorySearch,
    openSettings,
    closeSettings,
    openFeedback,
    closeFeedback,
    setTheme,
    toggleTheme,
    setFontSize,
    setSidebarFontSize,
    setFileFontSize,
    setSidebarRowHeight,
    setFileRowHeight,
    setHideProtected,
    toggleHideProtected,
    setHideFileExtensions,
    resetViewDefaults,
    displayFileName,
    setIndexRoots,
    setStatusText,
    refreshIndexStatus,
    startIndexStatusWatch,
  }
})

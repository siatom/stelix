import { defineStore } from 'pinia'
import { computed, ref } from 'vue'

/** 左侧栏面板类型 */
export type PanelKey = 'files' | 'tasks' | 'search'

/** 主题模式 */
export type ThemeMode = 'system' | 'dark' | 'light'

// ---- 本地偏好持久化（localStorage）----
const LS_THEME = 'stelix.theme'
const LS_HIDE_PROTECTED = 'stelix.hideProtected'
const LS_HIDE_FILE_EXTENSIONS = 'stelix.hideFileExtensions'
const LS_AI_VISIBLE = 'stelix.aiVisible'
const LS_SIDEBAR_COLLAPSED = 'stelix.sidebarCollapsed'
const LS_ACTIVE_PANEL = 'stelix.activePanel'
const LS_FONT_SIZE = 'stelix.fontSize'
const LS_SIDEBAR_ROW_H = 'stelix.sidebarRowHeight'
const LS_FILE_ROW_H = 'stelix.fileRowHeight'

// 行高可选范围与默认值（px）。默认值与 tokens.css 的 --sidebar-row-h / --file-row-h 保持一致。
const SIDEBAR_ROW_MIN = 20
const SIDEBAR_ROW_MAX = 32
const SIDEBAR_ROW_DEF = 24
const FILE_ROW_MIN = 22
const FILE_ROW_MAX = 40
const FILE_ROW_DEF = 26

function readFontSize(): number {
  try {
    const saved = Number(localStorage.getItem(LS_FONT_SIZE))
    return Number.isInteger(saved) && saved >= 11 && saved <= 16 ? saved : 12
  } catch {
    return 12
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

function readPanel(): PanelKey {
  try {
    const saved = localStorage.getItem(LS_ACTIVE_PANEL)
    return saved === 'tasks' || saved === 'search' ? saved : 'files'
  } catch {
    return 'files'
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

function applyRowHeights(sidebar: number, file: number) {
  const style = document.documentElement.style
  style.setProperty('--sidebar-row-h', `${sidebar}px`)
  style.setProperty('--file-row-h', `${file}px`)
}

/**
 * UI 全局状态 —— 由 **WS-C (feat/ui-shell)** 实现。
 * ⚠️ App.vue（Foundation 拥有）依赖以下字段名，WS-C 不得改名：
 * `sidebarCollapsed` / `aiVisible`
 */
export const useUiStore = defineStore('ui', () => {
  const sidebarCollapsed = ref(readBool(LS_SIDEBAR_COLLAPSED, false))
  const aiVisible = ref(readBool(LS_AI_VISIBLE, false))
  const theme = ref<ThemeMode>(readTheme())
  const fontSize = ref(readFontSize())
  const sidebarRowHeight = ref(readRowHeight(LS_SIDEBAR_ROW_H, SIDEBAR_ROW_MIN, SIDEBAR_ROW_MAX, SIDEBAR_ROW_DEF))
  const fileRowHeight = ref(readRowHeight(LS_FILE_ROW_H, FILE_ROW_MIN, FILE_ROW_MAX, FILE_ROW_DEF))
  const resolvedTheme = computed(() => theme.value === 'system' ? systemTheme() : theme.value)
  const hideProtected = ref(readBool(LS_HIDE_PROTECTED, true))
  const hideFileExtensions = ref(readBool(LS_HIDE_FILE_EXTENSIONS, false))
  const activePanel = ref<PanelKey>(readPanel())
  const settingsOpen = ref(false)
  const statusText = ref('就绪')
  const indexTotal = ref(0)

  // 首次进入时应用已持久化的主题
  applyTheme(theme.value)
  applyFontSize(fontSize.value)
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
    }
    persistSidebar()
  }

  function persistSidebar() {
    try {
      localStorage.setItem(LS_SIDEBAR_COLLAPSED, sidebarCollapsed.value ? '1' : '0')
      localStorage.setItem(LS_ACTIVE_PANEL, activePanel.value)
    } catch {
      /* ignore */
    }
  }

  function openSettings() {
    settingsOpen.value = true
  }

  function closeSettings() {
    settingsOpen.value = false
  }

  function toggleAi() {
    aiVisible.value = !aiVisible.value
    try {
      localStorage.setItem(LS_AI_VISIBLE, aiVisible.value ? '1' : '0')
    } catch {
      /* ignore */
    }
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
    if (!Number.isInteger(size) || size < 11 || size > 16) return
    fontSize.value = size
    applyFontSize(size)
    try {
      localStorage.setItem(LS_FONT_SIZE, String(size))
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

  function displayFileName(name: string, isDir: boolean): string {
    if (!hideFileExtensions.value || isDir) return name
    const dot = name.lastIndexOf('.')
    return dot > 0 ? name.slice(0, dot) : name
  }

  function setIndexRoots(roots: string[]) {
    indexRoots.value = [...roots]
  }

  function setScanning(value: boolean) {
    scanning.value = value
  }

  function setIndexTotal(total: number) {
    indexTotal.value = total
  }

  function setScanned(value: number) {
    scanned.value = value
  }

  function setStatusText(text: string) {
    statusText.value = text
  }

  return {
    sidebarCollapsed,
    aiVisible,
    theme,
    fontSize,
    sidebarRowHeight,
    fileRowHeight,
    resolvedTheme,
    hideProtected,
    hideFileExtensions,
    activePanel,
    settingsOpen,
    statusText,
    indexTotal,
    indexRoots,
    scanning,
    scanned,
    toggleSidebar,
    setPanel,
    openSettings,
    closeSettings,
    toggleAi,
    setTheme,
    toggleTheme,
    setFontSize,
    setSidebarRowHeight,
    setFileRowHeight,
    setHideProtected,
    toggleHideProtected,
    setHideFileExtensions,
    displayFileName,
    setIndexRoots,
    setScanning,
    setIndexTotal,
    setScanned,
    setStatusText,
  }
})

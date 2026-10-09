import { defineStore } from 'pinia'
import { computed, ref } from 'vue'

/** 左侧栏面板类型 */
export type PanelKey = 'files' | 'recent' | 'search' | 'settings'

/** 主题模式 */
export type ThemeMode = 'system' | 'dark' | 'light'

// ---- 本地偏好持久化（localStorage）----
const LS_THEME = 'file-povit.theme'
const LS_HIDE_PROTECTED = 'file-povit.hideProtected'
const LS_HIDE_FILE_EXTENSIONS = 'file-povit.hideFileExtensions'
const LS_AI_VISIBLE = 'file-povit.aiVisible'
const LS_SIDEBAR_COLLAPSED = 'file-povit.sidebarCollapsed'
const LS_ACTIVE_PANEL = 'file-povit.activePanel'

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
    return saved === 'recent' || saved === 'search' || saved === 'settings' ? saved : 'files'
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

/**
 * UI 全局状态 —— 由 **WS-C (feat/ui-shell)** 实现。
 * ⚠️ App.vue（Foundation 拥有）依赖以下字段名，WS-C 不得改名：
 * `sidebarCollapsed` / `aiVisible`
 */
export const useUiStore = defineStore('ui', () => {
  const sidebarCollapsed = ref(readBool(LS_SIDEBAR_COLLAPSED, false))
  const aiVisible = ref(readBool(LS_AI_VISIBLE, false))
  const theme = ref<ThemeMode>(readTheme())
  const resolvedTheme = computed(() => theme.value === 'system' ? systemTheme() : theme.value)
  const hideProtected = ref(readBool(LS_HIDE_PROTECTED, true))
  const hideFileExtensions = ref(readBool(LS_HIDE_FILE_EXTENSIONS, false))
  const activePanel = ref<PanelKey>(readPanel())
  const statusText = ref('就绪')
  const indexTotal = ref(0)

  // 首次进入时应用已持久化的主题
  applyTheme(theme.value)
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
    resolvedTheme,
    hideProtected,
    hideFileExtensions,
    activePanel,
    statusText,
    indexTotal,
    indexRoots,
    scanning,
    scanned,
    toggleSidebar,
    setPanel,
    toggleAi,
    setTheme,
    toggleTheme,
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

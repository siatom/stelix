import { defineStore } from 'pinia'
import { ref } from 'vue'

/** 左侧栏面板类型 */
export type PanelKey = 'files' | 'quick' | 'recent' | 'settings'

/** 主题模式 */
export type ThemeMode = 'dark' | 'light'

// ---- 本地偏好持久化（localStorage）----
const LS_THEME = 'file-povit.theme'
const LS_HIDE_PROTECTED = 'file-povit.hideProtected'
const LS_AI_VISIBLE = 'file-povit.aiVisible'

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
    return localStorage.getItem(LS_THEME) === 'light' ? 'light' : 'dark'
  } catch {
    return 'dark'
  }
}

function applyTheme(mode: ThemeMode) {
  document.documentElement.setAttribute('data-theme', mode)
}

/**
 * UI 全局状态 —— 由 **WS-C (feat/ui-shell)** 实现。
 * ⚠️ App.vue（Foundation 拥有）依赖以下字段名，WS-C 不得改名：
 * `sidebarCollapsed` / `aiVisible`
 */
export const useUiStore = defineStore('ui', () => {
  const sidebarCollapsed = ref(false)
  const aiVisible = ref(readBool(LS_AI_VISIBLE, false))
  const theme = ref<ThemeMode>(readTheme())
  const hideProtected = ref(readBool(LS_HIDE_PROTECTED, true))
  const activePanel = ref<PanelKey>('files')
  const statusText = ref('就绪')
  const indexTotal = ref(0)

  // 首次进入时应用已持久化的主题
  applyTheme(theme.value)

  /** 索引根目录列表（由设置面板维护） */
  const indexRoots = ref<string[]>([])
  /** 是否正在扫描 / 建立索引 */
  const scanning = ref(false)
  /** 最近一次索引已扫描条目数 */
  const scanned = ref(0)

  function toggleSidebar() {
    sidebarCollapsed.value = !sidebarCollapsed.value
  }

  function setPanel(panel: PanelKey) {
    if (activePanel.value === panel && !sidebarCollapsed.value) {
      sidebarCollapsed.value = true
    } else {
      activePanel.value = panel
      sidebarCollapsed.value = false
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
    setTheme(theme.value === 'dark' ? 'light' : 'dark')
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
    hideProtected,
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
    setIndexRoots,
    setScanning,
    setIndexTotal,
    setScanned,
    setStatusText,
  }
})

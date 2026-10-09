import { defineStore } from 'pinia'
import { ref } from 'vue'

/** 左侧栏面板类型 */
export type PanelKey = 'files' | 'quick' | 'recent' | 'settings'

/**
 * UI 全局状态 —— 由 **WS-C (feat/ui-shell)** 实现。
 * ⚠️ App.vue（Foundation 拥有）依赖以下字段名，WS-C 不得改名：
 * `sidebarCollapsed` / `aiVisible`
 */
export const useUiStore = defineStore('ui', () => {
  const sidebarCollapsed = ref(false)
  const aiVisible = ref(true)
  const activePanel = ref<PanelKey>('files')
  const statusText = ref('就绪')
  const indexTotal = ref(0)

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
    activePanel,
    statusText,
    indexTotal,
    indexRoots,
    scanning,
    scanned,
    toggleSidebar,
    setPanel,
    toggleAi,
    setIndexRoots,
    setScanning,
    setIndexTotal,
    setScanned,
    setStatusText,
  }
})

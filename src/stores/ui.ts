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

  return {
    sidebarCollapsed,
    aiVisible,
    activePanel,
    statusText,
    indexTotal,
    toggleSidebar,
    setPanel,
    toggleAi,
  }
})

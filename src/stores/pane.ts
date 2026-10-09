import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { api } from '../api'
import type { FileEntry, PaneState, TabState, ViewMode } from '../types'

/**
 * 双窗格 / 多标签状态 —— 由 **WS-D (feat/ui-panes)** 实现。
 * 提供 App 全局唯一的窗格状态与导航能力（左侧栏目录树点击即调用 `navigate`）。
 */

let tabSeq = 0
function newTabId() {
  tabSeq += 1
  return `tab-${tabSeq}`
}

function makeTab(path: string): TabState {
  return {
    id: newTabId(),
    path,
    entries: [],
    loading: false,
    error: null,
    filter: '',
    sortKey: 'name',
    sortAsc: true,
    selected: [],
  }
}

function makePane(id: string, path: string): PaneState {
  const tab = makeTab(path)
  return { id, tabs: [tab], activeTabId: tab.id, viewMode: 'details' }
}

export const usePaneStore = defineStore('pane', () => {
  const home = 'C:\\'
  const panes = ref<PaneState[]>([makePane('left', home), makePane('right', home)])
  const activePaneId = ref('left')

  const activePane = computed(() => panes.value.find((p) => p.id === activePaneId.value) ?? panes.value[0])
  const activeTab = computed(() => {
    const pane = activePane.value
    return pane.tabs.find((t) => t.id === pane.activeTabId) ?? pane.tabs[0]
  })

  /** 目录树 / 命令面板调用：把路径导航到当前活动窗格的活动标签 */
  async function navigate(path: string) {
    const tab = activeTab.value
    if (!tab) return
    tab.path = path
    await load(tab)
  }

  async function load(tab: TabState) {
    tab.loading = true
    tab.error = null
    try {
      const listing = await api.listDir(tab.path)
      tab.entries = listing.entries
      tab.selected = []
    } catch (e) {
      tab.error = String(e)
      tab.entries = []
    } finally {
      tab.loading = false
    }
  }

  function addTab(paneId: string, path?: string) {
    const pane = panes.value.find((p) => p.id === paneId)
    if (!pane) return
    const tab = makeTab(path ?? activeTab.value?.path ?? home)
    pane.tabs.push(tab)
    pane.activeTabId = tab.id
  }

  function closeTab(paneId: string, tabId: string) {
    const pane = panes.value.find((p) => p.id === paneId)
    if (!pane || pane.tabs.length <= 1) return
    const idx = pane.tabs.findIndex((t) => t.id === tabId)
    pane.tabs.splice(idx, 1)
    if (pane.activeTabId === tabId) {
      pane.activeTabId = pane.tabs[Math.max(0, idx - 1)].id
    }
  }

  function setViewMode(paneId: string, mode: ViewMode) {
    const pane = panes.value.find((p) => p.id === paneId)
    if (pane) pane.viewMode = mode
  }

  function select(tab: TabState, entries: FileEntry[], additive: boolean) {
    const paths = entries.map((e) => e.path)
    tab.selected = additive ? Array.from(new Set([...tab.selected, ...paths])) : paths
  }

  return {
    panes,
    activePaneId,
    activePane,
    activeTab,
    navigate,
    load,
    addTab,
    closeTab,
    setViewMode,
    select,
  }
})

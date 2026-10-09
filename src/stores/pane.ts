import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { api } from '../api'
import type { FileEntry, FsOp, FsOpResult, PaneState, TabState, ViewMode } from '../types'
import { useUiStore } from './ui'
import { dirname } from '../components/Pane/utils'

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

interface HistoryEntry {
  stack: string[]
  index: number
}

export const usePaneStore = defineStore('pane', () => {
  const fallbackHome = 'C:\\'

  const panes = ref<PaneState[]>([makePane('left', fallbackHome), makePane('right', fallbackHome)])
  const activePaneId = ref('left')
  /** 左右窗格宽度比例（左窗格占比） */
  const splitRatio = ref(0.5)
  /** 内部剪贴板（复制 / 剪切） */
  const clipboard = ref<{ op: 'copy' | 'move'; paths: string[] } | null>(null)
  /** 每个标签的浏览历史（存于 store，避免改动冻结的 types.ts） */
  const histories = ref<Record<string, HistoryEntry>>({})
  /** 外部请求定位到某个条目（搜索结果 / AI 操作后） */
  const revealTarget = ref<{ tabId: string; path: string } | null>(null)

  let initialized = false

  const activePane = computed(() => panes.value.find((p) => p.id === activePaneId.value) ?? panes.value[0])
  const activeTab = computed(() => {
    const pane = activePane.value
    if (!pane) return undefined
    return pane.tabs.find((t) => t.id === pane.activeTabId) ?? pane.tabs[0]
  })

  // ---- 内部小工具 ----

  function paneById(id: string): PaneState | undefined {
    return panes.value.find((p) => p.id === id)
  }

  function activeTabOf(pane: PaneState): TabState | undefined {
    return pane.tabs.find((t) => t.id === pane.activeTabId) ?? pane.tabs[0]
  }

  function hist(tabId: string, path: string): HistoryEntry {
    const existing = histories.value[tabId]
    if (existing) return existing
    const entry: HistoryEntry = { stack: [path], index: 0 }
    histories.value[tabId] = entry
    return entry
  }

  function recordHistory(tabId: string, path: string) {
    const h = hist(tabId, path)
    h.stack = h.stack.slice(0, h.index + 1)
    if (h.stack[h.stack.length - 1] !== path) {
      h.stack.push(path)
      h.index = h.stack.length - 1
    }
    histories.value[tabId] = { ...h }
  }

  function setStatus(text: string) {
    useUiStore().statusText = text
  }

  // ---- 加载 / 导航 ----

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
    setStatus(tab.error ? `无法打开 ${tab.path}` : `${tab.entries.length} 项 · ${tab.path}`)
  }

  /** 目录树 / 命令面板调用：把路径导航到当前活动窗格的活动标签 */
  async function navigate(path: string) {
    await navigateTo(activePaneId.value, path)
  }

  async function navigateTo(paneId: string, path: string, tabId?: string, pushHistory = true) {
    const pane = paneById(paneId)
    if (!pane) return
    const tab = tabId ? pane.tabs.find((t) => t.id === tabId) : activeTabOf(pane)
    if (!tab) return
    tab.path = path
    if (pushHistory) recordHistory(tab.id, path)
    await load(tab)
  }

  async function goHistory(paneId: string, delta: number) {
    const pane = paneById(paneId)
    if (!pane) return
    const tab = activeTabOf(pane)
    if (!tab) return
    const h = histories.value[tab.id]
    if (!h) return
    const next = h.index + delta
    if (next < 0 || next >= h.stack.length) return
    h.index = next
    histories.value[tab.id] = { ...h }
    await navigateTo(paneId, h.stack[next], tab.id, false)
  }

  function goBack(paneId: string) {
    return goHistory(paneId, -1)
  }

  function goForward(paneId: string) {
    return goHistory(paneId, 1)
  }

  function canGoBack(paneId: string): boolean {
    const pane = paneById(paneId)
    const tab = pane ? activeTabOf(pane) : undefined
    if (!tab) return false
    const h = histories.value[tab.id]
    return !!h && h.index > 0
  }

  function canGoForward(paneId: string): boolean {
    const pane = paneById(paneId)
    const tab = pane ? activeTabOf(pane) : undefined
    if (!tab) return false
    const h = histories.value[tab.id]
    return !!h && h.index < h.stack.length - 1
  }

  async function goUp(paneId: string) {
    const pane = paneById(paneId)
    if (!pane) return
    const tab = activeTabOf(pane)
    if (!tab) return
    const parent = dirname(tab.path)
    if (!parent || parent === tab.path) return
    await navigateTo(paneId, parent)
  }

  async function refresh(paneId: string) {
    const pane = paneById(paneId)
    if (!pane) return
    const tab = activeTabOf(pane)
    if (!tab) return
    await load(tab)
  }

  /** 导航到某文件所在目录并选中它（搜索结果 / AI 定位用） */
  async function reveal(path: string) {
    const pane = activePane.value
    const tab = activeTab.value
    if (!pane || !tab) return
    const dir = dirname(path)
    if (tab.path !== dir) {
      await navigateTo(pane.id, dir, tab.id)
    }
    tab.selected = [path]
    revealTarget.value = { tabId: tab.id, path }
  }

  // ---- 标签 ----

  function addTab(paneId: string, path?: string) {
    const pane = paneById(paneId)
    if (!pane) return
    const base = path ?? activeTabOf(pane)?.path ?? fallbackHome
    const tab = makeTab(base)
    pane.tabs.push(tab)
    pane.activeTabId = tab.id
    recordHistory(tab.id, base)
    void load(tab)
  }

  function closeTab(paneId: string, tabId: string) {
    const pane = paneById(paneId)
    if (!pane || pane.tabs.length <= 1) return
    const idx = pane.tabs.findIndex((t) => t.id === tabId)
    if (idx < 0) return
    pane.tabs.splice(idx, 1)
    delete histories.value[tabId]
    if (pane.activeTabId === tabId) {
      pane.activeTabId = pane.tabs[Math.max(0, idx - 1)].id
    }
  }

  function duplicateTab(paneId: string, tabId: string) {
    const pane = paneById(paneId)
    if (!pane) return
    const srcIdx = pane.tabs.findIndex((t) => t.id === tabId)
    if (srcIdx < 0) return
    const src = pane.tabs[srcIdx]
    const tab = makeTab(src.path)
    tab.filter = src.filter
    tab.sortKey = src.sortKey
    tab.sortAsc = src.sortAsc
    pane.tabs.splice(srcIdx + 1, 0, tab)
    pane.activeTabId = tab.id
    recordHistory(tab.id, tab.path)
    void load(tab)
  }

  function setActiveTab(paneId: string, tabId: string) {
    const pane = paneById(paneId)
    if (pane && pane.tabs.some((t) => t.id === tabId)) pane.activeTabId = tabId
  }

  // ---- 视图 / 排序 / 过滤 ----

  function setViewMode(paneId: string, mode: ViewMode) {
    const pane = paneById(paneId)
    if (pane) pane.viewMode = mode
  }

  function setSplitRatio(ratio: number) {
    splitRatio.value = Math.min(0.85, Math.max(0.15, ratio))
  }

  function toggleSort(tab: TabState, key: TabState['sortKey']) {
    if (tab.sortKey === key) {
      tab.sortAsc = !tab.sortAsc
    } else {
      tab.sortKey = key
      tab.sortAsc = true
    }
  }

  /** 本地过滤 + 排序（目录始终在前） */
  function entriesFor(tab: TabState): FileEntry[] {
    const q = tab.filter.trim().toLowerCase()
    const list = q ? tab.entries.filter((e) => e.name.toLowerCase().includes(q)) : tab.entries.slice()
    const dir = tab.sortAsc ? 1 : -1
    list.sort((a, b) => {
      if (a.isDir !== b.isDir) return a.isDir ? -1 : 1
      let cmp = 0
      switch (tab.sortKey) {
        case 'size':
          cmp = a.size - b.size
          break
        case 'modified':
          cmp = a.modified - b.modified
          break
        case 'ext':
          cmp = (a.ext ?? '').localeCompare(b.ext ?? '')
          break
        default:
          cmp = a.name.localeCompare(b.name, 'zh-Hans-CN', { numeric: true, sensitivity: 'base' })
      }
      return cmp * dir
    })
    return list
  }

  // ---- 选择 ----

  function select(tab: TabState, entries: FileEntry[], additive: boolean) {
    const paths = entries.map((e) => e.path)
    tab.selected = additive ? Array.from(new Set([...tab.selected, ...paths])) : paths
  }

  function toggleSelect(tab: TabState, path: string) {
    const idx = tab.selected.indexOf(path)
    if (idx >= 0) tab.selected.splice(idx, 1)
    else tab.selected.push(path)
  }

  function clearSelection(tab: TabState) {
    tab.selected = []
  }

  // ---- 文件操作 ----

  function setClipboard(op: 'copy' | 'move', paths: string[]) {
    clipboard.value = { op, paths }
    setStatus(op === 'copy' ? `已复制 ${paths.length} 项到剪贴板` : `已剪切 ${paths.length} 项`)
  }

  async function runFs(paneId: string, op: FsOp): Promise<FsOpResult> {
    let result: FsOpResult
    try {
      result = await api.fsOperation(op)
      if (!result.ok) setStatus(`操作失败：${result.errors.join('; ')}`)
      else setStatus('操作已完成')
    } catch (e) {
      result = { ok: false, errors: [String(e)] }
      setStatus(`操作失败：${String(e)}`)
    }
    await refresh(paneId)
    return result
  }

  /** 把选中项（或指定项）复制到另一个窗格当前目录 */
  async function copyToOtherPane(paneId: string, sources?: string[]) {
    const srcPane = paneById(paneId)
    const dstPane = panes.value.find((p) => p.id !== paneId)
    if (!srcPane || !dstPane) return
    const srcTab = activeTabOf(srcPane)
    const dstTab = activeTabOf(dstPane)
    if (!srcTab || !dstTab) return
    const paths = sources && sources.length ? sources : srcTab.selected
    if (!paths.length) {
      setStatus('未选择任何条目')
      return
    }
    await runFs(paneId, { op: 'copy', sources: paths, target: dstTab.path })
    await refresh(dstPane.id)
  }

  /** 把剪贴板内容粘贴到指定目录 */
  async function pasteInto(paneId: string, targetDir: string) {
    const clip = clipboard.value
    if (!clip || !clip.paths.length) {
      setStatus('剪贴板为空')
      return
    }
    await runFs(paneId, { op: clip.op, sources: clip.paths, target: targetDir })
  }

  // ---- 初始化 ----

  async function init() {
    if (initialized) return
    initialized = true
    let start = fallbackHome
    try {
      const roots = await api.getRoots()
      if (roots.length) start = roots[0].mount
    } catch {
      // 后端不可用时退回默认路径
    }
    for (const pane of panes.value) {
      for (const tab of pane.tabs) {
        tab.path = start
        recordHistory(tab.id, start)
      }
    }
    await Promise.all(panes.value.flatMap((p) => p.tabs.map((t) => load(t))))
  }

  return {
    panes,
    activePaneId,
    activePane,
    activeTab,
    splitRatio,
    clipboard,
    revealTarget,
    navigate,
    navigateTo,
    load,
    goUp,
    goBack,
    goForward,
    canGoBack,
    canGoForward,
    refresh,
    reveal,
    addTab,
    closeTab,
    duplicateTab,
    setActiveTab,
    setViewMode,
    setSplitRatio,
    toggleSort,
    entriesFor,
    select,
    toggleSelect,
    clearSelection,
    setClipboard,
    runFs,
    copyToOtherPane,
    pasteInto,
    init,
  }
})

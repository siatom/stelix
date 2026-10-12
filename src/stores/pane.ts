import { defineStore } from 'pinia'
import { computed, ref, watch } from 'vue'
import { homeDir } from '@tauri-apps/api/path'
import { api } from '../api'
import type { DirListing, FileEntry, FsOp, FsOpResult, PaneState, RemoteLocation, SearchHit, TabState, ViewMode } from '../types'
import { useUiStore } from './ui'
import { dirname } from '../components/Pane/utils'
import { normalizePath, setWatchSource } from '../shell/dirWatch'
import { remoteDirname, resolveRemotePassword } from '../shell/remote'

/**
 * 双窗格 / 多标签状态 —— 由 **WS-D (feat/ui-panes)** 实现。
 * 提供 App 全局唯一的窗格状态与导航能力（左侧栏目录树点击即调用 `navigate`）。
 */

let tabSeq = 0
const PANE_PATHS_KEY = 'stelix.panePaths'
const PANE_LAYOUT_KEY = 'stelix.paneLayout'
/** 文件夹大小看门狗的重试间隔（毫秒）。 */
const DIR_SIZE_RETRY_MS = 800

interface SavedPaneLayout {
  activePaneId?: 'left' | 'right'
  splitRatio?: number
  singlePane?: boolean
}

function readPaneLayout(): SavedPaneLayout {
  try {
    const saved: unknown = JSON.parse(localStorage.getItem(PANE_LAYOUT_KEY) ?? '{}')
    if (!saved || typeof saved !== 'object' || Array.isArray(saved)) return {}
    const value = saved as Record<string, unknown>
    return {
      activePaneId: value.activePaneId === 'right' ? 'right' : 'left',
      splitRatio:
        typeof value.splitRatio === 'number' && Number.isFinite(value.splitRatio)
          ? Math.min(0.85, Math.max(0.15, value.splitRatio))
          : 0.5,
      singlePane: typeof value.singlePane === 'boolean' ? value.singlePane : false,
    }
  } catch {
    return {}
  }
}

interface SavedPanePaths {
  tabs: string[]
  activeTabIndex: number
  viewMode: ViewMode
}

function readPanePaths(): Record<string, SavedPanePaths> {
  try {
    const saved: unknown = JSON.parse(localStorage.getItem(PANE_PATHS_KEY) ?? '{}')
    if (!saved || typeof saved !== 'object' || Array.isArray(saved)) return {}
    const result: Record<string, SavedPanePaths> = {}
    for (const [id, value] of Object.entries(saved)) {
      if (id !== 'left' && id !== 'right') continue
      // 兼容旧版本只保存每个窗格当前目录的格式。
      if (typeof value === 'string' && value.trim()) {
        result[id] = { tabs: [value], activeTabIndex: 0, viewMode: 'details' }
        continue
      }
      if (!value || typeof value !== 'object' || Array.isArray(value)) continue
      const pane = value as Record<string, unknown>
      const tabs = Array.isArray(pane.tabs)
        ? pane.tabs.filter((path): path is string => typeof path === 'string' && !!path.trim()).slice(0, 50)
        : []
      if (!tabs.length) continue
      result[id] = {
        tabs,
        activeTabIndex:
          typeof pane.activeTabIndex === 'number' && Number.isFinite(pane.activeTabIndex)
            ? Math.min(tabs.length - 1, Math.max(0, Math.floor(pane.activeTabIndex)))
            : 0,
        viewMode:
          pane.viewMode === 'list' || pane.viewMode === 'icons' ? pane.viewMode : 'details',
      }
    }
    return result
  } catch {
    return {}
  }
}

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

/** 两次列目录结果是否一致（顺序由后端保证，逐项比较关键字段）。 */
function sameEntries(a: FileEntry[], b: FileEntry[]): boolean {
  if (a.length !== b.length) return false
  for (let i = 0; i < a.length; i += 1) {
    const x = a[i]
    const y = b[i]
    if (
      x.path !== y.path ||
      x.isDir !== y.isDir ||
      x.size !== y.size ||
      x.modified !== y.modified ||
      x.hidden !== y.hidden ||
      x.system !== y.system
    ) {
      return false
    }
  }
  return true
}

/** 浏览历史中的一处位置：普通目录只记路径，搜索结果与远程目录还需带上还原所需的信息。 */
interface HistoryItem {
  path: string
  kind?: 'files' | 'search' | 'remote'
  title?: string
  searchQuery?: string
  entries?: FileEntry[]
  remote?: RemoteLocation
}

/**
 * 是否可持久化：搜索结果（命中无法跨会话还原）与远程标签（没有凭据、也拿不到远端内容）都不写入
 * localStorage，否则恢复时会被当成普通本地目录标签（表现为重复目录 / 打开失败）。
 */
function isPersistableTab(tab: TabState): boolean {
  return tab.kind !== 'search' && tab.kind !== 'remote'
}

interface HistoryEntry {
  stack: HistoryItem[]
  index: number
}

export const usePaneStore = defineStore('pane', () => {
  let fallbackHome = 'C:\\'
  const savedLayout = readPaneLayout()

  const panes = ref<PaneState[]>([makePane('left', fallbackHome), makePane('right', fallbackHome)])
  const activePaneId = ref<string>(savedLayout.activePaneId ?? 'left')
  /** 左右窗格宽度比例（左窗格占比） */
  const splitRatio = ref(savedLayout.splitRatio ?? 0.5)
  /** 是否只显示当前活动窗格 */
  const singlePane = ref(savedLayout.singlePane ?? false)
  /** 内部剪贴板（复制 / 剪切） */
  const clipboard = ref<{ op: 'copy' | 'move'; paths: string[] } | null>(null)
  /** 每个标签的浏览历史（存于 store，避免改动冻结的 types.ts） */
  const histories = ref<Record<string, HistoryEntry>>({})
  /** 外部请求定位到某个条目（搜索结果 / AI 操作后） */
  const revealTarget = ref<{ tabId: string; path: string } | null>(null)
  /**
   * 正在行内编辑（重命名 / 新建）的标签 id，由 `FileList` 上报。
   * 这些标签在收到目录变更时跳过自动刷新，避免打断正在进行的输入。
   */
  const editingTabId = ref<string | null>(null)
  /** 文件夹递归大小缓存（path -> 字节），由详情视图按需请求后回填。 */
  const folderSizes = ref<Record<string, number>>({})
  /** 各窗格详情视图当前可见的文件夹路径（聚合后去抖请求，只算可见行）。 */
  const visibleDirs = ref<Record<string, string[]>>({})
  /** 文件列表「原地展开」：tabId → 已展开的目录路径集合。 */
  const expandedDirs = ref<Record<string, string[]>>({})
  /** 「原地展开」时懒加载的子项缓存：目录路径 → 子项列表。 */
  const dirChildren = ref<Record<string, FileEntry[]>>({})

  let initialized = false
  let pathsReady = false

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

  function savePanePaths() {
    if (!pathsReady) return
    try {
      const paths: Record<string, SavedPanePaths> = {}
      for (const pane of panes.value) {
        // 搜索 / 远程标签不持久化（见 `isPersistableTab`）。
        const fileTabs = pane.tabs.filter(isPersistableTab)
        if (!fileTabs.length) continue
        let activeTabIndex = fileTabs.findIndex((tab) => tab.id === pane.activeTabId)
        if (activeTabIndex < 0) {
          // 活动标签是搜索 / 远程标签时，退回其左侧最近的可持久化标签。
          const activePos = pane.tabs.findIndex((tab) => tab.id === pane.activeTabId)
          const before = pane.tabs
            .slice(0, activePos < 0 ? pane.tabs.length : activePos)
            .filter(isPersistableTab).length
          activeTabIndex = Math.min(Math.max(before - 1, 0), fileTabs.length - 1)
        }
        paths[pane.id] = {
          tabs: fileTabs.map((tab) => tab.path),
          activeTabIndex,
          viewMode: pane.viewMode,
        }
      }
      localStorage.setItem(PANE_PATHS_KEY, JSON.stringify(paths))
    } catch {
      /* 忽略持久化失败，与其他本地偏好保持一致 */
    }
  }

  function savePaneLayout() {
    try {
      localStorage.setItem(
        PANE_LAYOUT_KEY,
        JSON.stringify({
          activePaneId: activePaneId.value,
          splitRatio: splitRatio.value,
          singlePane: singlePane.value,
        }),
      )
    } catch {
      /* 忽略持久化失败 */
    }
  }

  // 同步保存，涵盖导航、切换/关闭标签及交换窗格，也适用于直接从托盘退出。
  watch(
    () =>
      panes.value.map((pane) => ({
        paths: pane.tabs.map((tab) => tab.path),
        activeTabId: pane.activeTabId,
        viewMode: pane.viewMode,
      })),
    savePanePaths,
    { flush: 'sync' },
  )
  watch(() => [activePaneId.value, splitRatio.value, singlePane.value], savePaneLayout, { flush: 'sync' })

  // 把「所有打开的文件标签目录」上报给目录监听注册表（搜索 / 远程标签没有本地目录语义）。
  watch(
    () =>
      panes.value
        .flatMap((pane) => pane.tabs)
        .filter(isPersistableTab)
        .map((tab) => tab.path),
    (paths) => setWatchSource('panes', paths),
    { flush: 'sync' },
  )

  // 被「原地展开」的目录也纳入监听：外部变更时刷新其缓存的子项。
  watch(
    () => Array.from(new Set(Object.values(expandedDirs.value).flat())),
    (paths) => setWatchSource('expanded', paths),
    { flush: 'sync' },
  )

  function snapshot(tab: TabState): HistoryItem {
    return {
      path: tab.path,
      kind: tab.kind,
      title: tab.title,
      searchQuery: tab.searchQuery,
      entries: tab.kind === 'search' ? tab.entries : undefined,
      remote: tab.kind === 'remote' ? tab.remote : undefined,
    }
  }

  function hist(tabId: string, tab: TabState): HistoryEntry {
    const existing = histories.value[tabId]
    if (existing) return existing
    const entry: HistoryEntry = { stack: [snapshot(tab)], index: 0 }
    histories.value[tabId] = entry
    return entry
  }

  function recordHistory(tab: TabState) {
    const h = hist(tab.id, tab)
    h.stack = h.stack.slice(0, h.index + 1)
    const last = h.stack[h.stack.length - 1]
    // 同一路径且同一类型才视为「刷新」而覆盖；路径相同但类型不同（如搜索结果的父目录
    // 恰好就是搜索标签的 path）必须新压一条，否则会覆盖掉搜索结果快照、后退回不去。
    if (last && last.path === tab.path && last.kind === tab.kind) {
      h.stack[h.stack.length - 1] = snapshot(tab)
    } else {
      h.stack.push(snapshot(tab))
    }
    h.index = h.stack.length - 1
    histories.value[tab.id] = { ...h }
  }

  /** 搜索结果标签页刷新后重建浏览历史（后退回到搜索结果时需还原最新命中） */
  function resetHistory(tab: TabState) {
    histories.value[tab.id] = { stack: [snapshot(tab)], index: 0 }
  }

  function setStatus(text: string) {
    useUiStore().statusText = text
  }

  // ---- 加载 / 导航 ----

  /** 远程标签：取（内存 / 凭据库里的）密码后列出 FTP / SFTP 目录。 */
  async function listRemoteTab(tab: TabState): Promise<DirListing> {
    const location = tab.remote
    if (!location) throw new Error('远程连接信息缺失')
    const password = await resolveRemotePassword(location.id)
    return api.listRemoteDir({ ...location, password, path: tab.path })
  }

  async function load(tab: TabState) {
    tab.loading = true
    tab.error = null
    try {
      const listing = tab.kind === 'remote' ? await listRemoteTab(tab) : await api.listDir(tab.path)
      tab.entries = listing.entries
      tab.selected = []
    } catch (e) {
      tab.error = String(e)
      tab.entries = []
    } finally {
      tab.loading = false
    }
    if (tab.error) setStatus(`无法打开目录：${tab.error}`)
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
    tab.kind = 'files'
    tab.title = undefined
    tab.searchQuery = undefined
    tab.remote = undefined
    // 进入其它目录：折叠该标签内原地展开的子树（展开态属于旧目录）。
    if (tab.path !== path) clearExpand(tab.id)
    tab.path = path
    if (pushHistory) recordHistory(tab)
    await load(tab)
  }

  /**
   * 在文件窗格打开一个远程（FTP / SFTP）目录。
   *
   * 与本地 `navigateTo` 分开：`navigateTo` 会把标签强制打回本地（`kind = 'files'`），
   * 而远程标签要在整段浏览过程中保持 `kind = 'remote'` 与连接信息。
   */
  async function openRemote(paneId: string, location: RemoteLocation, path = '/') {
    await navigateRemote(paneId, location, path)
  }

  async function navigateRemote(
    paneId: string,
    location: RemoteLocation,
    path: string,
    tabId?: string,
    pushHistory = true,
  ) {
    const pane = paneById(paneId)
    if (!pane) return
    const tab = tabId ? pane.tabs.find((t) => t.id === tabId) : activeTabOf(pane)
    if (!tab) return
    tab.kind = 'remote'
    tab.remote = location
    tab.title = location.label
    tab.searchQuery = undefined
    tab.path = path
    if (pushHistory) recordHistory(tab)
    await load(tab)
  }

  /** 把标签页还原到某个历史位置：普通目录重新加载，搜索结果 / 远程目录按其快照还原。 */
  async function restoreHistory(paneId: string, tab: TabState, item: HistoryItem) {
    if (item.kind === 'search') {
      tab.kind = 'search'
      tab.title = item.title
      tab.searchQuery = item.searchQuery
      tab.path = item.path
      tab.entries = item.entries ?? []
      tab.selected = []
      tab.loading = false
      tab.error = null
      return
    }
    if (item.kind === 'remote' && item.remote) {
      await navigateRemote(paneId, item.remote, item.path, tab.id, false)
      return
    }
    await navigateTo(paneId, item.path, tab.id, false)
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
    await restoreHistory(paneId, tab, h.stack[next])
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
    if (tab.kind === 'remote' && tab.remote) {
      const parent = remoteDirname(tab.path)
      if (parent === tab.path) return
      await navigateRemote(paneId, tab.remote, parent)
      return
    }
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
    // 手动刷新是「深层变更」唯一确定的观测时机（notify 只监听被打开目录的直接子项，
    // 深层删除不会触发事件），因此在这里让该目录子树的大小缓存失效并重算；
    // 普通导航不做此失效，避免每次进目录都重走大目录。
    // 远程目录没有本地递归统计，跳过缓存失效。
    if (!tab.error && tab.kind !== 'remote') invalidateDirSizes([tab.path])
  }

  /**
   * 静默重载：仅在内容**确实变化**时替换 entries。
   *
   * 与 `load` 的区别：不显示 loading、不清空选中、失败时静默忽略（目录可能刚被删除），
   * 供「目录变更自动刷新」使用，避免应用内操作与 watcher 事件造成重复刷新与闪烁。
   */
  async function reloadQuiet(tab: TabState) {
    if (tab.loading || tab.kind === 'search' || tab.kind === 'remote') return
    let listing: DirListing
    try {
      listing = await api.listDir(tab.path)
    } catch {
      return
    }
    if (sameEntries(tab.entries, listing.entries)) return
    tab.entries = listing.entries
    // 仅保留仍然存在的选中项，避免选中已删除的条目。
    const alive = new Set(listing.entries.map((entry) => entry.path))
    tab.selected = tab.selected.filter((path) => alive.has(path))
    tab.error = null
  }

  /** 某目录内容变化时，刷新**所有**打开该目录的标签（跨窗格、含后台标签）。 */
  async function reloadByPath(dir: string) {
    const target = normalizePath(dir)
    const jobs: Promise<void>[] = []
    for (const pane of panes.value) {
      for (const tab of pane.tabs) {
        if (tab.id === editingTabId.value) continue // 正在行内编辑，跳过以免打断输入
        if (tab.kind === 'remote') continue // 远程目录与本地监听无关
        if (normalizePath(tab.path) !== target) continue
        jobs.push(reloadQuiet(tab))
      }
    }
    // 该目录若正被原地展开，同步刷新其缓存的子项。
    jobs.push(refreshCachedChildren(dir))
    await Promise.all(jobs)
  }

  // ---- 文件夹大小统计（详情视图按需，仅可见行） ----

  let dirSizeTimer: ReturnType<typeof setTimeout> | null = null

  /**
   * 自愈看门狗：只要有「可见但还没拿到大小」的文件夹，就周期性地重新请求。
   *
   * 事件回填是一次性握手 —— 首次请求的结果事件若在前端注册监听之前到达
   * （启动时 `paneStore.init()` 的加载早于 `App.vue` 的监听注册）就会被丢弃，
   * 且不会自动重发，表现为大小永久留空。后端对结果有缓存、对在途有去重，
   * 重复请求的代价极低，故用轮询兜底（全部拿到后自动停止）。
   */
  let dirSizeWatchdog: ReturnType<typeof setInterval> | null = null

  /** 当前所有窗格需要统计的文件夹并集。 */
  function wantedDirs(): string[] {
    const set = new Set<string>()
    for (const paths of Object.values(visibleDirs.value)) {
      for (const path of paths) set.add(path)
    }
    return Array.from(set)
  }

  /** 可见但尚无大小的文件夹（看门狗的驱动条件）。 */
  function missingDirs(): string[] {
    return wantedDirs().filter((path) => folderSizes.value[path] === undefined)
  }

  function stopDirSizeWatchdog() {
    if (dirSizeWatchdog !== null) {
      clearInterval(dirSizeWatchdog)
      dirSizeWatchdog = null
    }
  }

  /** 依据「是否仍有缺失」启停看门狗。 */
  function syncDirSizeWatchdog() {
    if (!missingDirs().length) {
      stopDirSizeWatchdog()
      return
    }
    if (dirSizeWatchdog !== null) return
    dirSizeWatchdog = setInterval(() => {
      const paths = wantedDirs()
      if (!paths.length) {
        stopDirSizeWatchdog()
        return
      }
      void api.dirSizeRequest(paths).catch(() => {})
    }, DIR_SIZE_RETRY_MS)
  }

  /**
   * 去抖后把「当前可见的文件夹集合」整体上报后端（后端按此集合取消已滚出视口的遍历）。
   *
   * `allowEmpty=false` 时跳过空集合的上报：失效流程可能早于详情视图上报可见行，
   * 此时若把空集合发给后端会取消在途遍历、而后续又不会重新上报 → 大小永久留空。
   */
  function flushDirSizeRequest(immediate = false, allowEmpty = true) {
    if (dirSizeTimer !== null) {
      clearTimeout(dirSizeTimer)
      dirSizeTimer = null
    }
    const run = () => {
      dirSizeTimer = null
      const paths = wantedDirs()
      if (!paths.length && !allowEmpty) return
      void api.dirSizeRequest(paths).catch(() => {})
    }
    if (immediate) run()
    else dirSizeTimer = setTimeout(run, 120)
  }

  /** 某窗格详情视图上报当前可见的文件夹路径（内容未变则不重复请求）。 */
  function setVisibleDirs(paneId: string, paths: string[]) {
    const prev = visibleDirs.value[paneId]
    if (prev && prev.length === paths.length && prev.every((p, i) => p === paths[i])) return
    visibleDirs.value[paneId] = paths
    flushDirSizeRequest()
    syncDirSizeWatchdog()
  }

  /** 应用后端回填的文件夹大小。 */
  function applyDirSize(path: string, size: number) {
    folderSizes.value[path] = size
    // 还有缺失就保持看门狗运行，全部拿到后自动停止。
    syncDirSizeWatchdog()
  }

  /** 去尾部路径分隔符并小写（Windows 路径大小写不敏感）。 */
  function normPath(p: string): string {
    return p.replace(/[\\/]+$/, '').toLowerCase()
  }

  /** child 是否等于 ancestor，或位于其之下（含分隔符边界）。 */
  function underPath(child: string, ancestor: string): boolean {
    if (!child || !ancestor) return false
    if (child === ancestor) return true
    if (!child.startsWith(ancestor)) return false
    const rest = child.slice(ancestor.length)
    return rest.startsWith('\\') || rest.startsWith('/')
  }

  /**
   * 目录内容变化：失效受影响目录的大小缓存并立即重算可见项。
   *
   * 命中范围与后端 `dir_size::invalidate` 一致 —— 变化目录 D 的**自身 / 祖先 / 后代**：
   * 后代被牵连是因为监视器只上报被监听目录的子项变化，深层变更会以「被监听目录自身」
   * 的路径上报，此时其后代目录的大小同样需要重算。
   */
  function invalidateDirSizes(dirs: string[]) {
    if (!dirs.length) return
    const changed = dirs.map(normPath).filter(Boolean)
    if (!changed.length) return
    for (const key of Object.keys(folderSizes.value)) {
      const k = normPath(key)
      if (changed.some((d) => underPath(k, d) || underPath(d, k))) {
        delete folderSizes.value[key]
      }
    }
    void api
      .dirSizeInvalidate(dirs)
      .then(() => flushDirSizeRequest(true, false))
      .catch(() => {})
    // 缓存刚被清空，重新拉起看门狗直到全部重算完成。
    syncDirSizeWatchdog()
  }

  // ---- 文件列表「原地展开」（不进入目录即展开其子项） ----

  /** 某目录是否已在指定标签内展开。 */
  function isExpanded(tabId: string, path: string): boolean {
    return expandedDirs.value[tabId]?.includes(path) ?? false
  }

  /**
   * 展开 / 折叠某目录。展开时懒加载其子项（首次读取后缓存）。
   * 读取失败（权限 / 目录已被删除）时回滚展开态并提示。
   */
  async function toggleExpand(tabId: string, path: string) {
    const cur = expandedDirs.value[tabId] ?? []
    if (cur.includes(path)) {
      expandedDirs.value = { ...expandedDirs.value, [tabId]: cur.filter((p) => p !== path) }
      return
    }
    expandedDirs.value = { ...expandedDirs.value, [tabId]: [...cur, path] }
    if (dirChildren.value[path]) return
    try {
      const listing = await api.listDir(path)
      dirChildren.value = { ...dirChildren.value, [path]: listing.entries }
    } catch (e) {
      const now = expandedDirs.value[tabId] ?? []
      expandedDirs.value = { ...expandedDirs.value, [tabId]: now.filter((p) => p !== path) }
      setStatus(`无法展开：${String(e)}`)
    }
  }

  /** 折叠指定标签的全部展开项（进入其它目录 / 关闭标签时调用）。 */
  function clearExpand(tabId: string) {
    if (!expandedDirs.value[tabId]) return
    const next = { ...expandedDirs.value }
    delete next[tabId]
    expandedDirs.value = next
  }

  /**
   * 目录内容变化时刷新其已缓存的子项（原地展开的子节点）。
   * 未展开 / 未缓存过的目录直接跳过。
   */
  async function refreshCachedChildren(dir: string) {
    const target = normPath(dir)
    const key = Object.keys(dirChildren.value).find((k) => normPath(k) === target)
    if (!key) return
    try {
      const listing = await api.listDir(dir)
      dirChildren.value = { ...dirChildren.value, [key]: listing.entries }
    } catch {
      /* 目录可能刚被删除，静默忽略 */
    }
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
    const loaded = pane.tabs.find((item) => item.path === base && !item.loading && !item.error)
    if (loaded) tab.entries = loaded.entries
    pane.tabs.push(tab)
    pane.activeTabId = tab.id
    recordHistory(tab)
    // 从响应式数组重新取出代理对象；直接修改 makeTab 返回的原始对象不会触发 Vue 更新。
    if (!loaded) {
      const reactiveTab = pane.tabs.find((item) => item.id === tab.id)
      if (reactiveTab) void load(reactiveTab)
    }
  }

  /** 在活动窗格创建搜索结果标签；后续同一次输入只更新该标签。 */
  function showSearchResults(query: string, hits: SearchHit[], tabId?: string): string | undefined {
    const pane = activePane.value
    if (!pane) return undefined
    const existing = tabId ? pane.tabs.find((tab) => tab.id === tabId) : undefined
    if (existing) {
      existing.kind = 'search'
      existing.searchQuery = query
      existing.title = '搜索结果'
      existing.entries = hits.map((hit) => hit.entry)
      existing.loading = false
      existing.error = null
      existing.selected = []
      pane.activeTabId = existing.id
      // 复用的搜索结果标签页重新作为起点：重建历史，避免后退落到旧目录。
      resetHistory(existing)
      return existing.id
    }

    const tab = makeTab(activeTabOf(pane)?.path ?? fallbackHome)
    tab.kind = 'search'
    tab.searchQuery = query
    tab.title = '搜索结果'
    tab.entries = hits.map((hit) => hit.entry)
    pane.tabs.push(tab)
    pane.activeTabId = tab.id
    resetHistory(tab)
    return tab.id
  }

  function closeTab(paneId: string, tabId: string) {
    const pane = paneById(paneId)
    if (!pane || pane.tabs.length <= 1) return
    const idx = pane.tabs.findIndex((t) => t.id === tabId)
    if (idx < 0) return
    pane.tabs.splice(idx, 1)
    delete histories.value[tabId]
    clearExpand(tabId)
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
    tab.kind = src.kind
    tab.title = src.title
    tab.searchQuery = src.searchQuery
    tab.remote = src.remote
    tab.filter = src.filter
    tab.sortKey = src.sortKey
    tab.sortAsc = src.sortAsc
    if (!src.loading && !src.error) tab.entries = src.entries
    pane.tabs.splice(srcIdx + 1, 0, tab)
    pane.activeTabId = tab.id
    recordHistory(tab)
    if (src.loading || src.error) {
      const reactiveTab = pane.tabs.find((item) => item.id === tab.id)
      if (reactiveTab) void load(reactiveTab)
    }
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

  function setSinglePane(value: boolean) {
    singlePane.value = value
  }

  function toggleSort(tab: TabState, key: TabState['sortKey']) {
    if (tab.sortKey === key) {
      tab.sortAsc = !tab.sortAsc
    } else {
      tab.sortKey = key
      tab.sortAsc = key !== 'modified' && key !== 'size'
    }
  }

  /** 排序用的条目大小：文件用自身大小，文件夹用统计值（未算出按 -1，排在已知项之后）。 */
  function sortSize(entry: FileEntry): number {
    if (!entry.isDir) return entry.size
    return folderSizes.value[entry.path] ?? -1
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
          cmp = sortSize(a) - sortSize(b)
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
    void api.setFileClipboard(paths, op === 'move').catch((error) => {
      setStatus(`系统剪贴板写入失败：${String(error)}`)
    })
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

  /** 把选中项（或指定项）移动到另一个窗格当前目录 */
  async function moveToOtherPane(paneId: string, sources?: string[]) {
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
    await runFs(paneId, { op: 'move', sources: paths, target: dstTab.path })
    await refresh(dstPane.id)
  }

  /** 把当前窗格的活动目录导航到对侧窗格 */
  async function showInOtherPane(paneId: string) {
    const srcPane = paneById(paneId)
    const dstPane = panes.value.find((p) => p.id !== paneId)
    if (!srcPane || !dstPane) return
    const srcTab = activeTabOf(srcPane)
    if (!srcTab) return
    if (srcTab.kind === 'remote' && srcTab.remote) {
      await navigateRemote(dstPane.id, srcTab.remote, srcTab.path)
      return
    }
    await navigateTo(dstPane.id, srcTab.path)
  }

  /** 交换左右两个窗格的标签 / 视图状态（历史随标签移动） */
  function swapPanes() {
    const [a, b] = panes.value
    if (!a || !b) return
    const tabs = a.tabs
    const activeTabId = a.activeTabId
    const viewMode = a.viewMode
    a.tabs = b.tabs
    a.activeTabId = b.activeTabId
    a.viewMode = b.viewMode
    b.tabs = tabs
    b.activeTabId = activeTabId
    b.viewMode = viewMode
    setStatus('已交换左右窗格')
  }

  /** 把剪贴板内容粘贴到指定目录 */
  async function pasteInto(paneId: string, targetDir: string) {
    let clip: { op: 'copy' | 'move'; paths: string[] } | null = null
    try {
      const systemClip = await api.getFileClipboard()
      if (systemClip?.paths.length) {
        clip = { op: systemClip.moveFiles ? 'move' : 'copy', paths: systemClip.paths }
      }
    } catch {
      // 系统剪贴板不可读时，仍可使用应用内剪贴板。
    }
    clip ??= clipboard.value
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
    try {
      fallbackHome = await homeDir()
    } catch {
      // 系统路径不可用时退回默认路径
    }
    const savedPaths = readPanePaths()
    await Promise.all(panes.value.map(async (pane) => {
      const savedPane = savedPaths[pane.id]
      if (savedPane) {
        pane.tabs = savedPane.tabs.map((path) => makeTab(path))
        pane.activeTabId = pane.tabs[savedPane.activeTabIndex]?.id ?? pane.tabs[0].id
        pane.viewMode = savedPane.viewMode
      } else {
        pane.tabs.forEach((tab) => (tab.path = fallbackHome))
      }
      for (const tab of pane.tabs) {
        await load(tab)
        if (tab.error && tab.path !== fallbackHome) {
          tab.path = fallbackHome
          await load(tab)
        }
        recordHistory(tab)
      }
    }))
    pathsReady = true
    savePanePaths()
  }

  return {
    panes,
    activePaneId,
    activePane,
    activeTab,
    splitRatio,
    singlePane,
    clipboard,
    revealTarget,
    editingTabId,
    folderSizes,
    setVisibleDirs,
    applyDirSize,
    invalidateDirSizes,
    expandedDirs,
    dirChildren,
    isExpanded,
    toggleExpand,
    clearExpand,
    navigate,
    navigateTo,
    openRemote,
    navigateRemote,
    load,
    goUp,
    goBack,
    goForward,
    canGoBack,
    canGoForward,
    refresh,
    reloadByPath,
    reveal,
    addTab,
    showSearchResults,
    closeTab,
    duplicateTab,
    setActiveTab,
    setViewMode,
    setSplitRatio,
    setSinglePane,
    toggleSort,
    entriesFor,
    select,
    toggleSelect,
    clearSelection,
    setClipboard,
    runFs,
    copyToOtherPane,
    moveToOtherPane,
    showInOtherPane,
    swapPanes,
    pasteInto,
    init,
  }
})

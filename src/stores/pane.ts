import { defineStore } from 'pinia'
import { computed, ref, watch } from 'vue'
import { homeDir } from '@tauri-apps/api/path'
import { open as openDialog } from '@tauri-apps/plugin-dialog'
import { api } from '../api'
import type { ConflictItem, ConflictPolicy, DirListing, FileEntry, FsOp, FsOpResult, FsProgress, PaneState, RemoteLocation, RemoteTransferItem, RemoteTransferResult, SearchHit, TabState, TrashEntry, ViewMode } from '../types'
import { useUiStore } from './ui'
import { basename, dirname, joinPath } from '../components/Pane/utils'
import { matchesTypeFilter } from '../searchFilters'
import { normalizePath, setWatchSource } from '../shell/dirWatch'
import { normalizeRemotePath, remoteDirname, resolveRemoteAuth } from '../shell/remote'
import { compareNames } from '../naturalSort'
import { refreshTrashIcon } from '../components/icons/fileIconCache'
import { useTerminalStore } from './terminal'

/**
 * 双窗格 / 多标签状态 —— 由 **WS-D (feat/ui-panes)** 实现。
 * 提供 App 全局唯一的窗格状态与导航能力（左侧栏目录树点击即调用 `navigate`）。
 */

let tabSeq = 0
/** 每个标签最近一次目录加载的 token（用于丢弃过期请求的结果）。 */
const loadTokens = new Map<string, number>()
const PANE_PATHS_KEY = 'stelix.panePaths'
const PANE_LAYOUT_KEY = 'stelix.paneLayout'
/** 文件夹大小看门狗的重试间隔（毫秒）。 */
const DIR_SIZE_RETRY_MS = 800
/** 撤销 / 重做栈的深度上限。 */
const HISTORY_LIMIT = 200
/** 已结束的文件操作在队列里保留的时长（供进度面板提示「已完成 / 失败」）。 */
const FS_QUEUE_LINGER_MS = 2000

/** 目录在前、同类按名称自然排序（与文件列表保持一致）。 */
function sortEntries(entries: FileEntry[]): FileEntry[] {
  return [...entries].sort((a, b) => {
    if (a.isDir !== b.isDir) return a.isDir ? -1 : 1
    return compareNames(a.name, b.name)
  })
}

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

/** 由文件名推导扩展名（与后端 `fs_core::to_entry` 的 `Path::extension()` 同规则）。 */
function extOfName(name: string): string | null {
  const dot = name.lastIndexOf('.')
  return dot > 0 ? name.slice(dot + 1).toLowerCase() : null
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
  kind?: 'files' | 'search' | 'remote' | 'terminal' | 'trash'
  title?: string
  searchQuery?: string
  entries?: FileEntry[]
  remote?: RemoteLocation
}

/**
 * 是否可持久化：搜索结果（命中无法跨会话还原）、远程标签（没有凭据、也拿不到远端内容）
 * 与 SSH 终端（依赖活动会话）都不写入 localStorage，否则恢复时会被当成普通本地目录标签。
 */
function isPersistableTab(tab: TabState): boolean {
  return (
    tab.kind !== 'search' &&
    tab.kind !== 'remote' &&
    tab.kind !== 'terminal' &&
    tab.kind !== 'trash'
  )
}

interface HistoryEntry {
  stack: HistoryItem[]
  index: number
}

export interface FolderCreateFailure {
  /** 用户输入的名称（已 trim） */
  name: string
  /** 可直接展示的失败原因 */
  reason: string
}

export interface CreateFoldersResult {
  created: string[]
  failures: FolderCreateFailure[]
}

export interface CreateFolderAndMoveResult {
  ok: boolean
  /** 新文件夹是否已成功创建（用于区分「未创建」与「已创建但移动失败」）。 */
  created: boolean
  /** 可直接展示的失败原因。 */
  reason?: string
}

/** 剪贴板内容：本地路径与远程路径语义不同，远程来源额外记录连接信息与目录标记。 */
interface PaneClipboard {
  op: 'copy' | 'move'
  paths: string[]
  /** 来源为远程目录时有效；未设置表示本地来源 */
  remote?: RemoteLocation
  /** 远程来源的条目类型（下载目录时需要递归） */
  items?: RemoteTransferItem[]
}

/** 一次可撤销的文件操作（可能包含多条逆向 FsOp，如批量重命名）。 */
export interface UndoEntry {
  /** 展示用名称，如「重命名」「批量重命名」。 */
  label: string
  /** 撤销时执行的逆向操作（按顺序执行；为空表示只能靠回收站还原）。 */
  ops: FsOp[]
  /** 重做时执行的正向操作；缺省表示该条目不可重做。 */
  redoOps?: FsOp[]
  /** 普通删除（移到回收站）的撤销信息：撤销时按原路径从回收站还原。 */
  trash?: { paths: string[]; at: number }
}

/** 重做栈条目：正向操作 + 重做成功后回填撤销栈的原始条目。 */
interface RedoEntry {
  label: string
  ops: FsOp[]
  undo: UndoEntry
}

/** 队列内部记录：完成回调与执行选项（不进入响应式状态）。 */
interface FsJob {
  resolve: (result: FsOpResult) => void
  recordUndo: boolean
  refreshList: boolean
}

/** 文件操作队列项的状态。 */
export type FsQueueState = 'waiting' | 'running' | 'done' | 'failed' | 'cancelled'

/** 一个排队中的文件操作（同一时刻只有一个处于 running）。 */
export interface FsQueueItem {
  /** 传给后端的 opId，用于进度事件与取消。 */
  opId: string
  paneId: string
  op: FsOp
  /** 展示用名称，如「复制 3 项」。 */
  label: string
  state: FsQueueState
  /** 最近一次进度（fs:progress 回传，按 opId 匹配）。 */
  progress: FsProgress | null
}

/** 顶部标题栏可派发给活动窗格的文件操作。 */
export type PaneActionKind = 'mkdir' | 'createFile' | 'mkdirMove' | 'delete'

export const usePaneStore = defineStore('pane', () => {
  let fallbackHome = 'C:\\'
  const savedLayout = readPaneLayout()

  const panes = ref<PaneState[]>([makePane('left', fallbackHome), makePane('right', fallbackHome)])
  const activePaneId = ref<string>(savedLayout.activePaneId ?? 'left')
  /** 左右窗格宽度比例（左窗格占比） */
  const splitRatio = ref(savedLayout.splitRatio ?? 0.5)
  /** 是否只显示当前活动窗格 */
  const singlePane = ref(savedLayout.singlePane ?? false)
  /** 各窗格最近停留的本地目录（远程工作区需要把左窗格固定在本地）。 */
  const lastLocalPaths = ref<Record<string, string>>({ left: fallbackHome, right: fallbackHome })
  /** 内部剪贴板（复制 / 剪切） */
  const clipboard = ref<PaneClipboard | null>(null)
  /** 撤销栈（最近操作在末尾，最多保留 200 步）。 */
  const undoStack = ref<UndoEntry[]>([])
  const canUndo = computed(() => undoStack.value.length > 0)
  const undoLabel = computed(() => undoStack.value[undoStack.value.length - 1]?.label ?? '')
  /** 重做栈（撤销后可重做；执行新的文件操作时清空）。 */
  const redoStack = ref<RedoEntry[]>([])
  const canRedo = computed(() => redoStack.value.length > 0)
  const redoLabel = computed(() => redoStack.value[redoStack.value.length - 1]?.label ?? '')
  /** 文件操作队列（FIFO；同一时刻只执行一个，其余排队）。 */
  const fsQueue = ref<FsQueueItem[]>([])
  /** 当前正在执行的文件操作（含进度）；空闲时为 null。 */
  const activeFsOp = computed<FsQueueItem | null>(
    () => fsQueue.value.find((item) => item.state === 'running') ?? null,
  )
  /** 排队等待中的操作数量。 */
  const pendingFsCount = computed(() => fsQueue.value.filter((item) => item.state === 'waiting').length)
  /** 每个标签的浏览历史（存于 store，避免改动冻结的 types.ts） */
  const histories = ref<Record<string, HistoryEntry>>({})
  /** 外部请求定位到某个条目（搜索结果 / AI 操作后） */
  const revealTarget = ref<{ tabId: string; path: string } | null>(null)
  /** 回收站内容刷新信号：本地删除后递增，打开的回收站标签据此重载。 */
  const trashRefreshSeq = ref(0)
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
  /** 队列内部记录：opId → 完成回调与执行选项。 */
  const fsJobs = new Map<string, FsJob>()
  /** 是否已有调度循环在跑（保证同一时刻只执行一个操作）。 */
  let fsDraining = false

  const activePane = computed(() => panes.value.find((p) => p.id === activePaneId.value) ?? panes.value[0])
  const activeTab = computed(() => {
    const pane = activePane.value
    if (!pane) return undefined
    return pane.tabs.find((t) => t.id === pane.activeTabId) ?? pane.tabs[0]
  })

  /**
   * 同步左右文件夹的默认目录：两侧都必须是普通本地目录。
   * 返回 reason 表示当前布局/标签不满足条件（标题栏按钮据此禁用）。
   */
  const syncDirs = computed<{ left?: string; right?: string; reason?: string }>(() => {
    if (singlePane.value) return { reason: '需要双窗格布局' }
    const leftPane = panes.value.find((pane) => pane.id === 'left')
    const rightPane = panes.value.find((pane) => pane.id === 'right')
    const leftTab = leftPane ? activeTabOf(leftPane) : undefined
    const rightTab = rightPane ? activeTabOf(rightPane) : undefined
    if (!leftTab || (leftTab.kind ?? 'files') !== 'files') {
      return { reason: '左侧窗格不是本地文件夹' }
    }
    if (!rightTab || (rightTab.kind ?? 'files') !== 'files') {
      return { reason: '右侧窗格不是本地文件夹' }
    }
    return { left: leftTab.path, right: rightTab.path }
  })

  /**
   * 顶部标题栏（App.vue）发起、目标窗格尚未消费的一次性操作。
   * 标题栏是全局唯一入口，目标固定为**当前活动窗格**，由窗格在 watcher 里消费后置空。
   */
  const pendingAction = ref<{
    paneId: string
    kind: PaneActionKind
    /** 仅 delete：true 表示跳过回收站永久删除。 */
    permanent?: boolean
    seq: number
  } | null>(null)
  let actionSeq = 0

  /** 派发一次文件操作给当前活动窗格（哪个窗格活动就控制哪个窗格）。 */
  function requestAction(kind: PaneActionKind, permanent = false) {
    actionSeq += 1
    pendingAction.value = { paneId: activePaneId.value, kind, permanent, seq: actionSeq }
  }

  /** 目标窗格处理完请求后回执，避免重复消费。 */
  function consumeAction(seq: number) {
    if (pendingAction.value?.seq === seq) pendingAction.value = null
  }

  // 搜索结果 / 远程标签 / 终端标签都没有本地「目录」写权限，禁用新建入口。
  const canCreateInActivePane = computed(() => {
    const tab = activeTab.value
    return (
      !!tab &&
      tab.kind !== 'search' &&
      tab.kind !== 'remote' &&
      tab.kind !== 'terminal' &&
      tab.kind !== 'trash'
    )
  })
  // 删除作用于选中项自身路径：搜索结果的命中项也是真实本地文件，与普通目录一样可删。
  const canDeleteInActivePane = computed(() => {
    const tab = activeTab.value
    return (
      !!tab &&
      tab.kind !== 'remote' &&
      tab.kind !== 'terminal' &&
      tab.kind !== 'trash' &&
      tab.selected.length > 0
    )
  })
  // 「移动到新建文件夹」需要同时具备本地目录写权限与选中项；搜索 / 远程 / 终端标签不可用。
  const canMoveToNewFolderInActivePane = computed(() => {
    const tab = activeTab.value
    return (
      !!tab &&
      (tab.kind ?? 'files') === 'files' &&
      tab.selected.length > 0
    )
  })

  // ---- 内部小工具 ----

  function paneById(id: string): PaneState | undefined {
    return panes.value.find((p) => p.id === id)
  }

  function activeTabOf(pane: PaneState): TabState | undefined {
    return pane.tabs.find((t) => t.id === pane.activeTabId) ?? pane.tabs[0]
  }

  /** 普通本地目录标签（搜索 / 远程标签都不可作为传输目标）。 */
  function isLocalTab(tab: TabState | undefined): tab is TabState {
    return !!tab && (tab.kind ?? 'files') === 'files'
  }

  function rememberLocalPath(paneId: string, path: string) {
    lastLocalPaths.value = { ...lastLocalPaths.value, [paneId]: path }
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
        // 必须一并读取 `kind`：`savePanePaths` 会按 `isPersistableTab` 过滤，
        // 搜索 / 远程标签转回普通目录时路径不变，仅靠 path 无法触发重新保存。
        tabs: pane.tabs.map((tab) => ({ path: tab.path, kind: tab.kind ?? 'files' })),
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

  /** 远程标签：取（内存 / 凭据库里的）密码或私钥口令后列出 FTP / SFTP 目录。 */
  async function listRemoteTab(tab: TabState): Promise<DirListing> {
    const location = tab.remote
    if (!location) throw new Error('远程连接信息缺失')
    const auth = await resolveRemoteAuth(location)
    return api.listRemoteDir({ ...location, ...auth, path: tab.path })
  }

  async function load(tab: TabState) {
    // 搜索结果的命中来自检索管道，不由目录列举产生：重列目录会把它退化成
    // 「搜索起点目录」的普通列表（`tab.path` 只是搜索发起时所在目录）。
    if (tab.kind === 'search') return
    // 终端标签没有目录语义（`path` 只是展示用的 ssh:// 地址），不参与列目录。
    if (tab.kind === 'terminal') return
    // 回收站内容由 TrashView 直接读取，不经过目录列举。
    if (tab.kind === 'trash') return
    // 每次加载领一个递增 token：先发出的请求若后返回，其结果是过期目录，必须丢弃，
    // 否则会覆盖新目录的内容并清空选中。
    const token = (loadTokens.get(tab.id) ?? 0) + 1
    loadTokens.set(tab.id, token)
    const requestPath = tab.path
    tab.loading = true
    tab.error = null
    try {
      const listing = tab.kind === 'remote' ? await listRemoteTab(tab) : await api.listDir(requestPath)
      if (loadTokens.get(tab.id) !== token) return
      tab.entries = listing.entries
      tab.selected = []
    } catch (e) {
      if (loadTokens.get(tab.id) !== token) return
      tab.error = String(e)
      tab.entries = []
    }
    // 已被更新的请求接管时不得改动 loading（由新请求负责收尾）。
    if (loadTokens.get(tab.id) !== token) return
    tab.loading = false
    if (tab.error) setStatus(`无法打开目录：${tab.error}`)
  }

  /** 目录树 / 命令面板调用：把路径导航到当前活动窗格的活动标签 */
  async function navigate(path: string) {
    await navigateTo(activePaneId.value, path)
  }

  /** 在指定窗格打开回收站；已有回收站标签时直接切过去。 */
  function openTrash(paneId: string = activePaneId.value) {
    const pane = paneById(paneId)
    if (!pane) return
    const existing = pane.tabs.find((tab) => tab.kind === 'trash')
    if (existing) {
      pane.activeTabId = existing.id
      return
    }
    const tab = makeTab('')
    tab.kind = 'trash'
    tab.title = '回收站'
    tab.path = ''
    tab.entries = []
    tab.selected = []
    pane.tabs.push(tab)
    pane.activeTabId = tab.id
    recordHistory(tab)
  }

  async function navigateTo(paneId: string, path: string, tabId?: string, pushHistory = true) {
    const pane = paneById(paneId)
    if (!pane) return
    const tab = tabId ? pane.tabs.find((t) => t.id === tabId) : activeTabOf(pane)
    if (!tab) return
    // 终端标签没有目录语义：从侧栏 / 搜索跳到本地目录时新开一个标签，避免把终端
    // 直接改造成普通目录标签（会话会被静默遗弃）。
    if (tab.kind === 'terminal') {
      addTab(paneId, path)
      return
    }
    tab.kind = 'files'
    tab.title = undefined
    tab.searchQuery = undefined
    tab.remote = undefined
    // 进入其它目录：折叠该标签内原地展开的子树（展开态属于旧目录）。
    if (tab.path !== path) clearExpand(tab.id)
    tab.path = path
    rememberLocalPath(paneId, path)
    if (pushHistory) recordHistory(tab)
    await load(tab)
  }

  /**
   * 在文件窗格打开一个远程（FTP / SFTP）目录。
   *
   * - 当前活动标签已经是同一台主机的远程目录：直接在该标签内导航；
   * - 否则进入「远程工作区」：自动展开双窗格，左本地 / 右远程，多台主机在右窗格
   *   以独立标签并存（支持同时连接多个服务器）。
   */
  async function openRemote(paneId: string, location: RemoteLocation, path = '/') {
    const pane = paneById(paneId)
    const tab = pane ? activeTabOf(pane) : undefined
    if (tab && tab.kind === 'remote' && tab.remote?.id === location.id) {
      await navigateRemote(paneId, location, path)
      return
    }
    await openRemoteWorkspace(location, path)
  }

  /** 在指定窗格新建一个远程标签（不替换现有本地 / 远程标签）。 */
  async function addRemoteTab(paneId: string, location: RemoteLocation, path = '/') {
    const pane = paneById(paneId)
    if (!pane) return
    const tab = makeTab(path)
    tab.kind = 'remote'
    tab.remote = location
    tab.title = location.label
    pane.tabs.push(tab)
    pane.activeTabId = tab.id
    recordHistory(tab)
    const reactiveTab = pane.tabs.find((item) => item.id === tab.id)
    if (reactiveTab) await load(reactiveTab)
  }

  /**
   * 远程工作区：固定「左本地 / 右远程」。
   *
   * 用户从侧栏连接主机时调用。左窗格保持当前本地目录（若左窗格原本是远程，
   * 则回到它最近一次停留的本地目录）；右窗格为远程目录，同一主机的不同目录
   * 复用标签，不同主机则新增标签，从而支持多服务器同时连接。
   */
  async function openRemoteWorkspace(location: RemoteLocation, path = '/') {
    // 活动窗格是本地目录时，以它作为左窗格的本地目录（常见于单窗格使用）。
    const activePane = panes.value.find((pane) => pane.id === activePaneId.value)
    const activeTab = activePane ? activeTabOf(activePane) : undefined
    const activeLocal = isLocalTab(activeTab) ? activeTab?.path : undefined

    const left = paneById('left')
    const right = paneById('right')
    if (!left || !right) return

    const leftTab = activeTabOf(left)
    const desiredLocal =
      activeLocal ?? lastLocalPaths.value.left ?? lastLocalPaths.value.right ?? fallbackHome
    if (!isLocalTab(leftTab)) {
      await navigateTo('left', desiredLocal)
      const restored = activeTabOf(left)
      // 最近目录可能已被删除 / 不可访问：退回用户主目录，保证左窗格始终有可用本地目录。
      if (restored?.error && desiredLocal !== fallbackHome) {
        await navigateTo('left', fallbackHome)
      }
    } else if (activeLocal && activePane?.id === 'right' && leftTab.path !== activeLocal) {
      // 用户正在右窗格浏览本地目录：把它镜像到左侧，右侧腾给远程。
      await navigateTo('left', activeLocal)
    }

    // 恢复双窗格布局（单窗格模式下无法同时看到本地与远程）。
    if (singlePane.value) singlePane.value = false

    const existing = right.tabs.find(
      (tab) => tab.kind === 'remote' && tab.remote?.id === location.id,
    )
    if (existing) {
      right.activeTabId = existing.id
      await navigateRemote('right', location, path, existing.id)
    } else {
      await addRemoteTab('right', location, path)
    }
    activePaneId.value = 'right'
    setStatus(`已连接 ${location.label || location.host}`)
  }

  /**
   * 在文件窗格内打开 SSH 终端标签。
   *
   * 终端与远程目录并列成同一个窗格里的标签：同一主机的终端标签默认复用（切换过去），
   * `newSession = true` 时强制新开一条连接（用于「复制标签」）。
   */
  async function openTerminal(paneId: string, location: RemoteLocation, newSession = false) {
    const pane = paneById(paneId)
    if (!pane) return
    if (!newSession) {
      const existing = pane.tabs.find(
        (tab) => tab.kind === 'terminal' && tab.remote?.id === location.id,
      )
      if (existing) {
        pane.activeTabId = existing.id
        activePaneId.value = pane.id
        return
      }
    }

    const terminal = useTerminalStore()
    const sessionId = terminal.open(location)
    const tab = makeTab(`ssh://${location.host}:${location.port}`)
    tab.kind = 'terminal'
    tab.remote = location
    tab.terminalId = sessionId
    tab.title = location.label || `${location.username}@${location.host}`
    pane.tabs.push(tab)
    pane.activeTabId = tab.id
    activePaneId.value = pane.id
    recordHistory(tab)
    setStatus(`SSH 终端：${location.label || location.host}`)
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
    // 终端标签保持不变：远程目录在新标签中打开（见 `openRemoteWorkspace`）。
    if (tab.kind === 'terminal') {
      await addRemoteTab(paneId, location, path)
      return
    }
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
    // 终端标签没有可还原的内容（会话不可重建），保留当前会话即可。
    if (item.kind === 'terminal') return
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
    if (item.kind === 'trash') {
      tab.kind = 'trash'
      tab.title = '回收站'
      tab.path = ''
      tab.entries = []
      tab.selected = []
      tab.loading = false
      tab.error = null
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
    if (tab.kind === 'trash') return
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

  /** 刷新左右两个窗格（同步完成后调用）。 */
  async function refreshBoth() {
    await Promise.all([refresh('left'), refresh('right')])
  }

  async function refresh(paneId: string) {
    const pane = paneById(paneId)
    if (!pane) return
    const tab = activeTabOf(pane)
    if (!tab) return
    if (tab.kind === 'trash') {
      trashRefreshSeq.value += 1
      refreshTrashIcon()
      return
    }
    // 已有内容的目录走静默重列：`load` 会把 `loading` 置为 true，文件列表因此被
    // 「正在读取…」状态页替换并卸载重建，滚动位置也随之回到顶部。用户删除 / 重命名
    // 等操作后只是内容变化，没必要整页重来。首次加载、上次出错或正在加载时才走完整 load。
    const canRefreshQuietly =
      tab.kind !== 'search' && tab.entries.length > 0 && !tab.error && !tab.loading
    if (canRefreshQuietly) {
      const listed = await reloadQuiet(tab)
      if (!listed && !tab.loading) await load(tab)
    } else {
      await load(tab)
    }
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
   *
   * @returns 是否成功读取目录；`false` 时调用方可退回 `load` 以展示错误状态。
   */
  async function reloadQuiet(tab: TabState): Promise<boolean> {
    if (
      tab.loading ||
      tab.kind === 'search' ||
      tab.kind === 'remote' ||
      tab.kind === 'trash'
    ) {
      return false
    }
    let listing: DirListing
    try {
      listing = await api.listDir(tab.path)
    } catch {
      return false
    }
    if (sameEntries(tab.entries, listing.entries)) return true
    tab.entries = listing.entries
    // 仅保留仍然存在的选中项，避免选中已删除的条目。
    const alive = new Set(listing.entries.map((entry) => entry.path))
    tab.selected = tab.selected.filter((path) => alive.has(path))
    tab.error = null
    return true
  }

  /** 某目录内容变化时，刷新**所有**打开该目录的标签（跨窗格、含后台标签）。 */
  async function reloadByPath(dir: string) {
    const target = normalizePath(dir)
    const jobs: Promise<unknown>[] = []
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

  /**
   * 文件操作后的刷新：普通目录重列目录；搜索结果标签没有目录语义（`tab.path` 只是搜索
   * 发起时所在目录），只按「实际执行成功」的部分就地修补命中列表，否则会把结果列表整片
   * 换成那个目录。
   *
   * `applied` 为 null 表示没有任何来源被执行（失败 / 用户取消提权），此时保持搜索结果原样。
   */
  async function refreshAfterOp(paneId: string, applied: FsOp | null) {
    const pane = paneById(paneId)
    const tab = pane ? activeTabOf(pane) : undefined
    if (tab && tab.kind === 'search') {
      if (applied) patchSearchHits(tab, applied)
      return
    }
    if (tab?.kind === 'trash') return
    await refresh(paneId)
  }

  /**
   * 就地修补搜索结果标签的命中列表。
   *
   * 原地改 `tab.entries`（不换数组）：浏览历史里的快照持有的是同一个数组引用，
   * 换成新数组会让「后退 → 前进」回到搜索结果时又看到已删除的旧命中。
   */
  function patchSearchHits(tab: TabState, op: FsOp) {
    if (op.op === 'rename' && op.target && op.sources?.length === 1) {
      const from = normalizePath(op.sources[0])
      const entry = tab.entries.find((e) => normalizePath(e.path) === from)
      if (entry) {
        entry.path = op.target
        entry.name = basename(op.target)
        entry.ext = entry.isDir ? null : extOfName(entry.name)
      }
    } else if (op.op === 'delete' || op.op === 'move') {
      const gone = new Set((op.sources ?? []).map(normalizePath))
      for (let i = tab.entries.length - 1; i >= 0; i -= 1) {
        if (gone.has(normalizePath(tab.entries[i].path))) tab.entries.splice(i, 1)
      }
    }
    // 复制不改变命中项自身；新建类不会在搜索结果里发生。
    const alive = new Set(tab.entries.map((e) => e.path))
    tab.selected = tab.selected.filter((p) => alive.has(p))
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
      dirChildren.value = { ...dirChildren.value, [path]: sortEntries(listing.entries) }
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
      dirChildren.value = { ...dirChildren.value, [key]: sortEntries(listing.entries) }
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
    // 只复用普通本地目录标签：搜索结果标签的 `path` 是搜索发起目录、远程标签的 `path`
    // 是远端路径，复用会把它们的命中 / 远端列表当成该路径的本地内容。
    const loaded = pane.tabs.find(
      (item) => item.path === base && (item.kind ?? 'files') === 'files' && !item.loading && !item.error,
    )
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
  function showSearchResults(
    query: string,
    hits: SearchHit[],
    tabId?: string,
    title = '搜索结果',
  ): string | undefined {
    const pane = activePane.value
    if (!pane) return undefined
    const existing = tabId ? pane.tabs.find((tab) => tab.id === tabId) : undefined
    if (existing) {
      existing.kind = 'search'
      existing.searchQuery = query
      existing.title = title
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
    tab.title = title
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
    const closing = pane.tabs[idx]
    pane.tabs.splice(idx, 1)
    // 终端标签关闭时同步回收 SSH 会话（连接中的会话由 store 延迟回收）。
    if (closing.kind === 'terminal' && closing.terminalId) {
      useTerminalStore().close(closing.terminalId)
    }
    delete histories.value[tabId]
    loadTokens.delete(tabId)
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
    // 终端标签「复制」= 新开一条到同一主机的 SSH 会话。
    if (src.kind === 'terminal' && src.remote) {
      void openTerminal(paneId, src.remote, true)
      return
    }
    const tab = makeTab(src.path)
    tab.kind = src.kind
    tab.title = src.title
    tab.searchQuery = src.searchQuery
    tab.remote = src.remote
    tab.filter = src.filter
    tab.typeFilter = src.typeFilter
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

  /**
   * 本地过滤 + 排序（目录始终在前）
   *
   * 性能：`localeCompare(other, locale, options)` 每次调用都会重新解析 locale/options
   * 并构造比较器；上万条目的排序会因此慢一个数量级（实测 20k 条：约 1.5s vs 70ms）。
   * 这里改用模块级缓存好的 `Intl.Collator`，并在「路径」列预计算 dirname，
   * 避免比较器内为每次比较重复分配路径前缀。
   */
  function entriesFor(tab: TabState): FileEntry[] {
    const q = tab.filter.trim().toLowerCase()
    const list = tab.entries.filter(
      (e) => (!q || e.name.toLowerCase().includes(q)) && matchesTypeFilter(e, tab.typeFilter),
    )
    const dir = tab.sortAsc ? 1 : -1
    const dirOf =
      tab.sortKey === 'path' ? new Map(list.map((entry) => [entry.path, dirname(entry.path)])) : null
    list.sort((a, b) => {
      if (a.isDir !== b.isDir) return a.isDir ? -1 : 1
      let cmp = 0
      switch (tab.sortKey) {
        case 'path':
          // 按所在目录排序（同一目录内按名称），用于搜索结果页的「路径」列。
          cmp = compareNames(dirOf?.get(a.path) ?? '', dirOf?.get(b.path) ?? '')
          if (cmp === 0) cmp = compareNames(a.name, b.name)
          break
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
          cmp = compareNames(a.name, b.name)
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

  /** 对侧窗格的活动标签为普通本地目录时返回其路径。 */
  function otherPaneLocalDir(paneId: string): string | null {
    const other = panes.value.find((pane) => pane.id !== paneId)
    const tab = other ? activeTabOf(other) : undefined
    if (!tab) return null
    return (tab.kind ?? 'files') === 'files' ? tab.path : null
  }

  /** 对侧窗格的活动标签为远程目录时返回连接信息与路径。 */
  function otherPaneRemote(paneId: string): { location: RemoteLocation; path: string } | null {
    const other = panes.value.find((pane) => pane.id !== paneId)
    const tab = other ? activeTabOf(other) : undefined
    if (!tab || tab.kind !== 'remote' || !tab.remote) return null
    return { location: tab.remote, path: tab.path }
  }

  /** 刷新所有打开该远程目录的标签（跨窗格 / 后台标签）。 */
  async function refreshRemoteTabs(location: RemoteLocation, path: string) {
    const target = normalizeRemotePath(path)
    const jobs: Promise<void>[] = []
    for (const pane of panes.value) {
      for (const tab of pane.tabs) {
        if (tab.kind !== 'remote' || !tab.remote || tab.remote.id !== location.id) continue
        if (normalizeRemotePath(tab.path) !== target) continue
        jobs.push(load(tab))
      }
    }
    await Promise.all(jobs)
  }

  function transferFailureText(action: string, result: RemoteTransferResult): string {
    const failed = result.errors.length
    const detail = result.errors.slice(0, 2).join('；')
    if (result.transferred > 0) {
      return `${action}完成 ${result.transferred} 项，${failed} 项失败：${detail}`
    }
    return `${action}失败：${detail || '未知错误'}`
  }

  /** 远端 → 本地：下载文件 / 目录（目录自动递归）。 */
  async function downloadFromRemote(
    _paneId: string,
    location: RemoteLocation,
    items: RemoteTransferItem[],
    targetDir: string,
    op: 'copy' | 'move' = 'copy',
  ): Promise<RemoteTransferResult> {
    if (!items.length) {
      setStatus('未选择任何条目')
      return { ok: false, errors: ['未选择任何条目'], transferred: 0, bytes: 0 }
    }
    setStatus(`正在下载 ${items.length} 项…`)
    try {
      const auth = await resolveRemoteAuth(location)
      const result = await api.remoteTransfer({
        ...location,
        ...auth,
        direction: 'download',
        target: targetDir,
        items,
        removeSources: op === 'move',
      })
      if (result.ok) {
        setStatus(
          op === 'move'
            ? `已移动 ${result.transferred} 项到本地（远端源已删除）`
            : `已下载 ${result.transferred} 项到 ${targetDir}`,
        )
      } else {
        setStatus(transferFailureText('下载', result))
      }
      await reloadByPath(targetDir)
      return result
    } catch (error) {
      setStatus(`下载失败：${String(error)}`)
      return { ok: false, errors: [String(error)], transferred: 0, bytes: 0 }
    }
  }

  /** 本地 → 远端：上传文件 / 目录（目录自动递归，空目录也会创建）。 */
  async function uploadToRemote(
    _paneId: string,
    location: RemoteLocation,
    items: RemoteTransferItem[],
    targetDir: string,
    op: 'copy' | 'move' = 'copy',
  ): Promise<RemoteTransferResult> {
    if (!items.length) {
      setStatus('未选择任何条目')
      return { ok: false, errors: ['未选择任何条目'], transferred: 0, bytes: 0 }
    }
    setStatus(`正在上传 ${items.length} 项…`)
    try {
      const auth = await resolveRemoteAuth(location)
      const result = await api.remoteTransfer({
        ...location,
        ...auth,
        direction: 'upload',
        target: targetDir,
        items,
        removeSources: op === 'move',
      })
      if (result.ok) {
        setStatus(
          op === 'move'
            ? `已移动 ${result.transferred} 项到远程（本地源已删除）`
            : `已上传 ${result.transferred} 项到 ${targetDir}`,
        )
      } else {
        setStatus(transferFailureText('上传', result))
      }
      await refreshRemoteTabs(location, targetDir)
      return result
    } catch (error) {
      setStatus(`上传失败：${String(error)}`)
      return { ok: false, errors: [String(error)], transferred: 0, bytes: 0 }
    }
  }

  function setClipboard(
    op: 'copy' | 'move',
    paths: string[],
    remote?: RemoteLocation,
    items?: RemoteTransferItem[],
  ) {
    clipboard.value = { op, paths, remote, items }
    if (remote) {
      // 远程路径不能写进系统文件剪贴板（Explorer 不认识），只保留在应用内。
      setStatus(op === 'copy' ? `已复制 ${paths.length} 个远程项` : `已剪切 ${paths.length} 个远程项`)
      return
    }
    setStatus(op === 'copy' ? `已复制 ${paths.length} 项到剪贴板` : `已剪切 ${paths.length} 项`)
    void api.setFileClipboard(paths, op === 'move').catch((error) => {
      setStatus(`系统剪贴板写入失败：${String(error)}`)
    })
  }

  /** 由被拒绝的路径重建一个「只针对这些路径」的重试操作。 */
  function buildRetryOp(op: FsOp, denied: string[]): FsOp {
    if (op.op === 'mkdir') {
      // 批量新建：targets 里每一项都是目标路径，只重试被拒绝的目标。
      return { op: 'mkdir', targets: denied }
    }
    if (op.op === 'createFile') {
      // 单目标新建，失败路径就是 target。
      return { op: 'createFile', target: denied[0] }
    }
    // 删除 / 重命名 / 移动 / 复制：沿用原 target，只把 sources 收窄为被拒绝项，
    // 避免对已成功的项重复执行（那会误报「路径不存在」）。permanent 必须保留，
    // 否则提权重试会把「永久删除」降级成「移到回收站」。
    return { op: op.op, sources: denied, target: op.target, permanent: op.permanent }
  }

  /**
   * 从原操作中剔除被拒绝的来源，得到「首次尝试里实际已执行」的那部分。
   * 没有任何来源执行成功时返回 null。
   */
  function appliedSources(op: FsOp, denied: string[]): FsOp | null {
    if (op.op === 'mkdir') {
      // 批量新建：targets 里每一项都是目标路径，剔除被拒绝的那部分。
      const deniedSet = new Set(denied.map(normalizePath))
      const targets = (op.targets ?? []).filter((path) => !deniedSet.has(normalizePath(path)))
      return targets.length ? { op: 'mkdir', targets } : null
    }
    if (op.op === 'createFile') {
      // 单目标新建：目标被拒绝就说明什么都没做。
      return denied.length ? null : op
    }
    const deniedSet = new Set(denied.map(normalizePath))
    const sources = (op.sources ?? []).filter((path) => !deniedSet.has(normalizePath(path)))
    if (!sources.length) return null
    return { op: op.op, sources, target: op.target, permanent: op.permanent }
  }

  /** 撤销栈条目的展示名称。 */
  function opLabel(op: FsOp): string {
    switch (op.op) {
      case 'rename':
        return '重命名'
      case 'move':
        return '移动'
      case 'copy':
        return '复制'
      case 'delete':
        return op.permanent ? '永久删除' : '删除'
      case 'mkdir':
        return '新建文件夹'
      case 'createFile':
        return '新建文件'
      case 'duplicate':
        return '复制副本'
    }
  }

  function pushUndo(
    label: string,
    ops: FsOp[],
    redoOps?: FsOp[],
    trash?: { paths: string[]; at: number },
  ) {
    undoStack.value.push({ label, ops, redoOps, trash })
    if (undoStack.value.length > HISTORY_LIMIT) undoStack.value.shift()
    // 新的操作产生后，原有的重做链失效（标准撤销 / 重做语义）。
    redoStack.value = []
  }

  /** 队列 / 进度面板里的展示名称，如「复制 3 项」。 */
  function describeFsOp(op: FsOp): string {
    const count = op.op === 'mkdir' ? (op.targets?.length ?? 0) : (op.sources?.length ?? 0)
    switch (op.op) {
      case 'copy':
        return count ? `复制 ${count} 项` : '复制'
      case 'move':
        return count ? `移动 ${count} 项` : '移动'
      case 'delete':
        return `${op.permanent ? '永久删除' : '删除'}${count ? ` ${count} 项` : ''}`
      case 'duplicate':
        return count ? `复制副本 ${count} 项` : '复制副本'
      case 'mkdir':
        return count > 1 ? `新建文件夹 ${count} 个` : '新建文件夹'
      case 'createFile':
        return '新建文件'
      case 'rename':
        return '重命名'
    }
  }

  /**
   * 执行单个队列项：调后端、按需发起 UAC 提权重试、记录撤销信息并刷新列表。
   * 由 `drainFsQueue` 串行调用，保证同一时刻只有一个文件操作在跑。
   */
  async function executeFsItem(
    item: FsQueueItem,
    recordUndo: boolean,
    refreshList: boolean,
  ): Promise<FsOpResult> {
    const { paneId, op, opId } = item
    let result: FsOpResult
    /** 实际已执行的部分；null 表示没有任何来源被改动（搜索命中列表保持原样）。 */
    let applied: FsOp | null = null
    /** 首次尝试与提权重试中实际成功条目的逆向操作。 */
    let undo: FsOp[] = []
    try {
      result = await api.fsOperation(op, opId)
      undo = [...(result.undo ?? [])]
      const denied = result.denied
      if (!result.ok && denied?.length) {
        // 权限不足 → 向系统发起 UAC 提权，以管理员身份只重试失败的那部分。
        setStatus('需要管理员权限，正在请求提权…')
        try {
          const elevated = await api.fsOperationElevated(buildRetryOp(op, denied))
          result = elevated
          undo.push(...(elevated.undo ?? []))
          if (!result.ok) {
            setStatus(`操作失败：${result.errors.join('; ')}`)
            applied = appliedSources(op, denied)
          } else {
            setStatus('操作已完成（管理员权限）')
            // 未拒绝的来源在首次尝试已成功，重试部分也成功 → 整个操作已生效。
            applied = op
          }
        } catch (e) {
          const msg = String(e)
          // 首次尝试中成功的那部分已落盘；其余（被拒绝的）随取消而未执行。
          applied = appliedSources(op, denied)
          if (msg.includes('取消')) {
            // 用户主动取消提权 → 不当作错误（不弹红色错误条），仅状态栏提示。
            result = { ok: false, errors: [], denied }
            setStatus(
              applied
                ? '已取消提权：部分操作已执行，其余未执行'
                : '已取消提权，操作未执行',
            )
          } else {
            setStatus(`提权失败：${msg}`)
            result = { ok: false, errors: [msg] }
          }
        }
      } else if (!result.ok) {
        setStatus(`操作失败：${result.errors.join('; ')}`)
      } else {
        setStatus('操作已完成')
        applied = op
      }
    } catch (e) {
      result = { ok: false, errors: [String(e)] }
      setStatus(`操作失败：${String(e)}`)
    }
    // 即使后续替换了 result（提权成功 / 失败），也保留首次尝试已成功条目的撤销信息。
    result.undo = undo
    if (recordUndo) {
      // 重做时重放的正向操作：优先用「实际生效」的那部分（提权被取消时可能只是子集）。
      const forward = applied ?? (result.ok ? op : null)
      // 删除没有逆向 FsOp（可去回收站还原），但成功时也要占一个栈位：
      // 否则 Ctrl+Z 会跳过删除、误撤销更早的其它操作。
      if (op.op === 'delete' && (result.ok || applied)) {
        const deleted = forward?.sources ?? op.sources ?? []
        const redoOps: FsOp[] = forward ? [forward] : []
        if (op.permanent) pushUndo(opLabel(op), [], redoOps)
        else pushUndo(opLabel(op), [], redoOps, { paths: [...deleted], at: Date.now() })
      } else if (undo.length) {
        pushUndo(opLabel(op), undo, forward ? [forward] : [])
      }
    }
    if (refreshList) await refreshAfterOp(paneId, applied)
    // 普通删除会改变回收站内容；已打开的回收站标签据此自动刷新，侧边栏图标同步切换。
    if (applied?.op === 'delete' && !applied.permanent) {
      trashRefreshSeq.value += 1
      refreshTrashIcon()
    }
    return result
  }

  /** 队列调度：串行执行等待中的操作（同一时刻只有一个 running）。 */
  async function drainFsQueue() {
    if (fsDraining) return
    fsDraining = true
    try {
      for (;;) {
        const item = fsQueue.value.find((entry) => entry.state === 'waiting')
        if (!item) break
        item.state = 'running'
        const job = fsJobs.get(item.opId)
        let result: FsOpResult
        try {
          result = await executeFsItem(item, job?.recordUndo ?? true, job?.refreshList ?? true)
        } catch (error) {
          result = { ok: false, errors: [String(error)] }
        }
        item.state = result.cancelled ? 'cancelled' : result.ok ? 'done' : 'failed'
        fsJobs.delete(item.opId)
        job?.resolve(result)
        // 结束后保留片刻，让进度面板能显示「已完成 / 已取消 / 失败」。
        setTimeout(() => {
          fsQueue.value = fsQueue.value.filter((entry) => entry.opId !== item.opId)
        }, FS_QUEUE_LINGER_MS)
      }
    } finally {
      fsDraining = false
    }
  }

  /** 同名冲突询问：非 null 时由 ConflictDialog 渲染，用户选择后回填到 runFs。 */
  const conflictPrompt = ref<{ kind: 'copy' | 'move'; items: ConflictItem[] } | null>(null)
  /** 等待用户选择的 resolver（放在闭包里，不进入响应式状态）。 */
  let conflictResolve: ((policy: ConflictPolicy | 'cancel') => void) | null = null

  /** 让用户选择冲突处理策略；'cancel' 表示放弃整个操作。 */
  function askConflict(items: ConflictItem[], kind: 'copy' | 'move'): Promise<ConflictPolicy | 'cancel'> {
    return new Promise((resolve) => {
      conflictResolve = resolve
      conflictPrompt.value = { kind, items }
    })
  }

  /** 由冲突对话框回填用户选择（取消也走这里）。 */
  function resolveConflict(policy: ConflictPolicy | 'cancel') {
    const resolve = conflictResolve
    conflictResolve = null
    conflictPrompt.value = null
    resolve?.(policy)
  }

  /**
   * 复制 / 移动前预检同名冲突：有冲突且调用方未指定策略时先让用户选。
   * 返回 null 表示用户取消了整个操作。
   */
  async function prepareConflictPolicy(op: FsOp): Promise<FsOp | null> {
    if (op.op !== 'copy' && op.op !== 'move') return op
    if (op.conflict) return op
    let items: ConflictItem[]
    try {
      items = await api.fsCheckConflicts(op)
    } catch {
      // 预检失败不阻断操作：退化为后端的默认策略（自动改名）。
      return op
    }
    if (!items.length) return op
    const policy = await askConflict(items, op.op)
    if (policy === 'cancel') {
      setStatus('已取消：目标位置存在同名项')
      return null
    }
    return { ...op, conflict: policy }
  }

  /**
   * 文件操作的唯一入口：入队后串行执行（同一时刻只跑一个，其余 FIFO 等待）。
   * 返回的 Promise 在该任务结束时 resolve 为执行结果，语义与原先直接执行一致。
   */
  async function runFs(
    paneId: string,
    op: FsOp,
    recordUndo = true,
    refreshList = true,
  ): Promise<FsOpResult> {
    // 同名冲突必须先问用户，再入队（策略确定后执行期不再询问）。
    const prepared = await prepareConflictPolicy(op)
    if (!prepared) return { ok: false, errors: [] }
    const opId = crypto.randomUUID()
    fsQueue.value.push({
      opId,
      paneId,
      op: prepared,
      label: describeFsOp(prepared),
      state: 'waiting',
      progress: null,
    })
    return new Promise<FsOpResult>((resolve) => {
      fsJobs.set(opId, { resolve, recordUndo, refreshList })
      void drainFsQueue()
    })
  }

  /** `fs:progress` 事件回填：按 opId 匹配队列项（未命中说明任务已结束，忽略）。 */
  function applyFsProgress(progress: FsProgress) {
    const item = fsQueue.value.find((entry) => entry.opId === progress.opId)
    if (!item) return
    item.progress = progress
  }

  /** 取消当前正在执行的文件操作（已完成的条目仍然生效）。 */
  async function cancelActiveFsOp() {
    const item = fsQueue.value.find((entry) => entry.state === 'running')
    if (!item) return
    try {
      await api.fsCancel(item.opId)
    } catch (error) {
      setStatus(`取消失败：${String(error)}`)
    }
  }

  /** 一条逆向操作涉及的目录（用于撤销后刷新列表）。 */
  function opAffectedDirs(op: FsOp): string[] {
    const dirs = new Set<string>()
    const addParent = (path?: string | null) => {
      if (!path) return
      const parent = dirname(path)
      if (parent) dirs.add(parent)
    }
    for (const source of op.sources ?? []) addParent(source)
    if (op.target) {
      // Rename / CreateFile 的 target 是完整新路径；Copy / Move 的 target 是目标目录。
      if (op.op === 'rename' || op.op === 'createFile') addParent(op.target)
      else dirs.add(op.target)
    }
    return [...dirs]
  }

  /**
   * 从系统回收站还原一批路径：为每条路径挑选「原路径相同、删除时间最接近」的条目。
   * 找不到任何匹配时不执行还原，由调用方提示用户。
   */
  async function restoreFromTrash(target: { paths: string[]; at: number }): Promise<{
    ok: boolean
    restored: number
    missing: number
  }> {
    let entries: TrashEntry[]
    try {
      entries = await api.trashList()
    } catch (error) {
      setStatus(`无法读取回收站：${String(error)}`)
      return { ok: false, restored: 0, missing: target.paths.length }
    }
    const used = new Set<string>()
    const ids: string[] = []
    const restoredPaths: string[] = []
    for (const path of target.paths) {
      const key = normalizePath(path)
      let best: TrashEntry | null = null
      for (const item of entries) {
        if (used.has(item.id)) continue
        if (normalizePath(item.originalPath) !== key) continue
        if (!best || Math.abs(item.deletedAt - target.at) < Math.abs(best.deletedAt - target.at)) {
          best = item
        }
      }
      if (!best) continue
      used.add(best.id)
      ids.push(best.id)
      restoredPaths.push(path)
    }
    const missing = target.paths.length - ids.length
    if (!ids.length) {
      setStatus('回收站中已找不到该项')
      return { ok: false, restored: 0, missing }
    }
    try {
      const result = await api.trashRestore(ids)
      if (result.errors.length) setStatus(`部分还原失败：${result.errors.join('；')}`)
    } catch (error) {
      setStatus(`还原失败：${String(error)}`)
      return { ok: false, restored: 0, missing }
    }
    // 回收站内容变化 → 打开中的回收站标签与侧边栏图标同步。
    trashRefreshSeq.value += 1
    refreshTrashIcon()
    const dirs = new Set<string>()
    for (const path of restoredPaths) {
      const parent = dirname(path)
      if (parent) dirs.add(parent)
    }
    await Promise.all([...dirs].map((dir) => reloadByPath(dir)))
    return { ok: true, restored: ids.length, missing }
  }

  /** 撤销成功后把正向操作压入重做栈（没有正向操作时不入栈）。 */
  function pushRedo(entry: UndoEntry, ops: FsOp[]) {
    if (!ops.length) return
    redoStack.value.push({ label: entry.label, ops, undo: entry })
    if (redoStack.value.length > HISTORY_LIMIT) redoStack.value.shift()
  }

  /** 撤销上一步文件操作（批量操作按记录顺序逆向执行；删除走回收站还原）。 */
  async function undoLast() {
    const entry = undoStack.value.pop()
    if (!entry) {
      setStatus('没有可撤销的操作')
      return
    }
    // 普通删除没有逆向 FsOp：按删除前的路径从回收站还原。
    if (entry.trash) {
      setStatus(`正在撤销：${entry.label}`)
      const outcome = await restoreFromTrash(entry.trash)
      if (!outcome.ok) {
        setStatus(`撤销未完成：${entry.label}`)
        return
      }
      // 重做 = 再次删除同一批路径（仍走回收站）。
      pushRedo(entry, [{ op: 'delete', sources: [...entry.trash.paths] }])
      setStatus(
        outcome.missing
          ? `已撤销：${entry.label}（${outcome.missing} 项在回收站中未找到）`
          : `已撤销：${entry.label}`,
      )
      return
    }
    if (!entry.ops.length) {
      setStatus(`${entry.label}操作无法撤销`)
      return
    }
    setStatus(`正在撤销：${entry.label}`)
    let ok = true
    const dirs = new Set<string>()
    for (const op of [...entry.ops].reverse()) {
      for (const dir of opAffectedDirs(op)) dirs.add(dir)
      const result = await runFs(activePaneId.value, op, false, false)
      if (!result.ok) ok = false
    }
    // runFs 只刷新活动窗格；对侧窗格 / 多个目录也一并刷新。
    await Promise.all([...dirs].map((dir) => reloadByPath(dir)))
    if (!ok) {
      setStatus(`撤销未完成：${entry.label}`)
      return
    }
    pushRedo(entry, entry.redoOps ?? [])
    setStatus(`已撤销：${entry.label}`)
  }

  /** 重做上一次撤销的操作（按原顺序重放正向操作）。 */
  async function redoLast() {
    const entry = redoStack.value.pop()
    if (!entry) {
      setStatus('没有可重做的操作')
      return
    }
    setStatus(`正在重做：${entry.label}`)
    let ok = true
    const dirs = new Set<string>()
    for (const op of entry.ops) {
      for (const dir of opAffectedDirs(op)) dirs.add(dir)
      const result = await runFs(activePaneId.value, op, false, false)
      if (!result.ok) ok = false
    }
    await Promise.all([...dirs].map((dir) => reloadByPath(dir)))
    if (!ok) {
      setStatus(`重做未完成：${entry.label}`)
      return
    }
    // 重做成功后原始条目回到撤销栈（删除类需要刷新回收站时间戳，便于下次撤销匹配）。
    if (entry.undo.trash) entry.undo.trash = { paths: entry.undo.trash.paths, at: Date.now() }
    undoStack.value.push(entry.undo)
    if (undoStack.value.length > HISTORY_LIMIT) undoStack.value.shift()
    setStatus(`已重做：${entry.label}`)
  }

  /** 弹出目录选择器，把所选条目复制 / 移动到指定文件夹。 */
  async function copySelectedTo(paneId: string, op: 'copy' | 'move') {
    const pane = paneById(paneId)
    const tab = pane ? activeTabOf(pane) : undefined
    if (!tab || !tab.selected.length) {
      setStatus('请先选择要操作的条目')
      return
    }
    if (tab.kind === 'remote' || tab.kind === 'terminal' || tab.kind === 'trash') {
      setStatus('该位置不支持此操作')
      return
    }
    let picked: string | string[] | null
    try {
      picked = await openDialog({
        directory: true,
        multiple: false,
        title: op === 'copy' ? '复制到…' : '移动到…',
        defaultPath: tab.path,
      })
    } catch (e) {
      setStatus(`无法打开文件夹选择器：${String(e)}`)
      return
    }
    const target = typeof picked === 'string' ? picked : null
    if (!target) return
    await runFs(paneId, { op, sources: [...tab.selected], target })
  }

  /** Windows 文件名不允许的字符（含控制字符）。 */
  const INVALID_FOLDER_NAME_CHARS = /[\\/:*?"<>|\u0000-\u001f]/
  /** Windows 保留设备名，带扩展名同样不可用（如 `CON.txt`）。 */
  const RESERVED_FOLDER_NAMES = /^(con|prn|aux|nul|com[1-9]|lpt[1-9])(?:\..*)?$/i

  /** 本地预检文件夹名，返回可展示的失败原因（后端仍会做最终校验）。 */
  function folderNameError(name: string): string | null {
    if (name.length > 255) return '名称过长（最多 255 个字符）'
    if (name === '.' || name === '..') return '名称不能是 "." 或 ".."'
    if (INVALID_FOLDER_NAME_CHARS.test(name)) return '名称不能包含 \\ / : * ? " < > | 等字符'
    if (name.endsWith('.')) return '名称不能以句点结尾'
    if (RESERVED_FOLDER_NAMES.test(name)) return '这是 Windows 保留名称'
    return null
  }

  /** 把后端错误收敛为针对单个名称的原因（去掉完整路径，避免状态栏过长）。 */
  function folderErrorReason(errors: string[], path: string, name: string): string {
    const raw = errors.join('；')
    if (raw.includes('已存在同名项')) return '已存在同名项'
    if (raw.includes('目标路径为空')) return '名称为空'
    const prefix = `新建目录失败 ${path}: `
    const detail = raw.startsWith(prefix) ? raw.slice(prefix.length) : raw
    return detail.split(path).join(name)
  }

  /** 把移动失败的后端错误收敛成可直接展示的原因。 */
  function moveErrorReason(errors: string[]): string {
    const raw = errors.join('；')
    if (!raw) return '移动未完成'
    if (raw.includes('路径不存在')) return '所选条目已不存在，可能已被移动或删除'
    if (raw.includes('拒绝访问') || raw.includes('os error 5')) return '没有权限移动所选条目'
    if (raw.includes('已拒绝把目录放入其自身子目录')) return '不能把文件夹移动到它自己的子目录'
    if (raw.includes('已拒绝操作驱动器根目录')) return '不能移动驱动器根目录'
    return raw
  }

  /** 状态栏里的失败摘要：逐条展示前几项，其余折叠为数量。 */
  function failureSummary(failures: FolderCreateFailure[]): string {
    const shown = failures
      .slice(0, 2)
      .map((item) => `${item.name}（${item.reason}）`)
      .join('；')
    return failures.length > 2 ? `${shown} 等 ${failures.length} 项` : shown
  }

  /**
   * 批量新建文件夹（名称由调用方按换行拆好）。
   *
   * 逐项创建：单项失败不阻塞其余项，并精确记录失败名称与原因；成功的照常落盘。
   * 若失败原因是权限不足，把被拒绝的目标合并成**一次**提权重试，避免连续弹多个 UAC。
   */
  async function createFolders(
    paneId: string,
    names: string[],
  ): Promise<CreateFoldersResult> {
    const created: string[] = []
    const failures: FolderCreateFailure[] = []
    const pane = paneById(paneId)
    const tab = pane ? activeTabOf(pane) : undefined
    if (!tab || tab.kind === 'remote' || tab.kind === 'search' || tab.kind === 'trash') {
      return {
        created,
        failures: names.map((name) => ({ name, reason: '当前标签不支持新建文件夹' })),
      }
    }

    const seen = new Set<string>()
    const targets: { name: string; path: string }[] = []
    for (const raw of names) {
      const name = raw.trim()
      if (!name) continue
      const invalid = folderNameError(name)
      if (invalid) {
        failures.push({ name, reason: invalid })
        continue
      }
      // Windows 文件名大小写不敏感：同一批里的 A / a 会落到同一个目录。
      const key = name.toLowerCase()
      if (seen.has(key)) {
        failures.push({ name, reason: '名称重复' })
        continue
      }
      seen.add(key)
      targets.push({ name, path: joinPath(tab.path, name) })
    }

    // 全部被本地预检拦下（或只有空行）：没有可执行项。
    if (!targets.length) {
      if (failures.length) setStatus(`新建失败：${failureSummary(failures)}`)
      return { created, failures }
    }

    // 逐项执行，把每个错误精确对应到名称；权限不足的目标留到提权阶段统一重试。
    const denied: { name: string; path: string }[] = []
    for (const item of targets) {
      try {
        const result = await api.fsOperation({ op: 'mkdir', targets: [item.path] })
        if (result.ok) {
          created.push(item.name)
        } else if (result.denied?.length) {
          denied.push(item)
        } else {
          failures.push({
            name: item.name,
            reason: folderErrorReason(result.errors, item.path, item.name),
          })
        }
      } catch (e) {
        failures.push({ name: item.name, reason: `操作失败：${String(e)}` })
      }
    }

    if (denied.length) {
      setStatus('需要管理员权限，正在请求提权…')
      try {
        // 合并为一次提权：所有被拒绝的目标放在 sources 里，只弹一次 UAC。
        const result = await api.fsOperationElevated({
          op: 'mkdir',
          targets: denied.map((item) => item.path),
        })
        if (result.ok) {
          for (const item of denied) created.push(item.name)
        } else {
          const fallback = result.errors.join('；') || '提权后仍未创建'
          for (const item of denied) {
            const hit = result.errors.find((error) => error.includes(item.path))
            failures.push({
              name: item.name,
              reason: hit
                ? folderErrorReason([hit], item.path, item.name)
                : fallback,
            })
          }
        }
      } catch (e) {
        const message = String(e)
        // 用户主动取消 UAC：不当作异常，但要让用户知道这些目录没有创建。
        const reason = message.includes('取消')
          ? '已取消提权（需要管理员权限）'
          : `提权失败：${message}`
        for (const item of denied) failures.push({ name: item.name, reason })
      }
    }

    if (created.length) await refresh(paneId)

    if (!failures.length) {
      setStatus(`已创建 ${created.length} 个文件夹`)
    } else if (!created.length) {
      setStatus(`新建失败：${failureSummary(failures)}`)
    } else {
      setStatus(
        `已创建 ${created.length} 个文件夹，${failures.length} 个失败：${failureSummary(failures)}`,
      )
    }
    return { created, failures }
  }

  /**
   * 新建一个文件夹，并把当前选中项移动进去。
   *
   * 创建与移动分两步执行：创建失败（名称非法 / 已存在 / 权限不足）时不动选中项；
   * 创建成功后即使移动失败，新文件夹也会保留，状态栏会说明当前进度。
   */
  async function createFolderAndMove(
    paneId: string,
    rawName: string,
  ): Promise<CreateFolderAndMoveResult> {
    const pane = paneById(paneId)
    const tab = pane ? activeTabOf(pane) : undefined
    if (!tab || (tab.kind ?? 'files') !== 'files') {
      setStatus('当前标签不支持新建文件夹')
      return { ok: false, created: false, reason: '当前标签不支持新建文件夹' }
    }

    const name = rawName.trim()
    if (!name) {
      setStatus('新建失败：名称为空')
      return { ok: false, created: false, reason: '名称为空' }
    }
    const invalid = folderNameError(name)
    if (invalid) {
      setStatus(`新建失败：${name}（${invalid}）`)
      return { ok: false, created: false, reason: invalid }
    }

    const sources = [...tab.selected]
    if (!sources.length) {
      setStatus('请先选择要移动的条目')
      return { ok: false, created: false, reason: '未选择任何条目' }
    }

    const target = joinPath(tab.path, name)

    /** runFs 刷新目录时会清空选中；失败后把仍然存在的来源恢复选中，便于改名重试。 */
    const restoreSelection = (paths: string[]) => {
      const currentPane = paneById(paneId)
      const currentTab = currentPane ? activeTabOf(currentPane) : undefined
      if (!currentTab || currentTab.id !== tab.id) return
      const alive = new Set(currentTab.entries.map((entry) => entry.path))
      currentTab.selected = paths.filter((path) => alive.has(path))
    }

    const create = await runFs(paneId, { op: 'mkdir', targets: [target] })
    if (!create.ok) {
      restoreSelection(sources)
      // 提权取消时 runFs 已在状态栏说明，这里只补充名称与具体原因。
      const reason = create.errors.length
        ? folderErrorReason(create.errors, target, name)
        : '新建文件夹未完成'
      if (create.errors.length) setStatus(`新建失败：${name}（${reason}）`)
      return { ok: false, created: false, reason }
    }

    const move = await runFs(paneId, { op: 'move', sources, target })
    if (!move.ok) {
      restoreSelection(sources)
      const reason = moveErrorReason(move.errors)
      setStatus(`已新建「${name}」，但移动失败：${reason}`)
      return { ok: false, created: true, reason }
    }

    setStatus(`已新建「${name}」并移动 ${sources.length} 项`)
    return { ok: true, created: true }
  }

  /** 从标签已知条目（含「原地展开」缓存）补齐每个路径的目录标记。 */
  function itemsForPaths(
    tab: TabState,
    paths: string[],
    provided?: RemoteTransferItem[],
  ): RemoteTransferItem[] {
    if (provided?.length) return provided
    const known = new Map<string, FileEntry>()
    for (const entry of tab.entries) known.set(entry.path, entry)
    for (const children of Object.values(dirChildren.value)) {
      for (const entry of children) known.set(entry.path, entry)
    }
    return paths.map((path) => ({
      path,
      isDir: known.get(path)?.isDir ?? false,
    }))
  }

  /** 把选中项（或指定项）复制到另一个窗格当前目录（跨本地 / 远程时自动上传或下载）。 */
  async function copyToOtherPane(
    paneId: string,
    sources?: string[],
    items?: RemoteTransferItem[],
  ) {
    const srcPane = paneById(paneId)
    const dstPane = panes.value.find((p) => p.id !== paneId)
    if (!srcPane || !dstPane) return
    const srcTab = activeTabOf(srcPane)
    const dstTab = activeTabOf(dstPane)
    if (!srcTab || !dstTab) return
    if ((dstTab.kind ?? 'files') === 'search') {
      setStatus('对侧窗格是搜索结果，无法作为传输目标')
      return
    }
    const paths = sources && sources.length ? sources : srcTab.selected
    if (!paths.length) {
      setStatus('未选择任何条目')
      return
    }
    const srcRemote = srcTab.kind === 'remote' ? srcTab.remote : undefined
    const dstRemote = dstTab.kind === 'remote' ? dstTab.remote : undefined
    const planned = itemsForPaths(srcTab, paths, items)
    if (srcRemote) {
      if (dstRemote) {
        setStatus('暂不支持在远程服务器之间直接传输，请先下载到本地')
        return
      }
      await downloadFromRemote(dstPane.id, srcRemote, planned, dstTab.path, 'copy')
      return
    }
    if (dstRemote) {
      await uploadToRemote(dstPane.id, dstRemote, planned, dstTab.path, 'copy')
      return
    }
    await runFs(paneId, { op: 'copy', sources: paths, target: dstTab.path })
    await refresh(dstPane.id)
  }

  /** 把选中项（或指定项）移动到另一个窗格当前目录（跨本地 / 远程时逐条成功后删源）。 */
  async function moveToOtherPane(
    paneId: string,
    sources?: string[],
    items?: RemoteTransferItem[],
  ) {
    const srcPane = paneById(paneId)
    const dstPane = panes.value.find((p) => p.id !== paneId)
    if (!srcPane || !dstPane) return
    const srcTab = activeTabOf(srcPane)
    const dstTab = activeTabOf(dstPane)
    if (!srcTab || !dstTab) return
    if ((dstTab.kind ?? 'files') === 'search') {
      setStatus('对侧窗格是搜索结果，无法作为传输目标')
      return
    }
    const paths = sources && sources.length ? sources : srcTab.selected
    if (!paths.length) {
      setStatus('未选择任何条目')
      return
    }
    const srcRemote = srcTab.kind === 'remote' ? srcTab.remote : undefined
    const dstRemote = dstTab.kind === 'remote' ? dstTab.remote : undefined
    const planned = itemsForPaths(srcTab, paths, items)
    if (srcRemote) {
      if (dstRemote) {
        setStatus('暂不支持在远程服务器之间直接传输，请先下载到本地')
        return
      }
      await downloadFromRemote(dstPane.id, srcRemote, planned, dstTab.path, 'move')
      return
    }
    if (dstRemote) {
      await uploadToRemote(dstPane.id, dstRemote, planned, dstTab.path, 'move')
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
    if (srcTab.kind === 'terminal' && srcTab.remote) {
      await openTerminal(dstPane.id, srcTab.remote)
      return
    }
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

  /** 把剪贴板内容粘贴到指定目录（本地 / 远程目标均可用）。 */
  async function pasteInto(paneId: string, targetDir: string) {
    const pane = paneById(paneId)
    const tab = pane ? activeTabOf(pane) : undefined
    const targetRemote = tab?.kind === 'remote' ? tab.remote : undefined

    let systemClip: PaneClipboard | null = null
    try {
      const value = await api.getFileClipboard()
      if (value?.paths.length) {
        systemClip = { op: value.moveFiles ? 'move' : 'copy', paths: value.paths }
      }
    } catch {
      // 系统剪贴板不可读时，仍可使用应用内剪贴板。
    }

    const internal = clipboard.value
    // 远程剪贴板无法用系统剪贴板表达，必须优先；其余情况系统剪贴板更新更及时。
    const clip = internal?.remote ? internal : (systemClip ?? internal)
    if (!clip || !clip.paths.length) {
      setStatus('剪贴板为空')
      return
    }

    if (clip.remote) {
      if (targetRemote) {
        setStatus('暂不支持在远程服务器之间直接传输，请先下载到本地')
        return
      }
      const items = clip.items?.length
        ? clip.items
        : clip.paths.map((path) => ({ path, isDir: false }))
      await downloadFromRemote(paneId, clip.remote, items, targetDir, clip.op)
      return
    }

    if (targetRemote) {
      await uploadToRemote(
        paneId,
        targetRemote,
        clip.paths.map((path) => ({ path, isDir: false })),
        targetDir,
        clip.op,
      )
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
      const local = pane.tabs.find(isLocalTab)
      if (local) rememberLocalPath(pane.id, local.path)
    }))
    pathsReady = true
    savePanePaths()
  }

  return {
    panes,
    activePaneId,
    activePane,
    activeTab,
    pendingAction,
    requestAction,
    consumeAction,
    canCreateInActivePane,
    canDeleteInActivePane,
    canMoveToNewFolderInActivePane,
    splitRatio,
    singlePane,
    clipboard,
    undoStack,
    canUndo,
    undoLabel,
    pushUndo,
    undoLast,
    redoStack,
    canRedo,
    redoLabel,
    redoLast,
    fsQueue,
    activeFsOp,
    pendingFsCount,
    applyFsProgress,
    cancelActiveFsOp,
    conflictPrompt,
    resolveConflict,
    copySelectedTo,
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
    openTrash,
    trashRefreshSeq,
    openRemote,
    openRemoteWorkspace,
    addRemoteTab,
    openTerminal,
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
    otherPaneLocalDir,
    otherPaneRemote,
    syncDirs,
    refreshBoth,
    downloadFromRemote,
    uploadToRemote,
    refreshRemoteTabs,
    runFs,
    createFolders,
    createFolderAndMove,
    copyToOtherPane,
    moveToOtherPane,
    showInOtherPane,
    swapPanes,
    pasteInto,
    init,
  }
})

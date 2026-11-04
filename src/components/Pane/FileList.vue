<script setup lang="ts">
// 文件列表：虚拟滚动 + 详细/列表/图标三视图 + 选择 + 右键菜单（WS-D）
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useVirtualizer } from '@tanstack/vue-virtual'
import { openPath } from '@tauri-apps/plugin-opener'
import { startDrag } from '@crabnebula/tauri-plugin-drag'
import type { FileEntry, FsOp, RemoteLocation, RemoteTransferItem, TabState } from '../../types'
import { usePaneStore } from '../../stores/pane'
import { useUiStore, type DetailColumnKey } from '../../stores/ui'
import { match, isCapturingShortcut } from '../../stores/shortcuts'
import { api } from '../../api'
import { basename, dirname, fileIcon, fileKind, formatSize, formatTime, joinPath, typeLabel } from './utils'
import { validateEntryName } from '../../utils/nameRules'
import Icon from '../icons/Icon.vue'
import FileIcon from '../icons/FileIcon.vue'
import BatchRenameDialog from './BatchRenameDialog.vue'
import { recordOpenedFileDirectory } from '../Sidebar/shared'

/** 由顶部标题栏发起的一次「新建」请求（FileList 处理后回发 create-handled）。 */
interface CreateRequest {
  kind: 'mkdir' | 'createFile' | 'mkdirMove'
  seq: number
  tabId: string
}

const props = defineProps<{ paneId: string; tab: TabState; createRequest?: CreateRequest | null }>()
const emit = defineEmits<{
  (e: 'delete', paths: string[], permanent?: boolean): void
  (e: 'create-handled'): void
  (e: 'error', message: string | null): void
}>()

const paneStore = usePaneStore()
const ui = useUiStore()

const ICON_CAP = 600
const DRAG_TYPE = 'application/x-stelix-items'
const DRAG_FALLBACK_TYPE = 'text/plain'
const DRAG_PREFIX = 'stelix:'
const dragOverPath = ref<string | null>(null)

const viewMode = computed(() => paneStore.panes.find((p) => p.id === props.paneId)?.viewMode ?? 'details')

// ---- 详细视图列宽（表头拖动调整，ui store 持久化） ----

/** 行 / 表头左侧内边距，计算最小表格宽度时补上，保证表头与行严格对齐。 */
const TABLE_PAD_LEFT = 8
/** 名称列自适应（未手动调整）时的最小宽度。 */
const NAME_COL_MIN = 120

const columnWidths = computed(() => ui.detailColumnWidths)
/** 详情视图的 grid 列定义：名称列在未手动调整时占满剩余空间。 */
const columnTemplate = computed(() => {
  const w = columnWidths.value
  const cols = [w.name === null ? `minmax(${NAME_COL_MIN}px, 1fr)` : `${w.name}px`]
  if (isSearch.value) cols.push(`${w.path}px`)
  cols.push(`${w.size}px`, `${w.time}px`, `${w.type}px`)
  return cols.join(' ')
})
/** 各列宽度之和：内容超出窗格时出现横向滚动，而不是挤压列宽。 */
const tableWidth = computed(() => {
  const w = columnWidths.value
  return (w.name ?? NAME_COL_MIN) + (isSearch.value ? w.path : 0) + w.size + w.time + w.type
})
const tableMinWidth = computed(() => `${tableWidth.value + TABLE_PAD_LEFT}px`)
/**
 * 列表可视区宽度（不含垂直滚动条）。
 * 表头在滚动容器之外，若直接用 100% 宽度会比行内容宽出滚动条那一截，
 * 导致右侧列错位；用实测宽度让表头与行严格对齐。
 */
const scrollPortWidth = ref(0)
const headerScrollX = ref(0)
/** 表头 / 行内新建行的实际宽度：至少铺满可视区，列宽总和超出时随内容变宽。 */
const tableDisplayWidth = computed(() =>
  Math.max(scrollPortWidth.value, tableWidth.value + TABLE_PAD_LEFT),
)
const rowStyle = computed(() =>
  viewMode.value === 'details' ? { gridTemplateColumns: columnTemplate.value } : undefined,
)
const innerStyle = computed(() =>
  viewMode.value === 'details' ? { minWidth: tableMinWidth.value } : undefined,
)
const headerStyle = computed(() =>
  viewMode.value === 'details'
    ? {
        gridTemplateColumns: columnTemplate.value,
        width: `${tableDisplayWidth.value}px`,
        transform: `translateX(-${headerScrollX.value}px)`,
      }
    : undefined,
)
/** 行内新建行与表头同步横向偏移。 */
const inlineNewStyle = computed(() =>
  viewMode.value === 'details'
    ? {
        gridTemplateColumns: columnTemplate.value,
        width: `${tableDisplayWidth.value}px`,
        transform: `translateX(-${headerScrollX.value}px)`,
      }
    : undefined,
)

/** 表格独立横向滚动时，表头跟随内容平移，保持列对齐。 */
function onListScroll() {
  const x = scrollRef.value?.scrollLeft ?? 0
  if (x !== headerScrollX.value) headerScrollX.value = x
}

interface ColumnDrag {
  key: DetailColumnKey
  startX: number
  startWidth: number
}
let columnDrag: ColumnDrag | null = null

/** 表头当前真实列宽（名称列自适应时也能拿到实际像素值）。 */
function measureColumn(key: DetailColumnKey): number {
  const el = rootRef.value?.querySelector<HTMLElement>(`.header .col[data-col="${key}"]`)
  if (el) return el.getBoundingClientRect().width
  return columnWidths.value[key] ?? NAME_COL_MIN
}

function startColumnResize(ev: MouseEvent, key: DetailColumnKey) {
  if (ev.button !== 0) return
  ev.preventDefault()
  ev.stopPropagation()
  cancelPendingRename()
  // 名称列自适应时先固定为当前像素宽度：否则它会把相邻列的增量全部吸收，
  // 拖动的分隔线不跟手（其余列宽看不出变化）。
  if (columnWidths.value.name === null) ui.setDetailColumnWidth('name', measureColumn('name'))
  columnDrag = { key, startX: ev.clientX, startWidth: measureColumn(key) }
  document.body.classList.add('col-resizing')
  window.addEventListener('mousemove', onColumnResizeMove)
  window.addEventListener('mouseup', stopColumnResize)
  // 拖到窗口外松手 / 切走窗口时收尾，避免监听器残留。
  window.addEventListener('blur', stopColumnResize)
}

function onColumnResizeMove(ev: MouseEvent) {
  if (!columnDrag) return
  ui.setDetailColumnWidth(columnDrag.key, columnDrag.startWidth + ev.clientX - columnDrag.startX)
}

function stopColumnResize() {
  if (!columnDrag) return
  columnDrag = null
  document.body.classList.remove('col-resizing')
  window.removeEventListener('mousemove', onColumnResizeMove)
  window.removeEventListener('mouseup', stopColumnResize)
  window.removeEventListener('blur', stopColumnResize)
}
/**
 * 远程（FTP / SFTP）标签：可浏览并上传 / 下载。
 *
 * 远程条目只能用矢量图标（`get_file_icon` 只认本地路径）；本地的重命名 / 删除 /
 * 终端 / 属性 / 收藏 / 递归大小等操作仍不适用于远程条目。
 */
const isRemote = computed(() => props.tab.kind === 'remote' && !!props.tab.remote)
/** 只有本地目录标签才有**目录级**本地操作（新建 / 粘贴，目标目录取 `tab.path`）。 */
const isLocal = computed(() => props.tab.kind !== 'search' && props.tab.kind !== 'remote')
/**
 * **条目级**本地操作（重命名 / 删除 / 属性 / 终端 / 收藏等）作用于条目自身路径。
 * 搜索结果里的命中项同样是真实本地文件，因此与普通目录标签一样可用。
 */
const canFileOp = computed(() => !isRemote.value)
/** 远程与本地之间可以通过复制 / 剪切 / 拖拽传输（搜索结果命中项是本地文件，也可上传）。 */
/** 搜索结果标签：命中项散落在各目录，需要「打开所在文件夹」这类定位操作。 */
const isSearch = computed(() => props.tab.kind === 'search')
// 先取 store 的过滤+排序结果，再按「显示隐藏文件 / 隐藏受保护的系统文件」剔除。
const allRows = computed(() => paneStore.entriesFor(props.tab))
/** 隐藏策略：关闭「显示隐藏文件」时隐藏全部隐藏项；开启时仍可单独隐藏系统文件。 */
function filterProtected(entries: FileEntry[]): FileEntry[] {
  return entries.filter((entry) => {
    if (entry.hidden && !ui.showHidden) return false
    if (entry.hidden && entry.system && ui.hideProtected) return false
    return true
  })
}
const rows = computed(() => filterProtected(allRows.value))

/** 展平后的展示行：条目 + 缩进层级 + 是否已展开。 */
interface TreeRow {
  entry: FileEntry
  depth: number
  expanded: boolean
}

/**
 * 把「当前标签内已展开的目录」就地展开成一维列表，供虚拟滚动渲染。
 * 子项来自 store 的懒加载缓存（点击三角时才读取该目录）。
 */
const treeRows = computed<TreeRow[]>(() => {
  const out: TreeRow[] = []
  const walk = (entries: FileEntry[], depth: number) => {
    for (const entry of entries) {
      const expanded = entry.isDir && paneStore.isExpanded(props.tab.id, entry.path)
      out.push({ entry, depth, expanded })
      if (expanded) {
        const kids = paneStore.dirChildren[entry.path]
        if (kids && kids.length) walk(filterProtected(kids), depth + 1)
      }
    }
  }
  walk(rows.value, 0)
  return out
})

const iconRows = computed(() => rows.value.slice(0, ICON_CAP))

const scrollRef = ref<HTMLElement | null>(null)
const iconsScrollRef = ref<HTMLElement | null>(null)
// 行高由设置面板控制（ui.fileRowHeight）：把它读进 computed 让虚拟滚动器在变更时重建。
const virtualizer = useVirtualizer(
  computed(() => {
    const rowH = ui.fileRowHeight
    return {
      count: treeRows.value.length,
      getScrollElement: () => scrollRef.value,
      estimateSize: () => rowH,
      overscan: 12,
    }
  }),
)

/** 当前可见的虚拟行：把虚拟项映射回树行，模板里少写一层索引。 */
const virtualRows = computed(() =>
  virtualizer.value.getVirtualItems().flatMap((vi) => {
    const row = treeRows.value[vi.index]
    return row ? [{ key: vi.key, start: vi.start, size: vi.size, index: vi.index, row }] : []
  }),
)

// 行高变更后强制重新测量，刷新已缓存的行尺寸。
watch(
  () => ui.fileRowHeight,
  () => {
    void nextTick(() => virtualizer.value.measure())
  },
)

/**
 * 把虚拟滚动器缓存的偏移量同步回滚动容器的真实 `scrollTop`。
 *
 * `.fl-scroll` 会随「加载中 / 错误 / 空目录 / 无匹配 / 图标视图」等分支被 `v-if` 卸载重建。
 * `@tanstack/virtual-core` 只在收到真实 `scroll` 事件时才更新内部偏移量，卸载时也不会清零，
 * 于是新容器（`scrollTop` 为 0）配着**上一个目录**的旧偏移量：虚拟窗口从「旧偏移 - overscan」
 * 行开始渲染，列表顶部就空出一大片。
 *
 * 更糟的是当旧偏移量超过新目录的内容高度时（大目录里滚动过再进入条目较少的目录），
 * 容器根本滚不动、永远不再发出 `scroll` 事件，第一行永远不会被渲染，空白会一直留在那里。
 * 因此这里在容器重建后主动把偏移量纠正为 0，保证列表总是从首行开始渲染。
 */
function resyncScrollOffset() {
  const el = scrollRef.value
  if (!el) return
  if (el.scrollTop !== 0) el.scrollTop = 0
  // 派发一次合成的 scroll 事件，驱动 virtualizer 重新读取真实偏移量并刷新可见区间。
  el.dispatchEvent(new Event('scroll'))
}

/** 监听列表可视区宽度，供表头与行内容对齐（垂直滚动条出现 / 消失时也会触发）。 */
let scrollPortObserver: ResizeObserver | null = null
watch(scrollRef, (el) => {
  scrollPortObserver?.disconnect()
  scrollPortObserver = null
  if (!el) return
  scrollPortWidth.value = el.clientWidth
  scrollPortObserver = new ResizeObserver(() => {
    scrollPortWidth.value = el.clientWidth
  })
  scrollPortObserver.observe(el)
  void nextTick(resyncScrollOffset)
})

let anchor = -1
/** 键盘导航的当前行下标（锚点仍用于 Shift 连选）。 */
let navCursor = -1
let lastItemClick: { path: string; at: number } | null = null

/** 判定「再次单击已选中条目」的最小间隔，低于它视为双击的一部分。 */
const RENAME_CLICK_INTERVAL_MS = 400
/**
 * 慢速单击重命名的确认窗口：需覆盖系统双击判定时长。
 * 第二次单击后先等这么久，期间收到 dblclick 就取消重命名（用户其实是在双击打开），
 * 否则才真正进入重命名。
 */
const RENAME_DBLCLICK_GUARD_MS = 600
let renameTimer: number | null = null

function cancelPendingRename() {
  if (renameTimer === null) return
  window.clearTimeout(renameTimer)
  renameTimer = null
}

interface MarqueeState {
  startX: number
  startY: number
  currentX: number
  currentY: number
  moved: boolean
  additive: boolean
  baseSelection: string[]
}

const marquee = ref<MarqueeState | null>(null)
const marqueeBox = ref({ left: 0, top: 0, width: 0, height: 0 })
let suppressBlankClick = false

// ---- 选择 ----

function onClick(ev: MouseEvent, index: number, entry: FileEntry) {
  const isPlainClick = !ev.shiftKey && !ev.ctrlKey && !ev.metaKey
  const now = Date.now()
  // 再次单击已单独选中的条目：可能是「两次单击重命名」的第二次单击，
  // 也可能是「稍后双击打开」的第一次单击。先挂起，等双击窗口过去再决定。
  const mayRename =
    isPlainClick &&
    !isRemote.value &&
    props.tab.selected.length === 1 &&
    props.tab.selected[0] === entry.path &&
    lastItemClick?.path === entry.path &&
    now - lastItemClick.at >= RENAME_CLICK_INTERVAL_MS

  cancelPendingRename()
  if (mayRename) {
    const path = entry.path
    renameTimer = window.setTimeout(() => {
      renameTimer = null
      // 等待期间选中项已变（含切换标签 / 目录）或已进入编辑，就放弃这次重命名。
      if (editing.value || props.tab.selected.length !== 1 || props.tab.selected[0] !== path) return
      lastItemClick = null
      startRename(path)
    }, RENAME_DBLCLICK_GUARD_MS)
  }

  lastItemClick = isPlainClick ? { path: entry.path, at: now } : null
  if (ev.shiftKey && anchor >= 0) {
    const a = Math.min(anchor, index)
    const b = Math.max(anchor, index)
    // 下标基准随视图不同：详情/列表用展平后的 treeRows，图标视图用 iconRows（仅顶层条目）。
    const paths =
      viewMode.value === 'icons'
        ? iconRows.value.slice(a, b + 1).map((e) => e.path)
        : treeRows.value.slice(a, b + 1).map((r) => r.entry.path)
    props.tab.selected = ev.ctrlKey || ev.metaKey ? Array.from(new Set([...props.tab.selected, ...paths])) : paths
    navCursor = index
  } else if (ev.ctrlKey || ev.metaKey) {
    paneStore.toggleSelect(props.tab, entry.path)
    anchor = index
    navCursor = index
  } else {
    props.tab.selected = [entry.path]
    anchor = index
    navCursor = index
  }
}

function clearSelection() {
  cancelPendingRename()
  paneStore.clearSelection(props.tab)
  anchor = -1
  navCursor = -1
}

// ---- 键盘导航 ----

interface NavItem {
  path: string
  entry: FileEntry
}

/** 当前视图的展示条目：详情 / 列表含展开子树，图标视图仅顶层前 ICON_CAP 项。 */
function navItems(): NavItem[] {
  return viewMode.value === 'icons'
    ? iconRows.value.map((entry) => ({ path: entry.path, entry }))
    : treeRows.value.map((row) => ({ path: row.entry.path, entry: row.entry }))
}

/** 键盘操作落点：优先上次导航 / 点击的行，其次最后一个选中项。 */
function activeNavIndex(items: NavItem[]): number {
  if (navCursor >= 0 && navCursor < items.length) return navCursor
  if (anchor >= 0 && anchor < items.length) return anchor
  if (props.tab.selected.length) {
    const last = props.tab.selected[props.tab.selected.length - 1]
    const idx = items.findIndex((item) => item.path === last)
    if (idx >= 0) return idx
  }
  return -1
}

/** 图标视图的列数（用于上下方向键按行移动）。 */
function iconColumnCount(): number {
  const grid = rootRef.value?.querySelector<HTMLElement>('.icons-grid')
  if (!grid) return 1
  const columns = getComputedStyle(grid).gridTemplateColumns
  return Math.max(1, columns.split(' ').filter(Boolean).length)
}

/** PageUp / PageDown 的步长：按当前视图一屏可容纳的条目数估算。 */
function pageStep(): number {
  if (viewMode.value === 'icons') {
    const grid = rootRef.value?.querySelector<HTMLElement>('.icons-grid')
    const cell = grid?.querySelector<HTMLElement>('.icon-cell')
    const cellHeight = cell?.getBoundingClientRect().height ?? 100
    const rows = Math.max(1, Math.floor((iconsScrollRef.value?.clientHeight ?? cellHeight) / (cellHeight + 8)))
    return Math.max(1, rows * iconColumnCount())
  }
  const viewport = scrollRef.value?.clientHeight ?? 0
  return Math.max(1, Math.floor(viewport / ui.fileRowHeight) || 1)
}

function scrollNavItemIntoView(index: number) {
  if (viewMode.value === 'icons') {
    void nextTick(() => {
      const cells = rootRef.value?.querySelectorAll<HTMLElement>('.icons-grid .icon-cell')
      cells?.[index]?.scrollIntoView({ block: 'nearest' })
    })
    return
  }
  virtualizer.value.scrollToIndex(index, { align: 'auto' })
}

function applyNavSelection(items: NavItem[], index: number, extend: boolean, current: number) {
  if (extend) {
    const base = anchor >= 0 && anchor < items.length ? anchor : current >= 0 ? current : index
    anchor = base
    const from = Math.min(base, index)
    const to = Math.max(base, index)
    props.tab.selected = items.slice(from, to + 1).map((item) => item.path)
  } else {
    anchor = index
    props.tab.selected = [items[index].path]
  }
  navCursor = index
  scrollNavItemIntoView(index)
}

function moveNavCursor(delta: number, extend: boolean) {
  const items = navItems()
  if (!items.length) return
  const current = activeNavIndex(items)
  const next =
    current < 0
      ? delta >= 0
        ? 0
        : items.length - 1
      : Math.min(items.length - 1, Math.max(0, current + delta))
  applyNavSelection(items, next, extend, current)
}

function gotoNavIndex(index: number, extend: boolean) {
  const items = navItems()
  if (!items.length) return
  const next = Math.min(items.length - 1, Math.max(0, index))
  applyNavSelection(items, next, extend, activeNavIndex(items))
}

// ---- 批量重命名 ----

const batchRenameOpen = ref(false)

/** 当前选中的条目（含原地展开的子项），供批量重命名对话框使用。 */
const batchRenameEntries = computed(() => {
  const remaining = new Set(props.tab.selected)
  const out: FileEntry[] = []
  const collect = (entry: FileEntry) => {
    if (!remaining.delete(entry.path)) return
    out.push(entry)
  }
  for (const item of navItems()) collect(item.entry)
  for (const entry of props.tab.entries) collect(entry)
  return out
})

/**
 * 应用批量重命名。
 *
 * 若某个新名称正好是另一个待改条目的原名（如互换 a/b），逐条改名会互相冲突：
 * 先全部改成同目录下的临时名，再改成目标名。撤销记录按操作顺序收集，
 * 撤销时由 store 反向执行，因此两阶段也能正确还原。
 */
async function onBatchRenameConfirm(plans: { from: string; to: string }[]) {
  batchRenameOpen.value = false
  if (!plans.length) return

  const lower = (value: string) => value.toLowerCase()
  const sourceNames = new Set(plans.map((plan) => lower(plan.from)))
  const needsTemp = plans.some((plan) => sourceNames.has(lower(plan.to)))
  const undoOps: FsOp[] = []
  let failures = 0
  ui.setStatusText(`正在重命名 ${plans.length} 项…`)

  const renameOne = async (from: string, to: string) =>
    paneStore.runFs(
      props.paneId,
      { op: 'rename', sources: [from], target: to },
      false,
      // 搜索结果没有目录语义，需要每次改名后就地修补命中项；普通目录最后统一刷新。
      isSearch.value,
    )

  if (!needsTemp) {
    for (const plan of plans) {
      const result = await renameOne(plan.from, plan.to)
      if (result.ok) undoOps.push(...(result.undo ?? []))
      else failures += 1
    }
  } else {
    const stamp = Date.now()
    const staged: { plan: { from: string; to: string }; temp: string; undo: FsOp[] }[] = []
    for (let i = 0; i < plans.length; i += 1) {
      const plan = plans[i]
      const temp = joinPath(dirname(plan.from), `.stelix-tmp-${stamp}-${i}`)
      const result = await renameOne(plan.from, temp)
      if (result.ok) staged.push({ plan, temp, undo: result.undo ?? [] })
      else failures += 1
    }
    for (const item of staged) {
      const result = await renameOne(item.temp, item.plan.to)
      if (result.ok) {
        undoOps.push(...item.undo, ...(result.undo ?? []))
      } else {
        failures += 1
        // 第二阶段失败：尽力改回原名，避免留下临时文件名；成功后丢弃第一阶段的撤销记录。
        const back = await renameOne(item.temp, item.plan.from)
        if (!back.ok) undoOps.push(...item.undo, ...(result.undo ?? []))
      }
    }
  }

  if (undoOps.length) paneStore.pushUndo('批量重命名', undoOps)
  // 所有改名完成后再统一刷新，避免两阶段改名过程中临时文件名在列表里闪现。
  const dirs = new Set(plans.map((plan) => dirname(plan.from)))
  await Promise.all([...dirs].map((dir) => paneStore.reloadByPath(dir)))
  if (!failures) {
    props.tab.selected = plans.map((plan) => plan.to)
    anchor = -1
    navCursor = -1
  }
  ui.setStatusText(
    failures
      ? `批量重命名完成，${failures} 项失败`
      : `已重命名 ${plans.length} 项`,
  )
}

/** 列表是否被模态层遮挡（设置 / 反馈 / 删除确认等）。 */
function isModalOpen(): boolean {
  return ui.settingsOpen || ui.feedbackOpen || !!document.querySelector('.mask')
}

/**
 * 处理文件列表的裸键导航（不占用可配置快捷键表）。
 * 返回 true 表示已消费该按键，调用方负责 preventDefault。
 */
function handleNavKey(ev: KeyboardEvent): boolean {
  if (isModalOpen()) return false
  const key = ev.key.toLowerCase()
  const items = navItems()

  if (key === 'enter') {
    const target = ev.target as HTMLElement | null
    // 焦点在按钮 / 菜单项上时交给控件自身处理，避免回车触发两次。
    if (target?.closest('button, a, [role="menuitem"]')) return false
    const index = activeNavIndex(items)
    const entry = index >= 0 ? items[index]?.entry : undefined
    if (!entry) return false
    cancelPendingRename()
    void openEntry(entry, true)
    return true
  }

  if (key === 'backspace') {
    void paneStore.goUp(props.paneId)
    return true
  }

  if (key === 'escape') {
    if (!props.tab.selected.length && anchor < 0) return false
    clearSelection()
    return true
  }

  if (!items.length) return false
  const extend = ev.shiftKey
  if (key === 'arrowdown') {
    moveNavCursor(viewMode.value === 'icons' ? iconColumnCount() : 1, extend)
    return true
  }
  if (key === 'arrowup') {
    moveNavCursor(viewMode.value === 'icons' ? -iconColumnCount() : -1, extend)
    return true
  }
  if (viewMode.value === 'icons' && key === 'arrowright') {
    moveNavCursor(1, extend)
    return true
  }
  if (viewMode.value === 'icons' && key === 'arrowleft') {
    moveNavCursor(-1, extend)
    return true
  }
  if (key === 'home') {
    gotoNavIndex(0, extend)
    return true
  }
  if (key === 'end') {
    gotoNavIndex(items.length - 1, extend)
    return true
  }
  if (key === 'pageup') {
    moveNavCursor(-pageStep(), extend)
    return true
  }
  if (key === 'pagedown') {
    moveNavCursor(pageStep(), extend)
    return true
  }
  return false
}

function updateMarqueeBox(ev: MouseEvent) {
  const state = marquee.value
  const root = rootRef.value
  if (!state || !root) return
  state.currentX = ev.clientX
  state.currentY = ev.clientY
  const bounds = root.getBoundingClientRect()
  const left = Math.min(state.startX, ev.clientX) - bounds.left
  const top = Math.min(state.startY, ev.clientY) - bounds.top
  const next = {
    left,
    top,
    width: Math.abs(ev.clientX - state.startX),
    height: Math.abs(ev.clientY - state.startY),
  }
  const prev = marqueeBox.value
  // 位置没变就不写回，省掉一次无用渲染。
  if (prev.left === next.left && prev.top === next.top && prev.width === next.width && prev.height === next.height) return
  marqueeBox.value = next
}

/** 两个路径列表是否完全一致（框选结果按 DOM 顺序生成，可直接逐项比较）。 */
function samePaths(a: string[], b: string[]): boolean {
  if (a.length !== b.length) return false
  for (let i = 0; i < a.length; i += 1) if (a[i] !== b[i]) return false
  return true
}

function applyMarqueeSelection() {
  const state = marquee.value
  const root = rootRef.value
  if (!state || !root || !state.moved) return

  const left = Math.min(state.startX, state.currentX)
  const top = Math.min(state.startY, state.currentY)
  const right = Math.max(state.startX, state.currentX)
  const bottom = Math.max(state.startY, state.currentY)
  const selected = Array.from(root.querySelectorAll<HTMLElement>('[data-selection-path]'))
    .filter((el) => {
      const rect = el.getBoundingClientRect()
      return rect.right >= left && rect.left <= right && rect.bottom >= top && rect.top <= bottom
    })
    .map((el) => el.dataset.selectionPath)
    .filter((path): path is string => !!path)

  const next = state.additive ? Array.from(new Set([...state.baseSelection, ...selected])) : selected
  // 选中集没变化时不要替换数组：否则每次 mousemove 都会让所有可见行重新渲染
  //（图标视图最多 600 格）。框选只在跨越行/格边界时才真正改变选中项。
  if (!samePaths(props.tab.selected, next)) props.tab.selected = next
  anchor = -1
  navCursor = -1
}

/**
 * 框选的 mousemove 合并到每帧一次。
 *
 * 一次拖拽可能产生上百个 mousemove；每个事件都要读全部可见行的
 * `getBoundingClientRect()` 并重算选中集，直接同步处理会在高刷新率屏幕 / 图标视图下
 * 持续超过帧预算。rAF 合并后每帧只做一次布局读取与选中集计算。
 */
let marqueeFrame = 0
let pendingMarqueeEvent: MouseEvent | null = null

function cancelMarqueeFrame() {
  if (marqueeFrame) {
    cancelAnimationFrame(marqueeFrame)
    marqueeFrame = 0
  }
  pendingMarqueeEvent = null
}

function scheduleMarqueeUpdate(ev: MouseEvent) {
  pendingMarqueeEvent = ev
  if (marqueeFrame) return
  marqueeFrame = requestAnimationFrame(() => {
    marqueeFrame = 0
    const pending = pendingMarqueeEvent
    pendingMarqueeEvent = null
    if (!pending || !marquee.value) return
    updateMarqueeBox(pending)
    applyMarqueeSelection()
  })
}

function stopMarquee(ev?: MouseEvent) {
  cancelMarqueeFrame()
  if (!marquee.value) return
  if (ev) {
    updateMarqueeBox(ev)
    const state = marquee.value
    state.moved = Math.abs(ev.clientX - state.startX) > 4 || Math.abs(ev.clientY - state.startY) > 4
    applyMarqueeSelection()
    suppressBlankClick = state.moved
  }
  marquee.value = null
  window.removeEventListener('mousemove', onMarqueeMove)
  window.removeEventListener('mouseup', onMarqueeUp)
}

function onMarqueeMove(ev: MouseEvent) {
  const state = marquee.value
  if (!state) return
  if (Math.abs(ev.clientX - state.startX) > 4 || Math.abs(ev.clientY - state.startY) > 4) state.moved = true
  scheduleMarqueeUpdate(ev)
}

function onMarqueeUp(ev: MouseEvent) {
  stopMarquee(ev)
}

function onRootMouseDown(ev: MouseEvent) {
  if (ev.button !== 0 || editing.value) return
  const target = ev.target as HTMLElement
  if (target.closest('.header, .row, .icon-cell, .inline-new, button, input')) return
  suppressBlankClick = false
  ev.preventDefault()
  marquee.value = {
    startX: ev.clientX,
    startY: ev.clientY,
    currentX: ev.clientX,
    currentY: ev.clientY,
    moved: false,
    additive: ev.ctrlKey || ev.metaKey,
    baseSelection: [...props.tab.selected],
  }
  marqueeBox.value = { left: 0, top: 0, width: 0, height: 0 }
  window.addEventListener('mousemove', onMarqueeMove)
  window.addEventListener('mouseup', onMarqueeUp)
}

function onBlankClick() {
  if (suppressBlankClick) {
    suppressBlankClick = false
    return
  }
  clearSelection()
}

// ---- 打开 ----

async function openEntry(entry: FileEntry, countForFrequentDirectories = false) {
  const location = props.tab.remote
  if (isRemote.value && location) {
    if (entry.isDir) {
      await paneStore.navigateRemote(props.paneId, location, entry.path)
      return
    }
    // 远程文件需要先下载到本地；通过右键菜单或复制到本地窗格完成。
    ui.statusText = '远程文件请右键 → 下载…，或复制后粘贴到本地窗格'
    return
  }
  if (entry.isDir) {
    await paneStore.navigateTo(props.paneId, entry.path)
    return
  }
  try {
    await openPath(entry.path)
    if (countForFrequentDirectories) recordOpenedFileDirectory(dirname(entry.path))
  } catch (e) {
    ui.statusText = `无法打开：${String(e)}`
  }
}

async function onDblClick(entry: FileEntry) {
  cancelPendingRename()
  await openEntry(entry, true)
}

// ---- 行内编辑（重命名 / 新建文件夹 / 新建文件） ----

interface Editing {
  mode: 'rename' | 'mkdir' | 'mkdirMove' | 'createFile'
  /** rename 时的原路径 */
  path?: string
  /** 输入框当前文本 */
  name: string
  /** 初始选中的字符范围（文件默认选中主名，目录/新建默认全选） */
  selStart: number
  selEnd: number
}

const editing = ref<Editing | null>(null)
/** 行内编辑会话序号：异步提交期间用户重新开始编辑 / 取消时，丢弃过期的回填。 */
let editSeq = 0

// 上报「正在行内编辑」的标签：store 在收到目录变更时会跳过该标签的自动刷新，避免打断输入。
watch(
  () => editing.value !== null,
  (isEditing) => {
    if (isEditing) paneStore.editingTabId = props.tab.id
    else if (paneStore.editingTabId === props.tab.id) paneStore.editingTabId = null
  },
)

// 根元素 ref：行内输入框在 v-for / v-if 中渲染，用 ref 会被 Vue 收成数组，
// 故用根 ref + querySelector 定位（同一时刻至多一个行内输入框）。
const rootRef = ref<HTMLElement | null>(null)

function focusEdit(attempt = 0) {
  void nextTick(() => {
    const el = rootRef.value?.querySelector<HTMLInputElement | HTMLTextAreaElement>(
      'input.inline-edit, textarea.inline-edit',
    )
    const e = editing.value
    if (!el) {
      // 虚拟滚动下目标行可能尚未渲染，重试几帧
      if (attempt < 3 && e) requestAnimationFrame(() => focusEdit(attempt + 1))
      return
    }
    if (!e) return
    el.focus()
    el.setSelectionRange(e.selStart, e.selEnd)
    if (el instanceof HTMLTextAreaElement) autoGrowInline(el)
  })
}

/** 多行输入框随内容自动增高（上限 120px，超出后内部滚动）。 */
function autoGrowInline(el: HTMLTextAreaElement) {
  el.style.height = 'auto'
  // scrollHeight 不含边框：补上上下各 1px 边框，避免多出 1 行滚动条。
  el.style.height = `${Math.min(el.scrollHeight + 2, 120)}px`
}

function onInlineInput(ev: Event) {
  const el = ev.target
  if (el instanceof HTMLTextAreaElement) autoGrowInline(el)
}

/**
 * 行内输入框键盘处理：Enter 提交、Esc 取消。
 * 「新建文件夹」是多行输入（每行一个名称），用 Shift+Enter 插入换行。
 */
function onInlineKeydown(ev: KeyboardEvent) {
  // 中文输入法正在组合时，Enter 用于确认候选词，不能当作「提交」。
  if (ev.isComposing || ev.keyCode === 229) return
  if (ev.key === 'Enter') {
    if (ev.shiftKey && editing.value?.mode === 'mkdir') return
    ev.preventDefault()
    ev.stopPropagation()
    void commitEdit(true)
    return
  }
  if (ev.key === 'Escape') {
    ev.preventDefault()
    ev.stopPropagation()
    cancelEdit()
  }
}

/** 进入某条目的行内重命名 */
function startRename(path: string) {
  cancelPendingRename()
  if (isRemote.value) return // 远程标签只读
  const entry =
    treeRows.value.find((r) => r.entry.path === path)?.entry ??
    props.tab.entries.find((e) => e.path === path)
  if (!entry) return
  const name = entry.name
  let selEnd = name.length
  if (!entry.isDir) {
    const dot = name.lastIndexOf('.')
    if (dot > 0) selEnd = dot // 文件：默认只选中主名（不含扩展名）
  }
  editSeq += 1
  editing.value = { mode: 'rename', path, name, selStart: 0, selEnd }
  if (viewMode.value !== 'icons') {
    const idx = treeRows.value.findIndex((r) => r.entry.path === path)
    if (idx >= 0) virtualizer.value.scrollToIndex(idx, { align: 'auto' })
  }
  focusEdit()
}

function cancelEdit() {
  editSeq += 1
  editing.value = null
}

/** 进入「新建」行内编辑（文件夹 / 空文件），在当前目录创建 */
function startCreate(kind: 'mkdir' | 'mkdirMove' | 'createFile') {
  if (!isLocal.value) return // 搜索结果 / 远程标签无本地目录语义
  emit('error', null)
  editSeq += 1
  editing.value = { mode: kind, name: '', selStart: 0, selEnd: 0 }
  if (kind === 'mkdir') {
    ui.setStatusText('新建文件夹：每行一个名称，Shift+Enter 换行，Enter 创建，Esc 取消')
  } else if (kind === 'mkdirMove') {
    const count = props.tab.selected.length
    ui.setStatusText(`移动到新建文件夹：输入名称，Enter 创建并移动 ${count} 项，Esc 取消`)
  }
  focusEdit()
}

/** 正在行内新建（文件夹 / 文件）时，即使目录为空也要渲染列表分支以显示输入行 */
const editingCreate = computed(
  () =>
    editing.value?.mode === 'mkdir' ||
    editing.value?.mode === 'mkdirMove' ||
    editing.value?.mode === 'createFile',
)

/** 行内新建的是文件夹（普通新建 / 新建并移动）。 */
const editingFolder = computed(
  () => editing.value?.mode === 'mkdir' || editing.value?.mode === 'mkdirMove',
)

/** 取文件名的扩展名（不含点、小写）；无扩展名（含 `.gitignore` 这类点开头）返回空串。 */
function extOf(fileName: string): string {
  const dot = fileName.lastIndexOf('.')
  return dot > 0 ? fileName.slice(dot + 1).toLowerCase() : ''
}

async function commitEdit(reopenFailed = false) {
  const e = editing.value
  if (!e) return
  // 新建类要落到「当前目录」，搜索结果没有目录语义；重命名作用于条目自身路径，本地标签都可用。
  const creating =
    e.mode === 'mkdir' || e.mode === 'mkdirMove' || e.mode === 'createFile'
  if (isRemote.value || (creating && !isLocal.value)) {
    editing.value = null
    return
  }
  // 新建文件夹：一行一个名称，空行忽略。
  const names =
    e.mode === 'mkdir'
      ? e.name.split(/\r?\n/).map((line) => line.trim()).filter(Boolean)
      : []
  const name = e.name.trim()
  editSeq += 1
  const seq = editSeq
  editing.value = null // 先关闭，避免 Enter 与 blur 重复提交
  if (e.mode === 'mkdir') {
    if (!names.length) return
    // 名称校验在本地完成：非法项不提交，合法项照常创建。
    const failures: { name: string; reason: string }[] = []
    const validNames: string[] = []
    for (const item of names) {
      const reason = validateEntryName(item)
      if (reason) failures.push({ name: item, reason })
      else validNames.push(item)
    }
    if (validNames.length) {
      const result = await paneStore.createFolders(props.paneId, validNames)
      failures.push(...result.failures)
    }
    if (failures.length) {
      const details = failures.map((item) => `${item.name}（${item.reason}）`).join('；')
      emit(
        'error',
        failures.length === 1
          ? `无法创建文件夹：${details}`
          : `有 ${failures.length} 个文件夹创建失败：${details}`,
      )
    } else {
      emit('error', null)
    }
    if (reopenFailed && failures.length && editSeq === seq && !editing.value) {
      // 回车提交失败时保留失败项，便于修正后直接重试（原因已由 store 汇总到状态栏）。
      const failedNames = failures.map((item) => item.name).filter(Boolean)
      if (failedNames.length) {
        const text = failedNames.join('\n')
        editing.value = { mode: 'mkdir', name: text, selStart: text.length, selEnd: text.length }
        focusEdit()
      }
    }
    return
  }
  if (e.mode === 'mkdirMove') {
    if (!name) return
    const nameError = validateEntryName(name)
    if (nameError) {
      emit('error', `无法创建文件夹「${name}」：${nameError}`)
      if (reopenFailed && editSeq === seq && !editing.value) {
        editing.value = { mode: 'mkdirMove', name, selStart: 0, selEnd: name.length }
        focusEdit()
      }
      return
    }
    const result = await paneStore.createFolderAndMove(props.paneId, name)
    if (!result.ok) {
      const reason = result.reason || '未知原因'
      emit(
        'error',
        result.created
          ? `文件夹「${name}」已创建，但所选条目移动失败：${reason}`
          : `无法创建文件夹「${name}」：${reason}`,
      )
    } else {
      emit('error', null)
    }
    // 文件夹尚未创建（名称非法 / 已存在 / 取消提权）时保留输入，便于直接改名重试。
    if (!result.created && result.reason && reopenFailed && editSeq === seq && !editing.value) {
      editing.value = {
        mode: 'mkdirMove',
        name,
        selStart: 0,
        selEnd: name.length,
      }
      focusEdit()
    }
    return
  }
  if (!name) return
  if (creating) {
    const nameError = validateEntryName(name)
    if (nameError) {
      emit('error', `无法新建文件：${nameError}`)
      if (reopenFailed && editSeq === seq && !editing.value) {
        editing.value = { mode: e.mode, name: e.name, selStart: 0, selEnd: e.name.length }
        focusEdit()
      }
      return
    }
    const result = await paneStore.runFs(props.paneId, {
      op: e.mode,
      target: joinPath(props.tab.path, name),
    })
    if (!result.ok) emit('error', result.errors.join('；') || '新建文件失败')
    else emit('error', null)
    return
  }
  if (!e.path || name === basename(e.path)) return // 名称未变则取消
  const nameError = validateEntryName(name)
  if (nameError) {
    emit('error', `无法重命名：${nameError}`)
    if (reopenFailed && editSeq === seq && !editing.value) {
      // 回车提交失败时保留用户输入，便于修正后直接重试。
      editing.value = { mode: 'rename', path: e.path, name: e.name, selStart: 0, selEnd: e.name.length }
      focusEdit()
    }
    return
  }
  const result = await paneStore.runFs(props.paneId, {
    op: 'rename',
    sources: [e.path],
    target: joinPath(dirname(e.path), name),
  })
  if (!result.ok) {
    emit('error', result.errors.join('；') || '重命名失败')
    return
  }
  emit('error', null)
  // 扩展名决定文件类型（图标 / 打开方式），变更后提示一次。
  const oldExt = extOf(basename(e.path))
  if (oldExt && oldExt !== extOf(name)) {
    ui.setStatusText('已重命名：扩展名已变更，文件类型可能改变')
  }
}

function openCtx(ev: MouseEvent, entry: FileEntry) {
  ev.preventDefault()
  if (isRemote.value) return
  if (!props.tab.selected.includes(entry.path)) props.tab.selected = [entry.path]
  void showNativeCtx(ev, entry)
}

function currentDirEntry(): FileEntry {
  return {
    id: -1,
    name: basename(props.tab.path),
    path: props.tab.path,
    isDir: true,
    size: 0,
    modified: 0,
    ext: null,
    hidden: false,
    system: false,
  }
}

function onBlankContextMenu(ev: MouseEvent) {
  if (isRemote.value) {
    ev.preventDefault()
    return
  }
  if (!isLocal.value) return
  const el = ev.target as HTMLElement | null
  if (el?.closest('.row, .icon-cell, .header, .inline-new')) return
  if (props.tab.loading || props.tab.error) return
  ev.preventDefault()
  void showNativeCtx(ev, currentDirEntry())
}

/**
 * 系统菜单末尾追加的应用扩展项（顺序固定）。
 * 回收站标签不追加任何应用项；远程标签不会走到这里。
 */
function contextMenuItems(): { id: string; label: string }[] {
  if (props.tab.kind === 'trash') return []
  const selected = props.tab.selected.length
  const items: { id: string; label: string }[] = [
    { id: 'undo', label: paneStore.canUndo ? `撤销${paneStore.undoLabel}` : '撤销上一步' },
  ]
  // 重做只在撤销栈里确实有可重做项时出现（系统菜单不支持禁用项）。
  if (paneStore.canRedo) items.push({ id: 'redo', label: `重做${paneStore.redoLabel}` })
  if (selected) {
    items.push({ id: 'duplicate', label: '复制副本' })
    items.push({ id: 'copy-to', label: '复制到…' })
    items.push({ id: 'move-to', label: '移动到…' })
  }
  if (selected >= 2) items.push({ id: 'batch-rename', label: '批量重命名…' })
  if (isSearch.value) items.push({ id: 'open-containing', label: '打开所在文件夹' })
  return items
}

/** 分发系统菜单里被选中的应用扩展项（目标一律取当前选中集，而非可能已过期的 entry）。 */
function handleCtxAction(id: string, entry: FileEntry) {
  switch (id) {
    case 'undo':
      void paneStore.undoLast()
      return
    case 'redo':
      void paneStore.redoLast()
      return
    case 'duplicate': {
      const sources = [...props.tab.selected]
      if (!sources.length) return
      void paneStore.runFs(props.paneId, { op: 'duplicate', sources })
      return
    }
    case 'copy-to':
      void paneStore.copySelectedTo(props.paneId, 'copy')
      return
    case 'move-to':
      void paneStore.copySelectedTo(props.paneId, 'move')
      return
    case 'batch-rename':
      batchRenameOpen.value = true
      return
    case 'open-containing': {
      // 按「本窗格 + 本标签」导航：非活动窗格右键时不能把活动窗格带走。
      const dir = dirname(entry.path)
      void paneStore.navigateTo(props.paneId, dir, props.tab.id).then(() => {
        props.tab.selected = [entry.path]
        paneStore.revealTarget = { tabId: props.tab.id, path: entry.path }
      })
      return
    }
  }
}

async function showNativeCtx(event: MouseEvent, entry: FileEntry) {
  cancelPendingRename()
  if (!props.tab.selected.includes(entry.path)) props.tab.selected = [entry.path]
  try {
    const picked = await api.showNativeShellMenu(
      entry.path,
      {
        x: event.clientX,
        y: event.clientY,
        scale: window.devicePixelRatio || 1,
      },
      contextMenuItems(),
    )
    if (picked) handleCtxAction(picked, entry)
  } catch (error) {
    ui.setStatusText(`无法打开系统菜单：${String(error)}`)
  }
}
// ---- 剪贴板 / 重命名快捷键（仅作用于当前活动窗格） ----

function isEditableTarget(t: EventTarget | null): boolean {
  if (!(t instanceof HTMLElement)) return false
  if (t.isContentEditable) return true
  const tag = t.tagName
  return tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT'
}

function onGlobalKey(ev: KeyboardEvent) {
  if (isCapturingShortcut()) return // 正在设置里捕获新键位，不要触发文件操作
  if (isEditableTarget(ev.target)) return
  if (paneStore.activePaneId !== props.paneId) return // 左右窗格只处理一次
  if (isModalOpen()) return // 有确认框 / 批量重命名等模态层时不要操作背后的列表

  // Ctrl+A：全选当前视图条目（Windows 资源管理器习惯）。
  if (
    !isModalOpen() &&
    (ev.ctrlKey || ev.metaKey) &&
    !ev.altKey &&
    !ev.shiftKey &&
    ev.key.toLowerCase() === 'a'
  ) {
    const items = navItems()
    if (!items.length) return
    ev.preventDefault()
    props.tab.selected = items.map((item) => item.path)
    anchor = 0
    navCursor = items.length - 1
    return
  }

  const id = match(ev)
  if (!id) {
    if (handleNavKey(ev)) ev.preventDefault()
    return
  }

  // 标签页操作对本地 / 搜索 / 远程标签都适用。
  if (id === 'close-tab') {
    ev.preventDefault()
    paneStore.closeTab(props.paneId, props.tab.id)
    return
  }
  if (id === 'new-tab') {
    ev.preventDefault()
    paneStore.addTab(props.paneId)
    return
  }

  switch (id) {
    case 'copy':
    case 'cut': {
      const sel = props.tab.selected
      if (!sel.length) return // 无选中项静默忽略
      const location = props.tab.remote
      if (location) {
        paneStore.setClipboard(
          id === 'copy' ? 'copy' : 'move',
          [...sel],
          location,
          transferItemsFor([...sel]),
        )
      } else {
        paneStore.setClipboard(id === 'copy' ? 'copy' : 'move', [...sel])
      }
      ev.preventDefault()
      return
    }
    case 'paste':
      // 远程标签的粘贴目标就是当前远端目录（本地上传 / 远端剪贴板下载）。
      if (!isLocal.value && !isRemote.value) return
      ev.preventDefault()
      void paneStore.pasteInto(props.paneId, props.tab.path)
      return
    case 'copy-path': {
      const sel = props.tab.selected
      if (!sel.length) return
      ev.preventDefault()
      // 与 Windows「复制为路径」一致：每行一个带引号的完整路径。
      const text = sel.map((path) => `"${path}"`).join('\r\n')
      void navigator.clipboard.writeText(text).then(
        () => ui.setStatusText(sel.length > 1 ? `已复制 ${sel.length} 个路径` : '已复制路径'),
        (error) => ui.setStatusText(`复制路径失败：${String(error)}`),
      )
      return
    }
    case 'copy-to':
      ev.preventDefault()
      void paneStore.copySelectedTo(props.paneId, 'copy')
      return
    case 'move-to':
      ev.preventDefault()
      void paneStore.copySelectedTo(props.paneId, 'move')
      return
    case 'undo':
      ev.preventDefault()
      void paneStore.undoLast()
      return
    case 'redo':
      ev.preventDefault()
      void paneStore.redoLast()
      return
    case 'duplicate':
      if (!canFileOp.value || !props.tab.selected.length) return
      ev.preventDefault()
      void paneStore.runFs(props.paneId, { op: 'duplicate', sources: [...props.tab.selected] })
      return
    case 'rename':
      if (!canFileOp.value) return // 远程条目不提供本地重命名
      if (props.tab.selected.length > 1) {
        // 多选时 F2 进入批量重命名。
        ev.preventDefault()
        batchRenameOpen.value = true
      } else if (props.tab.selected.length === 1) {
        ev.preventDefault()
        startRename(props.tab.selected[0])
      }
      return
    case 'delete':
    case 'delete-permanent':
      if (!canFileOp.value) return // 远程删除不在本轮本地文件操作范围内
      if (props.tab.selected.length) {
        ev.preventDefault()
        emit('delete', [...props.tab.selected], id === 'delete-permanent')
      }
      return
  }
}

onMounted(() => {
  window.addEventListener('keydown', onGlobalKey)
  window.addEventListener('stelix:batch-rename', onBatchRenameRequest)
})

onBeforeUnmount(() => {
  stopColumnResize()
  scrollPortObserver?.disconnect()
  scrollPortObserver = null
  cancelPendingRename()
  window.removeEventListener('keydown', onGlobalKey)
  window.removeEventListener('stelix:batch-rename', onBatchRenameRequest)
  stopMarquee()
  paneStore.setVisibleDirs(props.paneId, [])
})

/** 命令面板「批量重命名」入口。 */
function onBatchRenameRequest() {
  if (paneStore.activePaneId !== props.paneId) return
  if (!canFileOp.value || props.tab.selected.length < 2) {
    ui.setStatusText('请先选择至少两个本地条目')
    return
  }
  batchRenameOpen.value = true
}

// ---- 工具栏「新建」请求 ----

watch(
  () => props.createRequest,
  (req) => {
    if (!req || req.tabId !== props.tab.id) return
    startCreate(req.kind)
    emit('create-handled')
  },
)

// ---- 外部定位（搜索结果 / AI） ----

watch(
  () => paneStore.revealTarget,
  (t) => {
    if (!t || t.tabId !== props.tab.id) return
    const idx = treeRows.value.findIndex((r) => r.entry.path === t.path)
    if (idx >= 0 && viewMode.value !== 'icons') {
      virtualizer.value.scrollToIndex(idx, { align: 'center' })
    }
    paneStore.revealTarget = null
  },
)

watch(
  () => [props.tab.id, props.tab.path],
  () => {
    anchor = -1
    navCursor = -1
    // 切换标签 / 目录后回到表格最左侧，表头同步复位。
    if (scrollRef.value) scrollRef.value.scrollLeft = 0
    headerScrollX.value = 0
    cancelPendingRename()
    cancelEdit()
  },
)

// 详情/列表与图标视图的下标基准不同（treeRows 含展开子项），切换视图后旧锚点不再有效。
watch(viewMode, () => {
  anchor = -1
  navCursor = -1
  // 列表视图没有横向滚动，切回详情时以真实偏移为准重新同步表头。
  headerScrollX.value = 0
})

const hasEntries = computed(() => props.tab.entries.length > 0)
// 因隐藏设置而被清空的目录（无文本过滤与类型筛选时）
const emptyByHidden = computed(
  () =>
    hasEntries.value &&
    rows.value.length === 0 &&
    !props.tab.filter.trim() &&
    !props.tab.typeFilter &&
    (!ui.showHidden || ui.hideProtected),
)
// 因过滤词 / 类型筛选而被清空的目录
const emptyByFilter = computed(
  () =>
    hasEntries.value &&
    rows.value.length === 0 &&
    (!!props.tab.filter.trim() || !!props.tab.typeFilter),
)
const emptyText = computed(() =>
  props.tab.kind === 'search'
    ? '没有找到匹配项'
    : isRemote.value
      ? '远程目录为空（右键可上传文件）'
      : '此目录为空',
)

// ---- 详情视图：文件夹大小按需统计（仅可见行） ----

/** 详情视图当前可见的文件夹路径（虚拟滚动，通常仅十几行；含原地展开的子项）。 */
const visibleDirPaths = computed(() => {
  if (viewMode.value !== 'details') return []
  if (isRemote.value) return [] // POSIX 远程路径不做本地递归统计
  const out: string[] = []
  for (const item of virtualizer.value.getVirtualItems()) {
    const row = treeRows.value[item.index]
    if (row?.entry.isDir) out.push(row.entry.path)
  }
  return out
})

watch(
  visibleDirPaths,
  (paths) => paneStore.setVisibleDirs(props.paneId, paths),
  { immediate: true },
)

/** 大小列文案：文件夹用统计出的递归大小（尚未算出则留空）。 */
function sizeText(entry: FileEntry): string {
  if (!entry.isDir) return formatSize(entry.size)
  const size = paneStore.folderSizes[entry.path]
  return size === undefined ? '' : formatSize(size)
}

interface DragPayload {
  paneId: string
  items: RemoteTransferItem[]
  /** 来源为远程标签时带上连接信息 */
  remote?: RemoteLocation
}

/** 由选中路径补齐目录标记（含原地展开的子项），供拖拽 / 复制 / 剪切使用。 */
function transferItemsFor(paths: string[]): RemoteTransferItem[] {
  const known = new Map<string, FileEntry>()
  for (const entry of props.tab.entries) known.set(entry.path, entry)
  for (const row of treeRows.value) known.set(row.entry.path, row.entry)
  for (const children of Object.values(paneStore.dirChildren)) {
    for (const entry of children) known.set(entry.path, entry)
  }
  return paths.map((path) => ({ path, isDir: known.get(path)?.isDir ?? false }))
}

function readDragPayload(ev: DragEvent): DragPayload | null {
  const transfer = ev.dataTransfer
  const custom = transfer?.getData(DRAG_TYPE)
  const plain = transfer?.getData(DRAG_FALLBACK_TYPE) ?? ''
  const raw = custom || (plain.startsWith(DRAG_PREFIX) ? plain.slice(DRAG_PREFIX.length) : '')
  if (!raw) return null
  try {
    const payload = JSON.parse(raw) as Partial<DragPayload> & { paths?: unknown }
    if (typeof payload.paneId !== 'string') return null
    const items = Array.isArray(payload.items)
      ? payload.items
      : Array.isArray(payload.paths)
        ? payload.paths.map((path) => ({ path, isDir: false }))
        : []
    const parsed = items
      .filter(
        (item): item is RemoteTransferItem =>
          !!item &&
          typeof item === 'object' &&
          typeof (item as RemoteTransferItem).path === 'string' &&
          !!(item as RemoteTransferItem).path,
      )
      .map((item) => ({ path: item.path, isDir: !!item.isDir }))
    if (!parsed.length) return null
    return {
      paneId: payload.paneId,
      items: parsed,
      remote: payload.remote,
    }
  } catch {
    return null
  }
}

function onDragStart(ev: DragEvent, entry: FileEntry) {
  if (!ev.dataTransfer) return
  if (!props.tab.selected.includes(entry.path)) props.tab.selected = [entry.path]
  const paths = props.tab.selected.length ? [...props.tab.selected] : [entry.path]
  const payload: DragPayload = {
    paneId: props.paneId,
    items: transferItemsFor(paths),
    remote: props.tab.remote,
  }
  const data = JSON.stringify(payload)
  ev.dataTransfer.setData(DRAG_TYPE, data)
  // WebView2 在拖放过程中可能不暴露自定义 MIME 类型；用带标记的标准文本类型兼容。
  ev.dataTransfer.setData(DRAG_FALLBACK_TYPE, `${DRAG_PREFIX}${data}`)
  ev.dataTransfer.effectAllowed = 'copyMove'

  if (props.tab.remote) return
  // HTML drag data is only visible to other web views. Start an OS file drag
  // instead so applications such as WeChat receive real filesystem paths.
  ev.preventDefault()
  const operation = ev.ctrlKey || ev.shiftKey ? 'copy' : 'move'
  window.dispatchEvent(new CustomEvent('stelix:file-drag-start', { detail: { operation } }))
  void startDrag({
    item: paths,
    // Omit the optional preview asset so no asynchronous resource lookup can
    // delay starting the native drag after the user's pointer gesture.
    icon: '',
    // External applications should receive a copy; Stelix panes apply the
    // in-app move/copy behavior from the drag-start modifier.
    mode: 'copy',
  }, ({ result }) => {
    window.dispatchEvent(new CustomEvent('stelix:file-drag-end'))
    if (result === 'Cancelled') ui.setStatusText('文件未被目标应用接收')
  })
    .catch((error: unknown) => {
      window.dispatchEvent(new CustomEvent('stelix:file-drag-end'))
      console.error('无法启动文件拖拽', error)
      ui.setStatusText(`文件拖拽启动失败：${String(error)}`)
    })
}

/**
 * 是否为应用内拖拽。
 *
 * 只能看 `dataTransfer.types`：按 HTML 规范 `getData()` 要到 `drop` 阶段才可读，
 * `dragover` 阶段取到的永远是空串，不能用来判断。
 */
function isInternalDrag(ev: DragEvent): boolean {
  const types = Array.from(ev.dataTransfer?.types ?? [])
  return types.includes(DRAG_TYPE) || types.includes(DRAG_FALLBACK_TYPE)
}

function onDragOver(ev: DragEvent) {
  if (!isInternalDrag(ev) || !ev.dataTransfer) return
  if (!isRemote.value && !isLocal.value) return
  // 跨本地 / 远程默认复制：远端删除需逐条成功后追加 quote 操作，拖拽不隐含删除。
  // Ctrl / Shift 强制复制，符合 Windows 资源管理器的拖拽习惯。
  ev.dataTransfer.dropEffect = isRemote.value || ev.ctrlKey || ev.shiftKey ? 'copy' : 'move'
}

function onEntryDragOver(ev: DragEvent, entry: FileEntry) {
  if (!entry.isDir || !isInternalDrag(ev)) return
  ev.preventDefault()
  ev.stopPropagation()
  if (ev.dataTransfer) {
    ev.dataTransfer.dropEffect = isRemote.value || ev.ctrlKey || ev.shiftKey ? 'copy' : 'move'
  }
  dragOverPath.value = entry.path
}

function onEntryDragLeave(ev: DragEvent, path: string) {
  const related = ev.relatedTarget as Node | null
  if (related && (ev.currentTarget as HTMLElement).contains(related)) return
  if (dragOverPath.value === path) dragOverPath.value = null
}

async function onEntryDrop(ev: DragEvent, entry: FileEntry) {
  if (!entry.isDir) return
  ev.preventDefault()
  ev.stopPropagation()
  dragOverPath.value = null
  const payload = readDragPayload(ev)
  if (!payload) return

  if (isRemote.value) {
    const target = props.tab.remote
    if (!target) return
    if (payload.remote) {
      ui.setStatusText('暂不支持在远程服务器之间直接传输，请先下载到本地')
      return
    }
    await paneStore.uploadToRemote(props.paneId, target, payload.items, entry.path, 'copy')
    return
  }

  if (payload.remote) {
    await paneStore.downloadFromRemote(
      props.paneId,
      payload.remote,
      payload.items,
      entry.path,
      'copy',
    )
    return
  }

  const result = await paneStore.runFs(props.paneId, {
    op: ev.ctrlKey || ev.shiftKey ? 'copy' : 'move',
    sources: payload.items.map((item) => item.path),
    target: entry.path,
  })
  if (result.ok) await paneStore.reloadByPath(entry.path)
}

function onDrop(ev: DragEvent) {
  dragOverPath.value = null
  const payload = readDragPayload(ev)
  if (!payload || payload.paneId === props.paneId) return

  if (isRemote.value) {
    const target = props.tab.remote
    if (!target) return
    if (payload.remote) {
      ui.setStatusText('暂不支持在远程服务器之间直接传输，请先下载到本地')
      return
    }
    // 跨设备默认复制：不因拖拽意外删除本地源文件。
    void paneStore.uploadToRemote(props.paneId, target, payload.items, props.tab.path, 'copy')
    return
  }

  if (payload.remote) {
    void paneStore.downloadFromRemote(
      props.paneId,
      payload.remote,
      payload.items,
      props.tab.path,
      'copy',
    )
    return
  }

  const paths = payload.items.map((item) => item.path)
  if (ev.ctrlKey || ev.shiftKey) void paneStore.copyToOtherPane(payload.paneId, paths, payload.items)
  else void paneStore.moveToOtherPane(payload.paneId, paths, payload.items)
}
</script>

<template>
  <div
    ref="rootRef"
    class="file-list"
    :class="[viewMode, { search: isSearch }]"
    @mousedown="onRootMouseDown"
    @dragover.prevent="onDragOver"
    @drop.prevent="onDrop"
    @contextmenu="onBlankContextMenu"
  >
    <!-- 加载中 -->
    <div v-if="tab.loading" class="state">
      <span class="spinner" />
      <p>正在读取…</p>
    </div>

    <!-- 错误 -->
    <div v-else-if="tab.error" class="state error">
      <Icon name="alert-triangle" :size="30" class="big-ico" />
      <p class="msg">{{ tab.error }}</p>
      <button class="retry" @click="paneStore.refresh(paneId)">重试</button>
    </div>

    <!-- 空目录 / 无匹配 -->
    <div v-else-if="!hasEntries && !editingCreate" class="state">
      <Icon name="folder-open" :size="30" class="big-ico" />
      <p>{{ emptyText }}</p>
    </div>
    <div v-else-if="emptyByHidden && !editingCreate" class="state">
      <Icon name="folder-open" :size="30" class="big-ico" />
      <p>{{ !ui.showHidden ? '已隐藏隐藏文件' : '已隐藏受保护的操作系统文件' }}</p>
    </div>
    <div v-else-if="emptyByFilter && !editingCreate" class="state">
      <Icon name="search" :size="30" class="big-ico" />
      <p>{{ tab.filter.trim() ? `没有匹配「${tab.filter}」的条目` : '没有符合类型筛选的条目' }}</p>
    </div>

    <!-- 详细 / 列表视图（虚拟滚动） -->
    <template v-else-if="viewMode !== 'icons'">
      <div v-if="viewMode === 'details'" class="header-clip">
        <div class="header" :style="headerStyle">
          <button class="col name" data-col="name" @click="paneStore.toggleSort(tab, 'name')">
            名称 <Icon v-if="tab.sortKey === 'name'" :name="tab.sortAsc ? 'chevron-up' : 'chevron-down'" :size="11" />
            <span
              class="col-resizer"
              role="separator"
              aria-orientation="vertical"
              aria-label="调整名称列宽度"
              title="拖动调整「名称」列宽，双击恢复默认"
              @mousedown="startColumnResize($event, 'name')"
              @click.stop
              @dblclick.stop="ui.resetDetailColumnWidth('name')"
            />
          </button>
          <button v-if="isSearch" class="col path" data-col="path" @click="paneStore.toggleSort(tab, 'path')">
            路径 <Icon v-if="tab.sortKey === 'path'" :name="tab.sortAsc ? 'chevron-up' : 'chevron-down'" :size="11" />
            <span
              class="col-resizer"
              role="separator"
              aria-orientation="vertical"
              aria-label="调整路径列宽度"
              title="拖动调整「路径」列宽，双击恢复默认"
              @mousedown="startColumnResize($event, 'path')"
              @click.stop
              @dblclick.stop="ui.resetDetailColumnWidth('path')"
            />
          </button>
          <button class="col size" data-col="size" @click="paneStore.toggleSort(tab, 'size')">
            大小 <Icon v-if="tab.sortKey === 'size'" :name="tab.sortAsc ? 'chevron-up' : 'chevron-down'" :size="11" />
            <span
              class="col-resizer"
              role="separator"
              aria-orientation="vertical"
              aria-label="调整大小列宽度"
              title="拖动调整「大小」列宽，双击恢复默认"
              @mousedown="startColumnResize($event, 'size')"
              @click.stop
              @dblclick.stop="ui.resetDetailColumnWidth('size')"
            />
          </button>
          <button class="col time" data-col="time" @click="paneStore.toggleSort(tab, 'modified')">
            修改时间 <Icon v-if="tab.sortKey === 'modified'" :name="tab.sortAsc ? 'chevron-up' : 'chevron-down'" :size="11" />
            <span
              class="col-resizer"
              role="separator"
              aria-orientation="vertical"
              aria-label="调整修改时间列宽度"
              title="拖动调整「修改时间」列宽，双击恢复默认"
              @mousedown="startColumnResize($event, 'time')"
              @click.stop
              @dblclick.stop="ui.resetDetailColumnWidth('time')"
            />
          </button>
          <button class="col type" data-col="type" @click="paneStore.toggleSort(tab, 'ext')">
            类型 <Icon v-if="tab.sortKey === 'ext'" :name="tab.sortAsc ? 'chevron-up' : 'chevron-down'" :size="11" />
            <span
              class="col-resizer"
              role="separator"
              aria-orientation="vertical"
              aria-label="调整类型列宽度"
              title="拖动调整「类型」列宽，双击恢复默认"
              @mousedown="startColumnResize($event, 'type')"
              @click.stop
              @dblclick.stop="ui.resetDetailColumnWidth('type')"
            />
          </button>
        </div>
      </div>

      <div
        v-if="editingCreate && editing"
        class="inline-new"
        :class="{ file: editing.mode === 'createFile', multi: editing.mode === 'mkdir' }"
        :style="inlineNewStyle"
      >
        <span class="name-cell">
          <span class="twisty placeholder" />
          <span class="ico">
            <FileIcon v-if="editingFolder" :path="tab.path" :is-dir="true" :ext="null" :size="18" />
            <Icon v-else name="file" :size="18" />
          </span>
          <span class="nm">
            <textarea
              v-if="editing.mode === 'mkdir'"
              v-model="editing.name"
              class="inline-edit inline-edit-multi"
              spellcheck="false"
              rows="1"
              placeholder="每行一个名称"
              @keydown="onInlineKeydown"
              @input="onInlineInput"
              @blur="commitEdit()"
              @click.stop
            />
            <input
              v-else
              v-model="editing.name"
              class="inline-edit"
              spellcheck="false"
              :placeholder="editing.mode === 'mkdirMove' ? '新文件夹名称' : undefined"
              @keydown="onInlineKeydown"
              @blur="commitEdit()"
              @click.stop
            />
          </span>
        </span>
      </div>

      <div ref="scrollRef" class="fl-scroll" @click.self="onBlankClick" @scroll.passive="onListScroll">
        <div class="fl-inner" :style="[{ height: `${virtualizer.getTotalSize()}px` }, innerStyle]">
          <div
            v-for="vr in virtualRows"
            :key="String(vr.key)"
            class="row"
            :data-selection-path="vr.row.entry.path"
            :draggable="!editing"
            :class="{ selected: tab.selected.includes(vr.row.entry.path), dir: vr.row.entry.isDir, 'drop-target': dragOverPath === vr.row.entry.path }"
            :style="[{ height: `${vr.size}px`, transform: `translateY(${vr.start}px)` }, rowStyle]"
            @click="onClick($event, vr.index, vr.row.entry)"
            @dblclick="onDblClick(vr.row.entry)"
            @contextmenu="openCtx($event, vr.row.entry)"
            @dragstart="onDragStart($event, vr.row.entry)"
            @dragover="onEntryDragOver($event, vr.row.entry)"
            @dragleave="onEntryDragLeave($event, vr.row.entry.path)"
            @drop="onEntryDrop($event, vr.row.entry)"
          >
            <span class="name-cell">
              <span v-if="vr.row.depth" class="indent" :style="{ width: `${vr.row.depth * 10}px` }" />
              <button
                v-if="vr.row.entry.isDir && !isRemote"
                class="twisty"
                :class="{ expanded: vr.row.expanded }"
                :title="vr.row.expanded ? '折叠' : '展开'"
                :aria-label="vr.row.expanded ? '折叠' : '展开'"
                @click.stop="paneStore.toggleExpand(tab.id, vr.row.entry.path)"
                @dblclick.stop
              >
                <Icon :name="vr.row.expanded ? 'chevron-down' : 'chevron-right'" :size="12" />
              </button>
              <span v-else class="twisty placeholder" />
              <span class="ico" :class="`k-${fileKind(vr.row.entry)}`">
                <Icon v-if="isRemote" :name="fileIcon(vr.row.entry)" :size="18" />
                <FileIcon
                  v-else
                  :path="vr.row.entry.path"
                  :is-dir="vr.row.entry.isDir"
                  :ext="vr.row.entry.ext"
                  :size="18"
                />
              </span>
              <span v-if="editing && editing.mode === 'rename' && editing.path === vr.row.entry.path" class="nm">
                <input
                  v-model="editing.name"
                  class="inline-edit"
                  spellcheck="false"
                  @keydown.enter.stop.prevent="commitEdit"
                  @keydown.esc.stop.prevent="cancelEdit"
                  @blur="commitEdit()"
                  @click.stop
                  @dblclick.stop
                />
              </span>
              <span v-else class="nm" :title="vr.row.entry.path">{{ ui.displayFileName(vr.row.entry.name, vr.row.entry.isDir) }}</span>
            </span>
            <template v-if="viewMode === 'details'">
              <span v-if="isSearch" class="path" :title="dirname(vr.row.entry.path)">
                {{ dirname(vr.row.entry.path) }}
              </span>
              <span class="size">{{ sizeText(vr.row.entry) }}</span>
              <span class="time">{{ formatTime(vr.row.entry.modified) }}</span>
              <span class="type">{{ typeLabel(vr.row.entry) }}</span>
            </template>
          </div>
        </div>
      </div>
    </template>

    <!-- 图标视图（限制渲染数量） -->
    <div v-else ref="iconsScrollRef" class="icons-scroll" @click.self="onBlankClick">
      <div v-if="editingCreate && editing" class="icon-cell" :class="{ dir: editingFolder }">
        <span class="icon-big" :class="editingFolder ? 'k-folder' : 'k-file'">
          <FileIcon v-if="editingFolder" :path="tab.path" :is-dir="true" :ext="null" :size="38" />
          <Icon v-else name="file" :size="38" />
        </span>
        <span class="icon-name">
          <textarea
            v-if="editing.mode === 'mkdir'"
            v-model="editing.name"
            class="inline-edit inline-edit-multi"
            spellcheck="false"
            rows="1"
            placeholder="每行一个名称"
            @keydown="onInlineKeydown"
            @input="onInlineInput"
            @blur="commitEdit()"
            @click.stop
          />
          <input
            v-else
            v-model="editing.name"
            class="inline-edit"
            spellcheck="false"
            :placeholder="editing.mode === 'mkdirMove' ? '新文件夹名称' : undefined"
            @keydown="onInlineKeydown"
            @blur="commitEdit()"
            @click.stop
          />
        </span>
      </div>
      <div class="icons-grid">
        <div
          v-for="(entry, i) in iconRows"
          :key="entry.path"
          class="icon-cell"
          :data-selection-path="entry.path"
          :draggable="!editing"
          :class="{ selected: tab.selected.includes(entry.path), dir: entry.isDir, 'drop-target': dragOverPath === entry.path }"
          :title="entry.path"
          @click="onClick($event, i, entry)"
          @dblclick="onDblClick(entry)"
          @contextmenu="openCtx($event, entry)"
          @dragstart="onDragStart($event, entry)"
          @dragover="onEntryDragOver($event, entry)"
          @dragleave="onEntryDragLeave($event, entry.path)"
          @drop="onEntryDrop($event, entry)"
        >
          <span class="icon-big" :class="`k-${fileKind(entry)}`">
            <Icon v-if="isRemote" :name="fileIcon(entry)" :size="38" />
            <FileIcon v-else :path="entry.path" :is-dir="entry.isDir" :ext="entry.ext" :size="38" />
          </span>
          <span v-if="editing && editing.mode === 'rename' && editing.path === entry.path" class="icon-name">
            <input
              v-model="editing.name"
              class="inline-edit"
              spellcheck="false"
              @keydown.enter.stop.prevent="commitEdit"
              @keydown.esc.stop.prevent="cancelEdit"
              @blur="commitEdit()"
              @click.stop
              @dblclick.stop
            />
          </span>
          <span v-else class="icon-name">{{ ui.displayFileName(entry.name, entry.isDir) }}</span>
        </div>
      </div>
      <p v-if="rows.length > ICON_CAP" class="cap-hint">
        条目较多，仅显示前 {{ ICON_CAP }} 项（共 {{ rows.length }} 项）
      </p>
    </div>

    <BatchRenameDialog
      v-if="batchRenameOpen"
      :entries="batchRenameEntries"
      :siblings="tab.entries"
      @cancel="batchRenameOpen = false"
      @confirm="onBatchRenameConfirm"
    />

    <div
      v-if="marquee"
      class="marquee-box"
      :style="{
        left: `${marqueeBox.left}px`,
        top: `${marqueeBox.top}px`,
        width: `${marqueeBox.width}px`,
        height: `${marqueeBox.height}px`,
      }"
    />

  </div>
</template>

<style scoped>
.file-list {
  flex: 1;
  min-height: 0;
  font-size: var(--file-font-size);
  display: flex;
  flex-direction: column;
  background: transparent;
  overflow: hidden;
  position: relative;
}
.marquee-box {
  position: absolute;
  z-index: 20;
  pointer-events: none;
  background: color-mix(in srgb, var(--accent) 24%, transparent);
}

/* 状态页 */
.state {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 6px;
  color: var(--fg-dim);
  font-size: 1em;
  background: var(--bg-app);
  padding: 20px;
  text-align: center;
}
.state .big-ico {
  color: var(--fg-dim);
}
.state.error .big-ico {
  color: var(--danger);
}
.state .msg {
  max-width: 90%;
  word-break: break-word;
  color: var(--danger);
}
.state.error .msg {
  color: var(--fg-muted);
}
.retry {
  margin-top: 4px;
  padding: 3px 12px;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  color: var(--fg-muted);
}
.retry:hover {
  background: var(--bg-hover);
  color: var(--fg);
}
.spinner {
  width: 18px;
  height: 18px;
  border: 2px solid var(--border-strong);
  border-top-color: var(--accent);
  border-radius: 50%;
  animation: spin 0.7s linear infinite;
}
@keyframes spin {
  to {
    transform: rotate(360deg);
  }
}

/* 表头 */
/* 横向滚动时裁掉移出视口的表头，避免盖到窗格边缘之外 */
.header-clip {
  flex: none;
  overflow: hidden;
  background: var(--pane-header-bg);
  border-bottom: 1px solid var(--border);
}
.header {
  display: grid;
  grid-template-columns: 1fr 90px 140px 90px;
  height: 23px;
  padding-left: 8px;
  flex: none;
  /* 表头随表格横向滚动，用 transform 跟随（见 headerStyle） */
  will-change: transform;
}
.header .col {
  position: relative;
  text-align: left;
  padding: 0 6px;
  font-size: 0.9167em;
  color: var(--fg-muted);
  white-space: nowrap;
  overflow: hidden;
}
/* 列宽拖动手柄：贴在列右边缘，悬停时显示强调色分隔线 */
.col-resizer {
  position: absolute;
  top: 0;
  right: 0;
  z-index: 1;
  width: 7px;
  height: 100%;
  cursor: col-resize;
  touch-action: none;
}
.col-resizer::after {
  content: '';
  position: absolute;
  top: 4px;
  bottom: 4px;
  right: 0;
  width: 1px;
  /* 列间细分隔线：略短于表头高度，半透明的浅边框色，避免生硬 */
  background: color-mix(in srgb, var(--border-strong) 55%, transparent);
  transition:
    background 120ms ease,
    width 120ms ease;
}
/* 最后一列右侧不留分隔线，悬停时仍可拖动调整列宽 */
.header .col:last-child .col-resizer:not(:hover)::after {
  background: transparent;
}
.col-resizer:hover::after,
.col-resizer:active::after {
  width: 2px;
  background: var(--accent);
}
/* 拖动列宽期间统一光标，避免划过其他元素时闪烁 */
:global(body.col-resizing),
:global(body.col-resizing *) {
  cursor: col-resize !important;
  user-select: none !important;
}
/* 「名称」左移与文件名的文字对齐（让开 三角 + 图标 的位置） */
.header .col.name {
  padding-left: 38px;
}
.header .col:hover {
  color: var(--fg);
}
.header .col .icon {
  color: var(--accent);
  vertical-align: -0.1em;
}
/* 虚拟滚动 */
.fl-scroll {
  flex: 1;
  min-height: 0;
  overflow: auto;
  background: var(--bg-app);
}
.fl-inner {
  position: relative;
  width: 100%;
}

/* 行内「新建文件夹」输入行（表头之下、列表之上） */
.inline-new {
  display: grid;
  grid-template-columns: 1fr 90px 140px 90px;
  align-items: center;
  height: var(--file-row-h);
  padding-left: 8px;
  flex: none;
  font-size: 1em;
  background: var(--bg-hover);
  border-bottom: 1px solid var(--border);
}
.file-list.list .inline-new {
  grid-template-columns: 1fr;
}
.inline-new .ico {
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--ico-folder);
  flex: none;
}
.inline-new.file .ico {
  color: var(--ico-file);
}
.inline-new .nm {
  flex: 1 1 auto;
  min-width: 0;
}
/* 多行新建（文件夹）：输入框随行数增高，行容器跟随内容 */
.inline-new.multi {
  height: auto;
  min-height: var(--file-row-h);
  align-items: start;
  padding-top: 4px;
  padding-bottom: 4px;
}
.inline-new.multi .name-cell {
  align-items: start;
}
.inline-new.multi .ico,
.inline-new.multi .twisty {
  height: 20px;
}

/* 行内编辑输入框 */
.inline-edit {
  width: 100%;
  height: 20px;
  padding: 0 4px;
  font: inherit;
  font-size: 1em;
  color: var(--fg);
  background: var(--bg-input);
  border: 1px solid var(--accent);
  border-radius: var(--radius-sm);
  outline: none;
}
/* 多行输入框：Shift+Enter 换行，高度由脚本按内容设置 */
.inline-edit-multi {
  display: block;
  height: 20px;
  min-height: 20px;
  max-height: 120px;
  padding: 1px 4px;
  line-height: 16px;
  resize: none;
  overflow-y: auto;
  white-space: pre-wrap;
  overflow-wrap: break-word;
}
.icon-name .inline-edit {
  display: block;
  width: 100%;
}
/* 图标视图下多行新建：名称区跟随输入框增高，不再单行裁剪 */
.icon-name:has(.inline-edit-multi) {
  display: block;
  width: 100%;
  overflow: visible;
  white-space: normal;
}
.row {
  position: absolute;
  top: 0;
  left: 0;
  width: 100%;
  display: grid;
  grid-template-columns: 1fr 90px 140px 90px;
  align-items: center;
  padding-left: 8px;
  font-size: 1em;
  color: var(--fg);
  cursor: default;
  white-space: nowrap;
}
.row:hover {
  background: var(--bg-hover);
}
.row.selected {
  background: var(--bg-selected);
}
.row.drop-target {
  background: color-mix(in srgb, var(--accent) 24%, var(--bg-hover));
  outline: 2px solid color-mix(in srgb, var(--accent) 82%, transparent);
  outline-offset: -2px;
  box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--accent) 38%, transparent);
}
.row.drop-target .nm,
.row.drop-target .ico {
  color: var(--accent);
}
/* 名称单元格：缩进占位 + 展开三角 + 图标 + 名称（整体随层级右移，其余列保持对齐） */
.name-cell {
  display: flex;
  align-items: center;
  gap: 4px;
  min-width: 0;
  overflow: hidden;
}
.name-cell .indent {
  flex: none;
}
.name-cell .twisty {
  flex: none;
  display: flex;
  align-items: center;
  justify-content: center;
  width: 16px;
  height: 16px;
  border-radius: var(--radius-sm);
  color: var(--fg-dim);
  cursor: pointer;
}
.name-cell .twisty:hover {
  background: var(--bg-selected);
  color: var(--fg);
}
.name-cell .twisty.placeholder {
  visibility: hidden;
}
.row .ico {
  display: flex;
  align-items: center;
  justify-content: center;
  flex: none;
}
.row .nm {
  flex: 1 1 auto;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
}
.row.dir .nm {
  color: var(--fg-folder);
}
.row .size,
.row .time,
.row .type,
.row .path {
  padding: 0 6px;
  color: var(--fg-muted);
  font-size: 0.9583em;
  overflow: hidden;
  text-overflow: ellipsis;
}
/* 路径列（仅搜索结果）：允许收缩，宽度不足时尾部省略 */
.row .path {
  min-width: 0;
}

/* 列表视图：隐藏后三列 */
.file-list.list .row {
  grid-template-columns: 1fr;
}

/* 图标视图 */
.icons-scroll {
  flex: 1;
  min-height: 0;
  overflow: auto;
  padding: 10px;
  background: var(--bg-app);
}
.icons-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(100px, 1fr));
  gap: 8px;
}
.icon-cell {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 6px;
  padding: 8px 6px;
  border-radius: var(--radius);
  cursor: default;
  text-align: center;
}
.icon-cell:hover {
  background: var(--bg-hover);
}
.icon-cell.selected {
  background: var(--bg-selected);
}
.icon-cell.drop-target {
  background: color-mix(in srgb, var(--accent) 24%, var(--bg-hover));
  outline: 2px solid color-mix(in srgb, var(--accent) 82%, transparent);
  outline-offset: -2px;
  box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--accent) 38%, transparent);
}
.icon-cell.drop-target .icon-big,
.icon-cell.drop-target .icon-name {
  color: var(--accent);
}
.icon-big {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 52px;
  height: 52px;
  border-radius: 14px;
  background: transparent;
  transition: transform 120ms ease;
}
.icon-cell:hover .icon-big {
  transform: translateY(-1px);
}
.icon-cell.selected .icon-big {
  background: transparent;
}
.icon-name {
  font-size: 1em;
  color: var(--fg);
  max-width: 100%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.icon-cell.dir .icon-name {
  color: var(--fg-folder);
}
.cap-hint {
  margin: 12px 0 4px;
  text-align: center;
  font-size: 0.9167em;
  color: var(--fg-dim);
}

/* ---- 文件类型配色：让列表在不依赖彩色 emoji 的前提下仍有辨识度 ---- */
.k-folder {
  color: var(--ico-folder);
}
.k-doc {
  color: var(--ico-doc);
}
.k-image {
  color: var(--ico-image);
}
.k-video {
  color: var(--ico-video);
}
.k-audio {
  color: var(--ico-audio);
}
.k-archive {
  color: var(--ico-archive);
}
.k-app {
  color: var(--ico-app);
}
.k-code {
  color: var(--ico-code);
}
.k-disc {
  color: var(--ico-disc);
}
.k-file {
  color: var(--ico-file);
}
</style>

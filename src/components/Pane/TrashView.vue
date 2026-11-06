<script setup lang="ts">
// 回收站视图：列出系统回收站条目，支持还原、打开原位置、永久删除与清空。
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { api } from '../../api'
import type { TabState, TrashEntry, TrashOpResult } from '../../types'
import { usePaneStore } from '../../stores/pane'
import { useUiStore, type TrashColumnKey } from '../../stores/ui'
import { isCapturingShortcut } from '../../stores/shortcuts'
import { formatSize, formatTime, RECYCLE_BIN_SHELL_PATH } from './utils'
import Icon from '../icons/Icon.vue'
import FileIcon from '../icons/FileIcon.vue'
import { refreshTrashIcon } from '../icons/fileIconCache'

const props = defineProps<{ paneId: string; tab: TabState }>()
const paneStore = usePaneStore()
const ui = useUiStore()

const entries = ref<TrashEntry[]>([])
const selected = ref<Set<string>>(new Set())
const loading = ref(false)
const busy = ref(false)
const error = ref<string | null>(null)
const confirm = ref<{ kind: 'purge' | 'empty'; ids: string[] } | null>(null)
let selectionAnchor: string | null = null

const selectedIds = computed(() => entries.value.filter((item) => selected.value.has(item.id)).map((item) => item.id))
const selectedEntries = computed(() => entries.value.filter((item) => selected.value.has(item.id)))

// ---- 排序（点击表头切换，默认按删除时间从新到旧） ----
const sortKey = ref<TrashColumnKey>('deleted')
const sortAsc = ref(false)
const collator = new Intl.Collator('zh-CN', { numeric: true, sensitivity: 'base' })

const sortedEntries = computed(() => {
  const dir = sortAsc.value ? 1 : -1
  return [...entries.value].sort((a, b) => {
    // 文件夹始终排在文件之前，与文件窗格的详细视图一致。
    if (a.isDir !== b.isDir) return a.isDir ? -1 : 1
    let cmp = 0
    switch (sortKey.value) {
      case 'origin':
        cmp = collator.compare(a.originalParent, b.originalParent)
        if (cmp === 0) cmp = collator.compare(a.name, b.name)
        break
      case 'deleted':
        cmp = a.deletedAt - b.deletedAt
        if (cmp === 0) cmp = collator.compare(a.name, b.name)
        break
      case 'size':
        cmp = (a.isDir ? a.itemCount : a.size) - (b.isDir ? b.itemCount : b.size)
        break
      default:
        cmp = collator.compare(a.name, b.name)
    }
    return cmp * dir
  })
})

function toggleSort(key: TrashColumnKey) {
  if (sortKey.value === key) {
    sortAsc.value = !sortAsc.value
  } else {
    sortKey.value = key
    sortAsc.value = key === 'name' || key === 'origin'
  }
}

// ---- 列宽（表头拖动调整，ui store 持久化；与文件窗格详细视图同规则） ----

/** 表头与行左右内边距之和，计算最小表格宽度时补上，保证表头与行严格对齐。 */
const TABLE_PAD = 16
/** 名称列自适应（未手动调整）时的最小宽度。 */
const NAME_COL_MIN = 160
/** 行尾操作列固定宽度。 */
const ACTIONS_COL_WIDTH = 78

const columnWidths = computed(() => ui.trashColumnWidths)
/** 当前窗格的视图模式（详细 / 列表 / 图标），与普通文件夹共用同一窗格设置。 */
const viewMode = computed(
  () => paneStore.panes.find((pane) => pane.id === props.paneId)?.viewMode ?? 'details',
)
/** 图标视图渲染上限，避免回收站条目过多时一次性创建大量节点。 */
const ICON_CAP = 600
const iconEntries = computed(() => sortedEntries.value.slice(0, ICON_CAP))
const columnTemplate = computed(() => {
  const w = columnWidths.value
  const cols = [w.name === null ? `minmax(${NAME_COL_MIN}px, 1fr)` : `${w.name}px`]
  cols.push(`${w.origin}px`, `${w.deleted}px`, `${w.size}px`, `${ACTIONS_COL_WIDTH}px`)
  return cols.join(' ')
})
/** 各列宽度之和：内容超出窗格时出现横向滚动，而不是挤压列宽。 */
const tableWidth = computed(() => {
  const w = columnWidths.value
  return (w.name ?? NAME_COL_MIN) + w.origin + w.deleted + w.size + ACTIONS_COL_WIDTH
})
const tableMinWidth = computed(() => `${tableWidth.value + TABLE_PAD}px`)

/** 列表可视区宽度（不含垂直滚动条），用于让表头与行严格对齐。 */
const scrollRef = ref<HTMLElement | null>(null)
const rootRef = ref<HTMLElement | null>(null)
const scrollPortWidth = ref(0)
const headerScrollX = ref(0)
const tableDisplayWidth = computed(() => Math.max(scrollPortWidth.value, tableWidth.value + TABLE_PAD))
const headerStyle = computed(() => ({
  gridTemplateColumns: columnTemplate.value,
  width: `${tableDisplayWidth.value}px`,
  transform: `translateX(-${headerScrollX.value}px)`,
}))

/** 表格独立横向滚动时，表头跟随内容平移，保持列对齐。 */
function onScroll() {
  const x = scrollRef.value?.scrollLeft ?? 0
  if (x !== headerScrollX.value) headerScrollX.value = x
}

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
})

interface ColumnDrag {
  key: TrashColumnKey
  startX: number
  startWidth: number
}
let columnDrag: ColumnDrag | null = null

/** 表头当前真实列宽（名称列自适应时也能拿到实际像素值）。 */
function measureColumn(key: TrashColumnKey): number {
  const el = rootRef.value?.querySelector<HTMLElement>(`.header .col[data-col="${key}"]`)
  if (el) return el.getBoundingClientRect().width
  return columnWidths.value[key] ?? NAME_COL_MIN
}

function startColumnResize(ev: MouseEvent, key: TrashColumnKey) {
  if (ev.button !== 0) return
  ev.preventDefault()
  ev.stopPropagation()
  // 名称列自适应时先固定为当前像素宽度：否则它会把相邻列的增量全部吸收。
  if (columnWidths.value.name === null) ui.setTrashColumnWidth('name', measureColumn('name'))
  columnDrag = { key, startX: ev.clientX, startWidth: measureColumn(key) }
  document.body.classList.add('col-resizing')
  window.addEventListener('mousemove', onColumnResizeMove)
  window.addEventListener('mouseup', stopColumnResize)
  window.addEventListener('blur', stopColumnResize)
}

function onColumnResizeMove(ev: MouseEvent) {
  if (!columnDrag) return
  ui.setTrashColumnWidth(columnDrag.key, columnDrag.startWidth + ev.clientX - columnDrag.startX)
}

function stopColumnResize() {
  if (!columnDrag) return
  columnDrag = null
  document.body.classList.remove('col-resizing')
  window.removeEventListener('mousemove', onColumnResizeMove)
  window.removeEventListener('mouseup', stopColumnResize)
  window.removeEventListener('blur', stopColumnResize)
}

const canRestore = computed(() => !busy.value && selectedIds.value.length > 0)
const canPurge = computed(() => !busy.value && selectedIds.value.length > 0)
const canOpenOriginal = computed(() => !busy.value && selectedEntries.value.length === 1)
const canEmpty = computed(() => !busy.value && entries.value.length > 0)
const confirmCount = computed(() =>
  confirm.value?.kind === 'empty' ? entries.value.length : confirm.value?.ids.length ?? 0,
)

function extOf(name: string): string | null {
  const dot = name.lastIndexOf('.')
  return dot > 0 && dot < name.length - 1 ? name.slice(dot + 1).toLowerCase() : null
}

function sizeLabel(item: TrashEntry): string {
  return item.isDir ? `${item.itemCount} 项` : formatSize(item.size)
}

async function load() {
  if (loading.value) return
  loading.value = true
  error.value = null
  try {
    const next = await api.trashList()
    entries.value = next
    // 回收站内容可能已被还原 / 清空 / 外部修改，侧边栏图标随之刷新。
    refreshTrashIcon()
    const alive = new Set(next.map((item) => item.id))
    selected.value = new Set([...selected.value].filter((id) => alive.has(id)))
    if (selectionAnchor && !alive.has(selectionAnchor)) selectionAnchor = null
  } catch (cause) {
    error.value = String(cause)
  } finally {
    loading.value = false
  }
}

function clearSelection() {
  selected.value = new Set()
  selectionAnchor = null
}

function onRowClick(event: MouseEvent, item: TrashEntry) {
  if (event.shiftKey && selectionAnchor) {
    const anchor = sortedEntries.value.findIndex((entry) => entry.id === selectionAnchor)
    const current = sortedEntries.value.findIndex((entry) => entry.id === item.id)
    if (anchor >= 0 && current >= 0) {
      const [start, end] = anchor < current ? [anchor, current] : [current, anchor]
      const next = event.ctrlKey || event.metaKey ? new Set(selected.value) : new Set<string>()
      for (let i = start; i <= end; i += 1) next.add(sortedEntries.value[i].id)
      selected.value = next
      return
    }
  }
  if (event.ctrlKey || event.metaKey) {
    const next = new Set(selected.value)
    if (next.has(item.id)) next.delete(item.id)
    else next.add(item.id)
    selected.value = next
  } else {
    selected.value = new Set([item.id])
  }
  selectionAnchor = item.id
}

function openOriginal(item: TrashEntry) {
  if (!item.originalParent) {
    error.value = '无法确定该条目的原位置'
    return
  }
  paneStore.addTab(props.paneId, item.originalParent)
}

async function restore(ids: string[]) {
  if (!ids.length || busy.value) return
  busy.value = true
  error.value = null
  try {
    const result = await api.trashRestore(ids)
    // 先重载列表（会清空旧错误），再报告本次操作结果，避免错误提示被清掉。
    await load()
    report(result, '已还原')
  } catch (cause) {
    error.value = String(cause)
  } finally {
    busy.value = false
  }
}

function report(result: TrashOpResult, action: string) {
  if (result.ok > 0) ui.setStatusText(`${action} ${result.ok} 项`)
  if (result.errors.length) error.value = result.errors.join('；')
}

function closeConfirm() {
  if (!busy.value) confirm.value = null
}

function askPurge(ids: string[]) {
  if (!ids.length || busy.value) return
  confirm.value = { kind: 'purge', ids }
}

function restoreSelected() {
  void restore(selectedIds.value)
}

function openOriginalSelected() {
  const item = selectedEntries.value[0]
  if (item) openOriginal(item)
}

function askPurgeSelected() {
  askPurge(selectedIds.value)
}

function askEmpty() {
  if (!entries.value.length || busy.value) return
  confirm.value = { kind: 'empty', ids: [] }
}

/** 供窗格标题栏渲染回收站操作。 */
defineExpose({
  count: computed(() => entries.value.length),
  selectedCount: computed(() => selected.value.size),
  canRestore,
  canPurge,
  canOpenOriginal,
  canEmpty,
  restoreSelected,
  openOriginalSelected,
  askPurgeSelected,
  askEmpty,
})

async function confirmPurge() {
  const target = confirm.value
  if (!target || busy.value) return
  busy.value = true
  error.value = null
  try {
    const result =
      target.kind === 'empty'
        ? await api.trashEmpty()
        : await api.trashPurge(target.ids)
    confirm.value = null
    clearSelection()
    await load()
    report(result, target.kind === 'empty' ? '已清空回收站并删除' : '已永久删除')
  } catch (cause) {
    error.value = String(cause)
  } finally {
    busy.value = false
  }
}

function isEditableTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false
  const tag = target.tagName
  return target.isContentEditable || tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT'
}

function onKeydown(event: KeyboardEvent) {
  if (paneStore.activePaneId !== props.paneId || isCapturingShortcut()) return
  if (confirm.value) {
    if (event.key === 'Escape') {
      event.preventDefault()
      confirm.value = null
    } else if (event.key === 'Enter' && !event.isComposing) {
      event.preventDefault()
      void confirmPurge()
    }
    return
  }
  if (isEditableTarget(event.target)) return
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'a') {
    event.preventDefault()
    selected.value = new Set(entries.value.map((item) => item.id))
    return
  }
  if (event.key === 'Escape') {
    if (selected.value.size) {
      event.preventDefault()
      clearSelection()
    }
    return
  }
  if (event.key === 'Delete') {
    event.preventDefault()
    if (selectedIds.value.length) askPurge(selectedIds.value)
  }
}

watch(
  () => paneStore.trashRefreshSeq,
  () => {
    if (props.tab.kind === 'trash') void load()
  },
)

onMounted(() => {
  window.addEventListener('keydown', onKeydown, true)
  void load()
})

onBeforeUnmount(() => {
  window.removeEventListener('keydown', onKeydown, true)
  scrollPortObserver?.disconnect()
  stopColumnResize()
})
</script>

<template>
  <div ref="rootRef" class="trash-view">
    <div v-if="error" class="error-bar" role="alert">
      <Icon name="alert-circle" :size="13" />
      <span>{{ error }}</span>
      <button title="关闭" @click="error = null"><Icon name="x" :size="12" /></button>
    </div>

    <div v-if="loading && !entries.length" class="state">正在读取回收站…</div>
    <div v-else-if="!entries.length && error" class="state">回收站读取失败，请稍后重试。</div>
    <div v-else-if="!entries.length" class="empty-state">
      <div class="empty-icon">
        <FileIcon
          :path="RECYCLE_BIN_SHELL_PATH"
          :is-dir="false"
          :ext="null"
          :use-path-icon="true"
          :empty-trash="true"
          fallback="trash"
          :size="34"
        />
      </div>
      <p>回收站是空的</p>
      <span>删除的文件会先放到这里，之后可以随时还原。</span>
    </div>

    <template v-else>
      <div v-if="viewMode === 'details'" class="table-area">
      <div class="header-clip">
        <div class="header" :style="headerStyle">
          <button class="col name" data-col="name" @click="toggleSort('name')">
            名称 <Icon v-if="sortKey === 'name'" :name="sortAsc ? 'chevron-up' : 'chevron-down'" :size="11" />
            <span
              class="col-resizer"
              role="separator"
              aria-orientation="vertical"
              aria-label="调整名称列宽度"
              title="拖动调整「名称」列宽，双击恢复默认"
              @mousedown="startColumnResize($event, 'name')"
              @click.stop
              @dblclick.stop="ui.resetTrashColumnWidth('name')"
            />
          </button>
          <button class="col origin" data-col="origin" @click="toggleSort('origin')">
            原位置 <Icon v-if="sortKey === 'origin'" :name="sortAsc ? 'chevron-up' : 'chevron-down'" :size="11" />
            <span
              class="col-resizer"
              role="separator"
              aria-orientation="vertical"
              aria-label="调整原位置列宽度"
              title="拖动调整「原位置」列宽，双击恢复默认"
              @mousedown="startColumnResize($event, 'origin')"
              @click.stop
              @dblclick.stop="ui.resetTrashColumnWidth('origin')"
            />
          </button>
          <button class="col deleted" data-col="deleted" @click="toggleSort('deleted')">
            删除时间 <Icon v-if="sortKey === 'deleted'" :name="sortAsc ? 'chevron-up' : 'chevron-down'" :size="11" />
            <span
              class="col-resizer"
              role="separator"
              aria-orientation="vertical"
              aria-label="调整删除时间列宽度"
              title="拖动调整「删除时间」列宽，双击恢复默认"
              @mousedown="startColumnResize($event, 'deleted')"
              @click.stop
              @dblclick.stop="ui.resetTrashColumnWidth('deleted')"
            />
          </button>
          <button class="col size num" data-col="size" @click="toggleSort('size')">
            大小 <Icon v-if="sortKey === 'size'" :name="sortAsc ? 'chevron-up' : 'chevron-down'" :size="11" />
            <span
              class="col-resizer"
              role="separator"
              aria-orientation="vertical"
              aria-label="调整大小列宽度"
              title="拖动调整「大小」列宽，双击恢复默认"
              @mousedown="startColumnResize($event, 'size')"
              @click.stop
              @dblclick.stop="ui.resetTrashColumnWidth('size')"
            />
          </button>
          <span class="col actions" aria-hidden="true" />
        </div>
      </div>

      <div ref="scrollRef" class="table-scroll" @scroll="onScroll" @click.self="clearSelection">
        <div class="trash-table" :style="{ minWidth: tableMinWidth }">
          <div
            v-for="item in sortedEntries"
            :key="item.id"
            class="trash-row"
            :class="{ selected: selected.has(item.id), dir: item.isDir }"
            :style="{ gridTemplateColumns: columnTemplate }"
            @click="onRowClick($event, item)"
          >
            <div class="name-cell" :title="item.name">
              <FileIcon :path="item.originalPath" :is-dir="item.isDir" :ext="extOf(item.name)" :size="18" />
              <span class="name">{{ ui.displayFileName(item.name, item.isDir) }}</span>
            </div>
            <span class="path-cell" :title="item.originalPath">{{ item.originalParent }}</span>
            <span class="time-cell">{{ formatTime(item.deletedAt) }}</span>
            <span class="size-cell num">{{ sizeLabel(item) }}</span>
            <span class="row-actions">
              <button title="还原" @click.stop="restore([item.id])"><Icon name="restore" :size="12" /></button>
              <button title="打开原位置" @click.stop="openOriginal(item)"><Icon name="folder-arrow" :size="12" /></button>
              <button class="danger" title="永久删除" @click.stop="askPurge([item.id])"><Icon name="trash" :size="12" /></button>
            </span>
          </div>
        </div>
      </div>
      </div>

      <div v-else-if="viewMode === 'list'" class="list-scroll" @click.self="clearSelection">
        <div
          v-for="item in sortedEntries"
          :key="item.id"
          class="list-row"
          :class="{ selected: selected.has(item.id), dir: item.isDir }"
          :title="item.originalPath"
          @click="onRowClick($event, item)"
        >
          <div class="name-cell">
            <FileIcon :path="item.originalPath" :is-dir="item.isDir" :ext="extOf(item.name)" :size="18" />
            <span class="name">{{ ui.displayFileName(item.name, item.isDir) }}</span>
          </div>
          <span class="row-actions">
            <button title="还原" @click.stop="restore([item.id])"><Icon name="restore" :size="12" /></button>
            <button title="打开原位置" @click.stop="openOriginal(item)"><Icon name="folder-arrow" :size="12" /></button>
            <button class="danger" title="永久删除" @click.stop="askPurge([item.id])"><Icon name="trash" :size="12" /></button>
          </span>
        </div>
      </div>

      <div v-else class="icons-scroll" @click.self="clearSelection">
        <div class="icons-grid">
          <div
            v-for="item in iconEntries"
            :key="item.id"
            class="icon-cell"
            :class="{ selected: selected.has(item.id), dir: item.isDir }"
            :title="item.originalPath"
            @click="onRowClick($event, item)"
          >
            <span class="icon-big">
              <FileIcon :path="item.originalPath" :is-dir="item.isDir" :ext="extOf(item.name)" :size="38" />
            </span>
            <span class="icon-name">{{ ui.displayFileName(item.name, item.isDir) }}</span>
          </div>
        </div>
        <p v-if="sortedEntries.length > ICON_CAP" class="cap-hint">
          条目较多，仅显示前 {{ ICON_CAP }} 项（共 {{ sortedEntries.length }} 项）
        </p>
      </div>
    </template>

    <div v-if="confirm" class="mask" @mousedown.stop @click.self="closeConfirm">
      <div class="dialog">
        <h3>{{ confirm.kind === 'empty' ? '清空回收站' : '确认永久删除' }}</h3>
        <p class="warn-text danger">
          {{
            confirm.kind === 'empty'
              ? `将永久删除回收站中的全部 ${confirmCount} 项，此操作不可撤销。`
              : `将永久删除选中的 ${confirmCount} 项，此操作不可撤销。`
          }}
        </p>
        <div class="dialog-actions">
          <button :disabled="busy" @click="closeConfirm">取消</button>
          <button class="danger" :disabled="busy" @click="confirmPurge">
            {{ confirm.kind === 'empty' ? '清空回收站' : '永久删除' }}
          </button>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.trash-view {
  position: relative;
  display: flex;
  flex-direction: column;
  flex: 1;
  min-width: 0;
  min-height: 0;
  background: transparent;
  font-size: var(--file-font-size);
}
.error-bar {
  display: flex;
  align-items: center;
  gap: 7px;
  margin: 6px 8px 0;
  padding: 6px 8px;
  border: 1px solid var(--danger);
  border-radius: var(--radius);
  color: var(--fg);
  font-size: 0.9167em;
}
.error-bar span {
  flex: 1;
  min-width: 0;
  word-break: break-word;
}
.error-bar button {
  color: var(--fg-muted);
}
.state,
.empty-state {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 7px;
  color: var(--fg-dim);
  font-size: 1em;
  background: var(--pane-content-bg);
}
.state {
  padding: 20px;
}
.empty-icon {
  display: grid;
  place-items: center;
  width: 64px;
  height: 64px;
  margin-bottom: 3px;
  border-radius: 50%;
  color: var(--fg-dim);
  background: color-mix(in srgb, var(--fg-dim) 10%, transparent);
}
.empty-state p {
  margin: 0;
  color: var(--fg-muted);
  font-size: 1.0833em;
}
.empty-state span {
  font-size: 0.9167em;
}
/* 表头固定在列表上方，横向滚动时随内容平移（与文件窗格详细视图一致） */
.table-area {
  display: flex;
  flex-direction: column;
  flex: 1;
  min-width: 0;
  min-height: 0;
}
.header-clip {
  flex: none;
  overflow: hidden;
  background: var(--pane-header-bg);
  border-bottom: 1px solid var(--border);
}
.header {
  display: grid;
  align-items: center;
  height: 23px;
  padding: 0 8px;
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
/* 「名称」左移与文件名文字对齐（让开条目图标的位置） */
.header .col.name {
  padding-left: 24px;
}
.header .col:hover {
  color: var(--fg);
}
.header .col .icon {
  color: var(--accent);
  vertical-align: -0.1em;
}
.header .col.num {
  text-align: right;
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
  background: color-mix(in srgb, var(--border-strong) 55%, transparent);
  transition:
    background 120ms ease,
    width 120ms ease;
}
/* 最后一列右侧不留分隔线 */
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
.table-scroll {
  flex: 1;
  min-height: 0;
  min-width: 0;
  overflow: auto;
  background: var(--pane-content-bg);
}
.trash-table {
  width: 100%;
}
.trash-row {
  display: grid;
  align-items: center;
  padding: 0 8px;
  height: var(--file-row-h);
  color: var(--fg);
  font-size: 1em;
  cursor: default;
  user-select: none;
}
.trash-row:hover {
  background: var(--bg-hover);
}
.trash-row.selected {
  background: var(--bg-selected);
}
.name-cell {
  display: flex;
  align-items: center;
  gap: 7px;
  min-width: 0;
}
.name,
.path-cell {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.name {
  font-weight: 450;
}
.trash-row.dir .name,
.list-row.dir .name {
  color: var(--fg-folder);
}
.path-cell,
.time-cell {
  padding: 0 6px;
  color: var(--fg-muted);
  font-size: 0.9583em;
}
.time-cell {
  font-variant-numeric: tabular-nums;
}
.size-cell {
  padding: 0 6px;
  color: var(--fg-muted);
  font-size: 0.9583em;
}
.num {
  text-align: right;
}
.row-actions {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 1px;
  opacity: 0;
  transition: opacity 100ms ease;
}
.trash-row:hover .row-actions,
.trash-row.selected .row-actions,
.row-actions:focus-within {
  opacity: 1;
}
.row-actions button {
  display: grid;
  place-items: center;
  width: 22px;
  height: 22px;
  border-radius: var(--radius-sm);
  color: var(--fg-muted);
}
.row-actions button:hover {
  background: var(--bg-active);
  color: var(--fg);
}
.row-actions button.danger:hover {
  background: color-mix(in srgb, var(--danger) 16%, transparent);
  color: var(--danger);
}
/* 列表视图：单列展示名称，行内操作靠右（与文件窗格的列表视图一致） */
.list-scroll {
  flex: 1;
  min-width: 0;
  min-height: 0;
  overflow: auto;
  background: var(--pane-content-bg);
}
.list-row {
  display: flex;
  align-items: center;
  gap: 8px;
  height: var(--file-row-h);
  padding: 0 8px;
  color: var(--fg);
  font-size: 1em;
  cursor: default;
  user-select: none;
}
.list-row:hover {
  background: var(--bg-hover);
}
.list-row.selected {
  background: var(--bg-selected);
}
.list-row .name-cell {
  flex: 1 1 auto;
}
.list-row:hover .row-actions,
.list-row.selected .row-actions {
  opacity: 1;
}
/* 图标视图（与文件窗格的图标视图一致） */
.icons-scroll {
  flex: 1;
  min-height: 0;
  overflow: auto;
  padding: 10px;
  background: var(--pane-content-bg);
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
  user-select: none;
}
.icon-cell:hover {
  background: var(--bg-hover);
}
.icon-cell.selected {
  background: var(--bg-selected);
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
.mask {
  position: fixed;
  inset: 0;
  z-index: 300;
  display: flex;
  align-items: center;
  justify-content: center;
  background: var(--mask);
}
.dialog {
  width: 340px;
  max-width: 90vw;
  padding: 14px;
  display: flex;
  flex-direction: column;
  gap: 10px;
  border: 1px solid var(--border-strong);
  border-radius: var(--radius-lg);
  background: var(--bg-elevated);
  box-shadow: var(--shadow-lg);
}
.dialog h3 {
  margin: 0;
  color: var(--fg);
  font-size: 1.0833rem;
  font-weight: 600;
}
.warn-text {
  margin: 0;
  font-size: 1rem;
  color: var(--fg-muted);
}
.warn-text.danger {
  color: var(--danger);
}
.dialog-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}
.dialog-actions button {
  padding: 4px 14px;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  color: var(--fg-muted);
}
.dialog-actions button:hover:not(:disabled) {
  background: var(--bg-hover);
  color: var(--fg);
}
.dialog-actions button.danger {
  border-color: var(--danger);
  background: var(--danger);
  color: var(--accent-fg);
}
.dialog-actions button:disabled {
  opacity: 0.5;
  cursor: default;
}
</style>

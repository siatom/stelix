<script setup lang="ts">
// 回收站视图：列出系统回收站条目，支持还原、打开原位置、永久删除与清空。
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { api } from '../../api'
import type { TabState, TrashEntry, TrashOpResult } from '../../types'
import { usePaneStore } from '../../stores/pane'
import { useUiStore } from '../../stores/ui'
import { isCapturingShortcut } from '../../stores/shortcuts'
import { formatSize, formatTime, RECYCLE_BIN_SHELL_PATH } from './utils'
import Icon from '../icons/Icon.vue'
import FileIcon from '../icons/FileIcon.vue'

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
    const anchor = entries.value.findIndex((entry) => entry.id === selectionAnchor)
    const current = entries.value.findIndex((entry) => entry.id === item.id)
    if (anchor >= 0 && current >= 0) {
      const [start, end] = anchor < current ? [anchor, current] : [current, anchor]
      const next = event.ctrlKey || event.metaKey ? new Set(selected.value) : new Set<string>()
      for (let i = start; i <= end; i += 1) next.add(entries.value[i].id)
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
})
</script>

<template>
  <div class="trash-view">
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
          fallback="trash"
          :size="34"
        />
      </div>
      <p>回收站是空的</p>
      <span>删除的文件会先放到这里，之后可以随时还原。</span>
    </div>

    <div v-else class="table-scroll" @click.self="clearSelection">
      <div class="trash-table" :aria-busy="loading">
        <div class="trash-head">
          <span>名称</span>
          <span>原位置</span>
          <span>删除时间</span>
          <span class="num">大小</span>
          <span />
        </div>
        <div
          v-for="item in entries"
          :key="item.id"
          class="trash-row"
          :class="{ selected: selected.has(item.id) }"
          @click="onRowClick($event, item)"
        >
          <div class="name-cell" :title="item.name">
            <FileIcon :path="item.originalPath" :is-dir="item.isDir" :ext="extOf(item.name)" :size="17" />
            <span class="name">{{ item.name }}</span>
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
  background: var(--bg-app);
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
.table-scroll {
  flex: 1;
  min-height: 0;
  overflow: auto;
}
.trash-table {
  min-width: 720px;
}
.trash-head,
.trash-row {
  display: grid;
  grid-template-columns: minmax(180px, 1.4fr) minmax(160px, 1.6fr) 145px 96px 78px;
  align-items: center;
  gap: 8px;
  padding: 0 10px;
}
.trash-head {
  position: sticky;
  top: 0;
  z-index: 1;
  height: 30px;
  border-bottom: 1px solid var(--border);
  background: var(--bg-panel);
  color: var(--fg-dim);
  font-size: 0.875em;
  user-select: none;
}
.trash-row {
  position: relative;
  height: 34px;
  border-bottom: 1px solid color-mix(in srgb, var(--border) 62%, transparent);
  color: var(--fg);
  font-size: 0.9583em;
  cursor: default;
  user-select: none;
}
.trash-row:hover {
  background: var(--bg-hover);
}
.trash-row.selected {
  background: color-mix(in srgb, var(--accent) 14%, transparent);
}
.trash-row.selected:hover {
  background: color-mix(in srgb, var(--accent) 20%, transparent);
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
.path-cell,
.time-cell {
  color: var(--fg-muted);
  font-size: 0.9167em;
}
.time-cell {
  font-variant-numeric: tabular-nums;
}
.size-cell {
  color: var(--fg-muted);
  font-size: 0.9167em;
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

<script setup lang="ts">
// 文件列表：虚拟滚动 + 详细/列表/图标三视图 + 选择 + 右键菜单（WS-D）
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useVirtualizer } from '@tanstack/vue-virtual'
import { openPath } from '@tauri-apps/plugin-opener'
import type { FileEntry, TabState } from '../../types'
import { usePaneStore } from '../../stores/pane'
import { useUiStore } from '../../stores/ui'
import { basename, dirname, fileIcon, fileKind, formatSize, formatTime, joinPath, typeLabel } from './utils'
import Icon from '../icons/Icon.vue'

const props = defineProps<{ paneId: string; tab: TabState }>()
const emit = defineEmits<{
  (e: 'delete', paths: string[]): void
}>()

const paneStore = usePaneStore()
const ui = useUiStore()

const ROW_H = 26
const ICON_CAP = 600

const viewMode = computed(() => paneStore.panes.find((p) => p.id === props.paneId)?.viewMode ?? 'details')
// 先取 store 的过滤+排序结果，再按「隐藏受保护的操作系统文件」剔除（Explorer 语义：hidden && system）
const allRows = computed(() => paneStore.entriesFor(props.tab))
const rows = computed(() =>
  ui.hideProtected ? allRows.value.filter((e) => !(e.hidden && e.system)) : allRows.value,
)
const iconRows = computed(() => rows.value.slice(0, ICON_CAP))

const scrollRef = ref<HTMLElement | null>(null)
const virtualizer = useVirtualizer(
  computed(() => ({
    count: rows.value.length,
    getScrollElement: () => scrollRef.value,
    estimateSize: () => ROW_H,
    overscan: 12,
  })),
)

let anchor = -1

// ---- 选择 ----

function onClick(ev: MouseEvent, index: number, entry: FileEntry) {
  if (ev.shiftKey && anchor >= 0) {
    const a = Math.min(anchor, index)
    const b = Math.max(anchor, index)
    const paths = rows.value.slice(a, b + 1).map((e) => e.path)
    props.tab.selected = ev.ctrlKey || ev.metaKey ? Array.from(new Set([...props.tab.selected, ...paths])) : paths
  } else if (ev.ctrlKey || ev.metaKey) {
    paneStore.toggleSelect(props.tab, entry.path)
    anchor = index
  } else {
    props.tab.selected = [entry.path]
    anchor = index
  }
}

function clearSelection() {
  paneStore.clearSelection(props.tab)
  anchor = -1
}

// ---- 打开 ----

async function openEntry(entry: FileEntry) {
  if (entry.isDir) {
    await paneStore.navigateTo(props.paneId, entry.path)
    return
  }
  try {
    await openPath(entry.path)
  } catch (e) {
    ui.statusText = `无法打开：${String(e)}`
  }
}

async function onDblClick(entry: FileEntry) {
  await openEntry(entry)
}

// ---- 行内编辑（重命名 / 新建文件夹） ----

interface Editing {
  mode: 'rename' | 'mkdir'
  /** rename 时的原路径 */
  path?: string
  /** 输入框当前文本 */
  name: string
  /** 初始选中的字符范围（文件默认选中主名，目录/新建默认全选） */
  selStart: number
  selEnd: number
}

const editing = ref<Editing | null>(null)
// 根元素 ref：行内输入框在 v-for / v-if 中渲染，用 ref 会被 Vue 收成数组，
// 故用根 ref + querySelector 定位（同一时刻至多一个行内输入框）。
const rootRef = ref<HTMLElement | null>(null)

function focusEdit(attempt = 0) {
  void nextTick(() => {
    const el = rootRef.value?.querySelector<HTMLInputElement>('input.inline-edit')
    const e = editing.value
    if (!el) {
      // 虚拟滚动下目标行可能尚未渲染，重试几帧
      if (attempt < 3 && e) requestAnimationFrame(() => focusEdit(attempt + 1))
      return
    }
    if (!e) return
    el.focus()
    el.setSelectionRange(e.selStart, e.selEnd)
  })
}

/** 进入某条目的行内重命名 */
function startRename(path: string) {
  const entry = rows.value.find((e) => e.path === path) ?? props.tab.entries.find((e) => e.path === path)
  if (!entry) return
  const name = entry.name
  let selEnd = name.length
  if (!entry.isDir) {
    const dot = name.lastIndexOf('.')
    if (dot > 0) selEnd = dot // 文件：默认只选中主名（不含扩展名）
  }
  editing.value = { mode: 'rename', path, name, selStart: 0, selEnd }
  if (viewMode.value !== 'icons') {
    const idx = rows.value.findIndex((e) => e.path === path)
    if (idx >= 0) virtualizer.value.scrollToIndex(idx, { align: 'auto' })
  }
  focusEdit()
}

/** 在列表顶部插入一行行内「新建文件夹」输入 */
function startNewFolder() {
  const name = '新建文件夹'
  editing.value = { mode: 'mkdir', name, selStart: 0, selEnd: name.length }
  focusEdit()
}

function cancelEdit() {
  editing.value = null
}

/** 正在行内新建文件夹时，即使目录为空也要渲染列表分支以显示输入行 */
const editingMkdir = computed(() => editing.value?.mode === 'mkdir')

async function commitEdit() {
  const e = editing.value
  if (!e) return
  const name = e.name.trim()
  editing.value = null // 先关闭，避免 Enter 与 blur 重复提交
  if (!name) return
  if (e.mode === 'mkdir') {
    await paneStore.runFs(props.paneId, { op: 'mkdir', target: joinPath(props.tab.path, name) })
    return
  }
  if (!e.path || name === basename(e.path)) return // 名称未变则取消
  await paneStore.runFs(props.paneId, {
    op: 'rename',
    sources: [e.path],
    target: joinPath(dirname(e.path), name),
  })
}

defineExpose({ startRename, startNewFolder })

// ---- 右键菜单 ----

interface CtxState {
  x: number
  y: number
  entry: FileEntry
}
const ctx = ref<CtxState | null>(null)

function openCtx(ev: MouseEvent, entry: FileEntry) {
  ev.preventDefault()
  if (!props.tab.selected.includes(entry.path)) props.tab.selected = [entry.path]
  const menuW = 176
  const menuH = 210
  const x = Math.min(ev.clientX, window.innerWidth - menuW - 4)
  const y = Math.min(ev.clientY, window.innerHeight - menuH - 4)
  ctx.value = { x, y, entry }
}

function closeCtx() {
  ctx.value = null
}

function copyToClipboard(op: 'copy' | 'move') {
  const c = ctx.value
  if (!c) return
  paneStore.setClipboard(op, [c.entry.path])
  closeCtx()
}

async function copyPath() {
  const c = ctx.value
  if (!c) return
  try {
    await navigator.clipboard.writeText(c.entry.path)
    ui.statusText = '路径已复制'
  } catch {
    ui.statusText = `路径：${c.entry.path}`
  }
  closeCtx()
}

async function paste() {
  closeCtx()
  await paneStore.pasteInto(props.paneId, props.tab.path)
}

function onDocClick() {
  closeCtx()
}

function onKey(ev: KeyboardEvent) {
  if (ev.key === 'Escape') closeCtx()
}

// ---- 剪贴板 / 重命名快捷键（仅作用于当前活动窗格） ----

function isEditableTarget(t: EventTarget | null): boolean {
  if (!(t instanceof HTMLElement)) return false
  if (t.isContentEditable) return true
  const tag = t.tagName
  return tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT'
}

function onGlobalKey(ev: KeyboardEvent) {
  if (isEditableTarget(ev.target)) return
  if (paneStore.activePaneId !== props.paneId) return // 左右窗格只处理一次

  const key = ev.key.toLowerCase()
  const ctrl = ev.ctrlKey || ev.metaKey

  if (ctrl && !ev.shiftKey && !ev.altKey) {
    if (key === 'c' || key === 'x') {
      const sel = props.tab.selected
      if (!sel.length) return // 无选中项静默忽略
      paneStore.setClipboard(key === 'c' ? 'copy' : 'move', [...sel])
      ev.preventDefault()
      return
    }
    if (key === 'v') {
      ev.preventDefault()
      void paneStore.pasteInto(props.paneId, props.tab.path)
      return
    }
  }

  if (key === 'f2' && !ctrl && !ev.altKey && !ev.shiftKey) {
    if (props.tab.selected.length === 1) {
      ev.preventDefault()
      startRename(props.tab.selected[0])
    }
  }
}

onMounted(() => {
  document.addEventListener('click', onDocClick)
  document.addEventListener('keydown', onKey)
  window.addEventListener('keydown', onGlobalKey)
})

onBeforeUnmount(() => {
  document.removeEventListener('click', onDocClick)
  document.removeEventListener('keydown', onKey)
  window.removeEventListener('keydown', onGlobalKey)
})

// ---- 外部定位（搜索结果 / AI） ----

watch(
  () => paneStore.revealTarget,
  (t) => {
    if (!t || t.tabId !== props.tab.id) return
    const idx = rows.value.findIndex((e) => e.path === t.path)
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
    closeCtx()
    cancelEdit()
  },
)

const hasEntries = computed(() => props.tab.entries.length > 0)
// 因「隐藏受保护的操作系统文件」而被清空的目录（无文本过滤时）
const emptyByProtect = computed(
  () => hasEntries.value && rows.value.length === 0 && !props.tab.filter.trim() && ui.hideProtected,
)
const emptyByFilter = computed(
  () => hasEntries.value && rows.value.length === 0 && !!props.tab.filter.trim(),
)
</script>

<template>
  <div ref="rootRef" class="file-list" :class="viewMode">
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
    <div v-else-if="!hasEntries && !editingMkdir" class="state">
      <Icon name="folder-open" :size="30" class="big-ico" />
      <p>此目录为空</p>
    </div>
    <div v-else-if="emptyByProtect && !editingMkdir" class="state">
      <Icon name="folder-open" :size="30" class="big-ico" />
      <p>已隐藏受保护的操作系统文件</p>
    </div>
    <div v-else-if="emptyByFilter && !editingMkdir" class="state">
      <Icon name="search" :size="30" class="big-ico" />
      <p>没有匹配「{{ tab.filter }}」的条目</p>
    </div>

    <!-- 详细 / 列表视图（虚拟滚动） -->
    <template v-else-if="viewMode !== 'icons'">
      <div v-if="viewMode === 'details'" class="header">
        <span class="col-ico" />
        <button class="col name" @click="paneStore.toggleSort(tab, 'name')">
          名称 <Icon v-if="tab.sortKey === 'name'" :name="tab.sortAsc ? 'chevron-up' : 'chevron-down'" :size="11" />
        </button>
        <button class="col size" @click="paneStore.toggleSort(tab, 'size')">
          大小 <Icon v-if="tab.sortKey === 'size'" :name="tab.sortAsc ? 'chevron-up' : 'chevron-down'" :size="11" />
        </button>
        <button class="col time" @click="paneStore.toggleSort(tab, 'modified')">
          修改时间 <Icon v-if="tab.sortKey === 'modified'" :name="tab.sortAsc ? 'chevron-up' : 'chevron-down'" :size="11" />
        </button>
        <button class="col type" @click="paneStore.toggleSort(tab, 'ext')">
          类型 <Icon v-if="tab.sortKey === 'ext'" :name="tab.sortAsc ? 'chevron-up' : 'chevron-down'" :size="11" />
        </button>
      </div>

      <div v-if="editing && editing.mode === 'mkdir'" class="inline-new">
        <span class="ico"><Icon name="folder" :size="14" /></span>
        <span class="nm">
          <input
            v-model="editing.name"
            class="inline-edit"
            spellcheck="false"
            @keydown.enter.stop.prevent="commitEdit"
            @keydown.esc.stop.prevent="cancelEdit"
            @blur="commitEdit"
            @click.stop
          />
        </span>
      </div>

      <div ref="scrollRef" class="fl-scroll" @click.self="clearSelection">
        <div class="fl-inner" :style="{ height: `${virtualizer.getTotalSize()}px` }">
          <div
            v-for="vi in virtualizer.getVirtualItems()"
            :key="String(vi.key)"
            class="row"
            :class="{ selected: tab.selected.includes(rows[vi.index].path), dir: rows[vi.index].isDir }"
            :style="{ height: `${vi.size}px`, transform: `translateY(${vi.start}px)` }"
            @click="onClick($event, vi.index, rows[vi.index])"
            @dblclick="onDblClick(rows[vi.index])"
            @contextmenu="openCtx($event, rows[vi.index])"
          >
            <span class="ico" :class="`k-${fileKind(rows[vi.index])}`">
              <Icon :name="fileIcon(rows[vi.index])" :size="14" />
            </span>
            <span v-if="editing && editing.mode === 'rename' && editing.path === rows[vi.index].path" class="nm">
              <input
                v-model="editing.name"
                class="inline-edit"
                spellcheck="false"
                @keydown.enter.stop.prevent="commitEdit"
                @keydown.esc.stop.prevent="cancelEdit"
                @blur="commitEdit"
                @click.stop
                @dblclick.stop
              />
            </span>
            <span v-else class="nm" :title="rows[vi.index].path">{{ ui.displayFileName(rows[vi.index].name, rows[vi.index].isDir) }}</span>
            <template v-if="viewMode === 'details'">
              <span class="size">{{ formatSize(rows[vi.index].size, rows[vi.index].isDir) }}</span>
              <span class="time">{{ formatTime(rows[vi.index].modified) }}</span>
              <span class="type">{{ typeLabel(rows[vi.index]) }}</span>
            </template>          </div>
        </div>
      </div>
    </template>

    <!-- 图标视图（限制渲染数量） -->
    <div v-else class="icons-scroll" @click.self="clearSelection">
      <div v-if="editing && editing.mode === 'mkdir'" class="icon-cell dir">
        <span class="icon-big k-folder"><Icon name="folder" :size="30" :stroke-width="1.4" /></span>
        <span class="icon-name">
          <input
            v-model="editing.name"
            class="inline-edit"
            spellcheck="false"
            @keydown.enter.stop.prevent="commitEdit"
            @keydown.esc.stop.prevent="cancelEdit"
            @blur="commitEdit"
            @click.stop
          />
        </span>
      </div>
      <div class="icons-grid">
        <div
          v-for="(entry, i) in iconRows"
          :key="entry.path"
          class="icon-cell"
          :class="{ selected: tab.selected.includes(entry.path), dir: entry.isDir }"
          :title="entry.path"
          @click="onClick($event, i, entry)"
          @dblclick="onDblClick(entry)"
          @contextmenu="openCtx($event, entry)"
        >
          <span class="icon-big" :class="`k-${fileKind(entry)}`">
            <Icon :name="fileIcon(entry)" :size="30" :stroke-width="1.4" />
          </span>
          <span v-if="editing && editing.mode === 'rename' && editing.path === entry.path" class="icon-name">
            <input
              v-model="editing.name"
              class="inline-edit"
              spellcheck="false"
              @keydown.enter.stop.prevent="commitEdit"
              @keydown.esc.stop.prevent="cancelEdit"
              @blur="commitEdit"
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

    <!-- 右键菜单 -->
    <div
      v-if="ctx"
      class="ctx-menu"
      :style="{ left: `${ctx.x}px`, top: `${ctx.y}px` }"
      @click.stop
      @contextmenu.prevent
    >
      <button @click="openEntry(ctx.entry); closeCtx()">打开</button>
      <div class="sep" />
      <button @click="copyToClipboard('copy')">复制</button>
      <button @click="copyToClipboard('move')">剪切</button>
      <button :disabled="!paneStore.clipboard" @click="paste">粘贴到此目录</button>
      <div class="sep" />
      <button @click="startRename(ctx.entry.path); closeCtx()">重命名</button>
      <button class="danger" @click="emit('delete', [ctx.entry.path]); closeCtx()">删除</button>
      <div class="sep" />
      <button @click="copyPath">复制路径</button>
    </div>
  </div>
</template>

<style scoped>
.file-list {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  background: var(--bg-app);
  overflow: hidden;
  position: relative;
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
  font-size: 12px;
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
.header {
  display: grid;
  grid-template-columns: 22px 1fr 90px 140px 90px;
  height: 24px;
  padding-left: 8px;
  background: var(--bg-panel);
  border-bottom: 1px solid var(--border);
  flex: none;
}
.header .col-ico {
  display: block;
}
.header .col {
  text-align: left;
  padding: 0 6px;
  font-size: 11px;
  color: var(--fg-muted);
  white-space: nowrap;
  overflow: hidden;
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
}
.fl-inner {
  position: relative;
  width: 100%;
}

/* 行内「新建文件夹」输入行（表头之下、列表之上） */
.inline-new {
  display: grid;
  grid-template-columns: 22px 1fr 90px 140px 90px;
  align-items: center;
  height: 26px;
  padding-left: 8px;
  flex: none;
  font-size: 12.5px;
  background: var(--bg-hover);
  border-bottom: 1px solid var(--border);
}
.file-list.list .inline-new {
  grid-template-columns: 22px 1fr;
}
.inline-new .ico {
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--ico-folder);
}

/* 行内编辑输入框 */
.inline-edit {
  width: 100%;
  height: 20px;
  padding: 0 4px;
  font: inherit;
  font-size: 12.5px;
  color: var(--fg);
  background: var(--bg-input);
  border: 1px solid var(--accent);
  border-radius: var(--radius-sm);
  outline: none;
}
.row {
  position: absolute;
  top: 0;
  left: 0;
  width: 100%;
  display: grid;
  grid-template-columns: 22px 1fr 90px 140px 90px;
  align-items: center;
  padding-left: 8px;
  font-size: 12.5px;
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
.row .ico {
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--fg-muted);
}
.row .nm {
  overflow: hidden;
  text-overflow: ellipsis;
}
.row.dir .nm {
  color: var(--ico-folder);
}
.row .size,
.row .time,
.row .type {
  padding: 0 6px;
  color: var(--fg-muted);
  font-size: 11.5px;
  overflow: hidden;
  text-overflow: ellipsis;
}

/* 列表视图：隐藏后三列 */
.file-list.list .row {
  grid-template-columns: 22px 1fr;
}

/* 图标视图 */
.icons-scroll {
  flex: 1;
  min-height: 0;
  overflow: auto;
  padding: 10px;
}
.icons-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(96px, 1fr));
  gap: 6px;
}
.icon-cell {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 4px;
  padding: 8px 4px;
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
.icon-big {
  display: flex;
  align-items: center;
  justify-content: center;
  height: 32px;
}
.icon-name {
  font-size: 11.5px;
  color: var(--fg);
  max-width: 100%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.icon-cell.dir .icon-name {
  color: var(--ico-folder);
}
.cap-hint {
  margin: 12px 0 4px;
  text-align: center;
  font-size: 11px;
  color: var(--fg-dim);
}

/* 右键菜单 */
.ctx-menu {
  position: fixed;
  z-index: 200;
  min-width: 168px;
  padding: 4px;
  background: var(--bg-elevated);
  border: 1px solid var(--border-strong);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-md);
  display: flex;
  flex-direction: column;
}
.ctx-menu button {
  text-align: left;
  padding: 5px 10px;
  border-radius: var(--radius);
  font-size: 12.5px;
  color: var(--fg);
}
.ctx-menu button:hover:not(:disabled) {
  background: var(--bg-hover);
}
.ctx-menu button:disabled {
  color: var(--fg-dim);
  cursor: default;
}
.ctx-menu button.danger {
  color: var(--danger);
}
.ctx-menu .sep {
  height: 1px;
  margin: 4px 6px;
  background: var(--border);
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

<script setup lang="ts">
// 文件夹同步对话框：左右目录对比 → 差异清单（可勾选）→ 批量执行。
// 三种模式：单向（只补/更新）、双向（较新者胜）、镜像（以源为准并删除目标多余项）。
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useVirtualizer } from '@tanstack/vue-virtual'
import type { FsProgress, SyncDirection, SyncEntry, SyncMode, SyncPlan, SyncResult } from '../../types'
import { api, events } from '../../api'
import { usePaneStore } from '../../stores/pane'
import { useUiStore } from '../../stores/ui'
import Icon from '../icons/Icon.vue'
import MacWindowLights from '../MacWindowLights.vue'
import { IS_MACOS } from '../../utils/platform'
import { formatSize } from './utils'

const pane = usePaneStore()
const ui = useUiStore()

const left = ref(pane.syncDirs.left ?? '')
const right = ref(pane.syncDirs.right ?? '')
const mode = ref<SyncMode>('oneWay')
const direction = ref<SyncDirection>('leftToRight')
const compareHash = ref(false)
const permanent = ref(false)

const scanning = ref(false)
const plan = ref<SyncPlan | null>(null)
const error = ref('')
/** 勾选项：entry.path 的小写形式（相对路径大小写不敏感）。 */
const selected = ref(new Set<string>())
const filter = ref<'all' | 'copy' | 'delete' | 'conflict'>('all')

const running = ref(false)
const opId = ref('')
const progress = ref<FsProgress | null>(null)
const result = ref<SyncResult | null>(null)

const modeOptions: { value: SyncMode; label: string }[] = [
  { value: 'oneWay', label: '单向' },
  { value: 'twoWay', label: '双向' },
  { value: 'mirror', label: '镜像' },
]
const filters: { value: typeof filter.value; label: string }[] = [
  { value: 'all', label: '全部' },
  { value: 'copy', label: '复制' },
  { value: 'delete', label: '删除' },
  { value: 'conflict', label: '冲突' },
]

function keyOf(entry: SyncEntry): string {
  return entry.path.toLowerCase()
}

const visibleEntries = computed<SyncEntry[]>(() => {
  const entries = plan.value?.entries ?? []
  switch (filter.value) {
    case 'copy':
      return entries.filter((entry) => entry.action === 'copyToRight' || entry.action === 'copyToLeft')
    case 'delete':
      return entries.filter((entry) => entry.action === 'deleteRight' || entry.action === 'deleteLeft')
    case 'conflict':
      return entries.filter((entry) => entry.action === 'conflict')
    default:
      return entries
  }
})

/** 可勾选（能直接执行）的条目：冲突项必须先指定方向。 */
function isActionable(entry: SyncEntry): boolean {
  return entry.action !== 'conflict' && entry.action !== 'equal'
}

const selectedCount = computed(() => selected.value.size)
const allSelected = computed(() => {
  const list = visibleEntries.value.filter(isActionable)
  return list.length > 0 && list.every((entry) => selected.value.has(keyOf(entry)))
})

const scrollRef = ref<HTMLElement | null>(null)
const virtualizer = useVirtualizer(
  computed(() => ({
    count: visibleEntries.value.length,
    getScrollElement: () => scrollRef.value,
    estimateSize: () => 26,
    overscan: 10,
  })),
)
const rows = computed(() =>
  virtualizer.value.getVirtualItems().flatMap((item) => {
    const entry = visibleEntries.value[item.index]
    return entry ? [{ key: String(item.key), start: item.start, entry }] : []
  }),
)

const percent = computed(() => {
  const value = progress.value
  if (!value) return 0
  if (value.bytesTotal > 0) return Math.min(100, Math.round((value.bytesDone / value.bytesTotal) * 100))
  if (value.filesTotal > 0) return Math.min(100, Math.round((value.filesDone / value.filesTotal) * 100))
  return 0
})

const progressDetail = computed(() => {
  const value = progress.value
  if (!value) return ''
  if (value.bytesTotal > 0) return `${formatSize(value.bytesDone)} / ${formatSize(value.bytesTotal)}`
  return `${value.filesDone} / ${value.filesTotal} 个文件`
})

const resultText = computed(() => {
  const value = result.value
  if (!value) return ''
  const head = value.cancelled ? '已取消' : value.ok ? '同步完成' : '同步结束'
  const errors = value.errors.length ? `，${value.errors.length} 个错误` : ''
  return `${head}：复制 ${value.copied} 项、删除 ${value.deleted} 项${errors}`
})

const hint = computed(() => {
  if (scanning.value) return '正在扫描两侧目录…'
  if (mode.value === 'mirror') return '镜像会删除目标侧多余项，默认移到回收站'
  if (mode.value === 'twoWay') return '双向不会删除任何一边，时间相同但内容不同的项需要手动指定方向'
  return '单向只补齐 / 更新目标，不删除目标多余项'
})

let scanSeq = 0
let scanTimer: number | null = null

function scheduleScan() {
  if (scanTimer !== null) window.clearTimeout(scanTimer)
  scanTimer = window.setTimeout(() => {
    scanTimer = null
    void scan()
  }, 250)
}

async function scan() {
  if (running.value) return
  const leftPath = left.value.trim()
  const rightPath = right.value.trim()
  if (!leftPath || !rightPath) {
    error.value = '请填写左右两个文件夹'
    return
  }
  const seq = ++scanSeq
  scanning.value = true
  error.value = ''
  try {
    const next = await api.syncScan({
      left: leftPath,
      right: rightPath,
      mode: mode.value,
      direction: direction.value,
      compare: compareHash.value ? 'hash' : 'meta',
    })
    if (seq !== scanSeq) return
    plan.value = next
    // 默认勾选所有可直接执行的差异项；冲突项留给用户决定。
    selected.value = new Set(
      next.entries.filter(isActionable).map((entry) => keyOf(entry)),
    )
  } catch (e) {
    if (seq !== scanSeq) return
    plan.value = null
    selected.value = new Set()
    error.value = String(e)
  } finally {
    if (seq === scanSeq) scanning.value = false
  }
}

function toggle(entry: SyncEntry) {
  const key = keyOf(entry)
  if (selected.value.has(key)) selected.value.delete(key)
  else selected.value.add(key)
}

function toggleAll() {
  if (allSelected.value) {
    for (const entry of visibleEntries.value) selected.value.delete(keyOf(entry))
  } else {
    for (const entry of visibleEntries.value) {
      if (isActionable(entry)) selected.value.add(keyOf(entry))
    }
  }
}

/** 冲突项：手动指定用哪一侧覆盖，并顺带勾选。 */
function resolve(entry: SyncEntry, action: 'copyToRight' | 'copyToLeft') {
  entry.action = action
  selected.value.add(keyOf(entry))
}

function swap() {
  const value = left.value
  left.value = right.value
  right.value = value
  void scan()
}

async function execute() {
  if (!plan.value || running.value) return
  const items = plan.value.entries
    .filter((entry) => selected.value.has(keyOf(entry)) && isActionable(entry))
    .map((entry) => ({
      path: entry.path,
      isDir: entry.isDir,
      size: entry.size,
      action: entry.action,
    }))
  if (!items.length) return

  opId.value = crypto.randomUUID()
  running.value = true
  progress.value = null
  result.value = null
  error.value = ''
  try {
    const res = await api.syncExecute(
      {
        left: left.value.trim(),
        right: right.value.trim(),
        permanent: permanent.value,
        items,
      },
      opId.value,
    )
    result.value = res
    ui.setStatusText(
      res.cancelled
        ? `同步已取消：复制 ${res.copied} 项，删除 ${res.deleted} 项`
        : res.ok
          ? `同步完成：复制 ${res.copied} 项，删除 ${res.deleted} 项`
          : `同步结束：${res.errors.length} 个错误`,
    )
  } catch (e) {
    error.value = String(e)
  } finally {
    running.value = false
    opId.value = ''
  }
  // 先解除 running 再重新扫描（scan 在 running 时会直接返回）。
  await pane.refreshBoth()
  await scan()
}

async function cancel() {
  if (!opId.value) return
  try {
    await api.fsCancel(opId.value)
  } catch (e) {
    error.value = String(e)
  }
}

function close() {
  if (running.value) return
  ui.closeSync()
}

function actionLabel(action: SyncEntry['action']): string {
  switch (action) {
    case 'copyToRight':
      return '左 → 右'
    case 'copyToLeft':
      return '右 → 左'
    case 'deleteRight':
      return '删除右侧'
    case 'deleteLeft':
      return '删除左侧'
    case 'conflict':
      return '冲突'
    default:
      return '相同'
  }
}

function onKeydown(event: KeyboardEvent) {
  if (event.isComposing) return
  if (event.key === 'Escape') {
    event.preventDefault()
    close()
  }
}

let unlisten: (() => void) | null = null

onMounted(async () => {
  window.addEventListener('keydown', onKeydown, true)
  unlisten = await events.onFsProgress((value) => {
    if (value.opId === opId.value) progress.value = value
  })
  void scan()
})

onBeforeUnmount(() => {
  window.removeEventListener('keydown', onKeydown, true)
  unlisten?.()
  if (scanTimer !== null) window.clearTimeout(scanTimer)
})

watch([mode, direction, compareHash], () => {
  scheduleScan()
})
</script>

<template>
  <div class="mask" @click.self="close">
    <section class="sync-dialog" role="dialog" aria-modal="true" aria-labelledby="sync-title" @mousedown.stop>
      <header class="sd-head">
        <MacWindowLights :disabled="running" @close="close" />
        <h3 id="sync-title">
          <Icon name="folder-sync" :size="15" class="head-icon" />
          文件夹同步
        </h3>
        <span v-if="plan" class="sd-total">
          {{ plan.stats.copy + plan.stats.delete + plan.stats.conflict }} 项差异
        </span>
        <button v-if="!IS_MACOS" class="sd-close" type="button" :disabled="running" aria-label="关闭" @click="close">
          <Icon name="x" :size="14" />
        </button>
      </header>

      <div class="sd-dirs">
        <label class="dir">
          <span class="dir-label">左侧</span>
          <input v-model="left" spellcheck="false" :disabled="running" @change="scan" />
        </label>
        <button class="swap" type="button" title="交换左右" :disabled="running" @click="swap">
          <Icon name="arrow-right-left" :size="14" />
        </button>
        <label class="dir">
          <span class="dir-label">右侧</span>
          <input v-model="right" spellcheck="false" :disabled="running" @change="scan" />
        </label>
      </div>

      <div class="sd-options">
        <div class="seg" role="group" aria-label="同步模式">
          <button
            v-for="option in modeOptions"
            :key="option.value"
            type="button"
            :class="{ on: mode === option.value }"
            :disabled="running"
            @click="mode = option.value"
          >
            {{ option.label }}
          </button>
        </div>
        <div v-if="mode !== 'twoWay'" class="seg" role="group" aria-label="同步方向">
          <button
            type="button"
            :class="{ on: direction === 'leftToRight' }"
            :disabled="running"
            @click="direction = 'leftToRight'"
          >
            左 → 右
          </button>
          <button
            type="button"
            :class="{ on: direction === 'rightToLeft' }"
            :disabled="running"
            @click="direction = 'rightToLeft'"
          >
            右 → 左
          </button>
        </div>
        <label class="chk">
          <input v-model="compareHash" type="checkbox" :disabled="running" />
          内容校验
        </label>
        <label v-if="mode === 'mirror'" class="chk">
          <input v-model="permanent" type="checkbox" :disabled="running" />
          永久删除
        </label>
      </div>

      <div v-if="plan" class="sd-stats">
        <span><b>{{ plan.stats.copy }}</b> 复制</span>
        <span><b>{{ plan.stats.delete }}</b> 删除</span>
        <span :class="{ warn: plan.stats.conflict > 0 }"><b>{{ plan.stats.conflict }}</b> 冲突</span>
        <span><b>{{ plan.stats.equal }}</b> 相同</span>
        <span class="bytes">待传 {{ formatSize(plan.stats.bytes) }}</span>
      </div>

      <div v-if="error" class="sd-error">{{ error }}</div>

      <div class="sd-list">
        <div class="sd-list-head">
          <label class="chk">
            <input type="checkbox" :checked="allSelected" :disabled="running || scanning" @change="toggleAll" />
            全选
          </label>
          <div class="seg small">
            <button
              v-for="option in filters"
              :key="option.value"
              type="button"
              :class="{ on: filter === option.value }"
              @click="filter = option.value"
            >
              {{ option.label }}
            </button>
          </div>
          <span class="sd-sel">已选 {{ selectedCount }} 项</span>
        </div>

        <div ref="scrollRef" class="sd-scroll">
          <div v-if="scanning" class="sd-empty">正在扫描…</div>
          <div v-else-if="!visibleEntries.length" class="sd-empty">没有差异</div>
          <div
            v-else
            class="sd-canvas"
            :style="{ height: `${virtualizer.getTotalSize()}px` }"
          >
            <div
              v-for="row in rows"
              :key="row.key"
              class="sd-row"
              :class="[row.entry.action, { on: selected.has(keyOf(row.entry)) }]"
              :style="{ transform: `translateY(${row.start}px)` }"
            >
              <input
                type="checkbox"
                :checked="selected.has(keyOf(row.entry))"
                :disabled="running || row.entry.action === 'conflict'"
                @change="toggle(row.entry)"
              />
              <Icon :name="row.entry.isDir ? 'folder' : 'file'" :size="13" class="kind" />
              <span class="path" :title="row.entry.path">{{ row.entry.path }}</span>
              <span v-if="row.entry.action === 'conflict'" class="resolve">
                <button type="button" :disabled="running" @click="resolve(row.entry, 'copyToRight')">用左</button>
                <button type="button" :disabled="running" @click="resolve(row.entry, 'copyToLeft')">用右</button>
              </span>
              <span class="badge" :class="row.entry.action">{{ actionLabel(row.entry.action) }}</span>
              <span class="size">{{ row.entry.isDir ? '—' : formatSize(row.entry.size) }}</span>
            </div>
          </div>
        </div>
      </div>

      <footer class="sd-foot">
        <div v-if="running" class="sd-progress">
          <div class="bar"><div class="fill" :style="{ width: `${percent}%` }" /></div>
          <span class="pct">{{ percent }}%</span>
          <span class="detail">{{ progressDetail }}</span>
          <button type="button" @click="cancel">取消</button>
        </div>
        <p v-else-if="result" class="sd-result">{{ resultText }}</p>
        <span v-else class="sd-hint">{{ hint }}</span>
        <button type="button" :disabled="scanning || running" @click="scan">重新扫描</button>
        <button
          type="button"
          class="primary"
          :disabled="!selectedCount || running || scanning"
          @click="execute"
        >
          {{ running ? '同步中…' : `开始同步（${selectedCount} 项）` }}
        </button>
      </footer>
    </section>
  </div>
</template>

<style scoped>
.mask {
  position: fixed;
  inset: 0;
  z-index: 330;
  display: flex;
  align-items: center;
  justify-content: center;
  background: var(--mask);
}
.sync-dialog {
  display: flex;
  flex-direction: column;
  gap: 10px;
  width: 780px;
  max-width: calc(100vw - 48px);
  max-height: calc(100vh - 64px);
  padding: 14px;
  border: 1px solid var(--border-strong);
  border-radius: var(--radius-lg);
  background: var(--bg-elevated);
  box-shadow: var(--shadow-lg);
}
.sd-head {
  display: flex;
  align-items: center;
  gap: 8px;
}
.sd-head h3 {
  display: flex;
  align-items: center;
  gap: 6px;
  margin: 0;
  font-size: 1.0833rem;
  font-weight: 600;
  color: var(--fg);
}
.head-icon {
  flex: none;
  color: var(--accent);
}
.sd-total {
  margin-left: auto;
  color: var(--fg-dim);
  font-size: 0.9167rem;
}
.sd-close {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 24px;
  height: 24px;
  border-radius: var(--radius-sm);
  color: var(--fg-muted);
}
.sd-close:hover:not(:disabled) {
  background: var(--bg-hover);
  color: var(--fg);
}
.sd-dirs {
  display: flex;
  align-items: center;
  gap: 6px;
}
.dir {
  display: flex;
  align-items: center;
  gap: 6px;
  flex: 1;
  min-width: 0;
}
.dir-label {
  flex: none;
  color: var(--fg-dim);
  font-size: 0.9167rem;
}
.dir input {
  flex: 1;
  min-width: 0;
  height: 27px;
  padding: 0 8px;
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  background: var(--bg-app);
  color: var(--fg);
  font-family: var(--font-mono, var(--font));
  font-size: 0.9167rem;
}
.dir input:focus {
  border-color: var(--accent);
  outline: none;
}
.swap {
  display: flex;
  align-items: center;
  justify-content: center;
  flex: none;
  width: 26px;
  height: 26px;
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  color: var(--fg-muted);
}
.swap:hover:not(:disabled) {
  background: var(--bg-hover);
  color: var(--fg);
}
.sd-options {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}
.seg {
  display: flex;
  padding: 2px;
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  background: var(--bg-app);
}
.seg button {
  height: 22px;
  padding: 0 10px;
  border-radius: calc(var(--radius-sm) - 1px);
  color: var(--fg-muted);
  font-size: 0.9167rem;
}
.seg button.on {
  background: var(--bg-active);
  color: var(--fg);
}
.seg.small button {
  height: 20px;
  padding: 0 8px;
  font-size: 0.8333rem;
}
.chk {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  color: var(--fg-muted);
  font-size: 0.9167rem;
  cursor: pointer;
}
.chk input {
  accent-color: var(--accent);
}
.sd-stats {
  display: flex;
  align-items: center;
  gap: 14px;
  padding: 6px 10px;
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  background: var(--bg-app);
  color: var(--fg-muted);
  font-size: 0.9167rem;
}
.sd-stats b {
  color: var(--fg);
  font-variant-numeric: tabular-nums;
}
.sd-stats .warn b {
  color: var(--warn);
}
.sd-stats .bytes {
  margin-left: auto;
  color: var(--fg-dim);
}
.sd-error {
  padding: 6px 10px;
  border: 1px solid var(--danger);
  border-radius: var(--radius-sm);
  background: var(--danger-soft);
  color: var(--danger);
  font-size: 0.9167rem;
}
.sd-list {
  display: flex;
  flex-direction: column;
  min-height: 0;
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  background: var(--bg-app);
}
.sd-list-head {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 5px 8px;
  border-bottom: 1px solid var(--border);
}
.sd-sel {
  margin-left: auto;
  color: var(--fg-dim);
  font-size: 0.8333rem;
}
.sd-scroll {
  position: relative;
  height: 300px;
  overflow: auto;
}
.sd-empty {
  display: flex;
  align-items: center;
  justify-content: center;
  height: 100%;
  color: var(--fg-dim);
  font-size: 0.9167rem;
}
.sd-canvas {
  position: relative;
  width: 100%;
}
.sd-row {
  position: absolute;
  top: 0;
  left: 0;
  display: flex;
  align-items: center;
  gap: 7px;
  width: 100%;
  height: 26px;
  padding: 0 8px;
  color: var(--fg);
  font-size: 0.9167rem;
  white-space: nowrap;
}
.sd-row:hover {
  background: var(--bg-hover);
}
.sd-row.on {
  background: var(--bg-active);
}
.sd-row input[type='checkbox'] {
  flex: none;
  accent-color: var(--accent);
}
.kind {
  flex: none;
  color: var(--fg-dim);
}
.path {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
}
.badge {
  flex: none;
  padding: 0 6px;
  border-radius: 999px;
  background: var(--bg-elevated);
  color: var(--fg-dim);
  font-size: 0.75rem;
  line-height: 17px;
}
.badge.copyToRight,
.badge.copyToLeft {
  color: var(--accent);
  background: color-mix(in srgb, var(--accent) 12%, transparent);
}
.badge.deleteRight,
.badge.deleteLeft {
  color: var(--danger);
  background: var(--danger-soft);
}
.badge.conflict {
  color: var(--warn);
  background: color-mix(in srgb, var(--warn) 14%, transparent);
}
.size {
  flex: none;
  width: 76px;
  color: var(--fg-dim);
  font-variant-numeric: tabular-nums;
  text-align: right;
}
.resolve {
  display: flex;
  gap: 3px;
  flex: none;
}
.resolve button {
  height: 18px;
  padding: 0 6px;
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  background: var(--bg-elevated);
  color: var(--fg-muted);
  font-size: 0.75rem;
}
.resolve button:hover:not(:disabled) {
  border-color: var(--accent);
  color: var(--accent);
}
.sd-foot {
  display: flex;
  align-items: center;
  gap: 8px;
}
.sd-foot > button {
  height: 27px;
  padding: 0 12px;
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  background: var(--bg-app);
  color: var(--fg);
  font-size: 0.9167rem;
}
.sd-foot > button:hover:not(:disabled) {
  background: var(--bg-hover);
}
.sd-foot > button.primary {
  border-color: var(--accent);
  background: var(--accent);
  color: var(--accent-fg);
}
.sd-foot > button.primary:hover:not(:disabled) {
  background: color-mix(in srgb, var(--accent) 86%, var(--fg));
}
.sd-foot > button:disabled {
  opacity: 0.45;
  cursor: default;
}
.sd-hint {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  color: var(--fg-dim);
  font-size: 0.8333rem;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.sd-result {
  flex: 1;
  margin: 0;
  color: var(--fg-muted);
  font-size: 0.8333rem;
}
.sd-progress {
  display: flex;
  align-items: center;
  gap: 8px;
  flex: 1;
  min-width: 0;
}
.bar {
  flex: 1;
  min-width: 0;
  height: 6px;
  overflow: hidden;
  border-radius: 999px;
  background: var(--bg-active);
}
.fill {
  height: 100%;
  border-radius: 999px;
  background: var(--accent);
  transition: width var(--t-fast);
}
.pct {
  flex: none;
  width: 34px;
  color: var(--fg);
  font-size: 0.8333rem;
  font-variant-numeric: tabular-nums;
  text-align: right;
}
.detail {
  flex: none;
  color: var(--fg-dim);
  font-size: 0.8333rem;
}
.sd-progress button {
  height: 22px;
  padding: 0 10px;
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  background: var(--bg-app);
  color: var(--fg);
  font-size: 0.8333rem;
}
</style>

<script setup lang="ts">
// 全局搜索栏（Everything 式，走索引引擎）（WS-D）
import { computed, nextTick, onMounted, ref, watch } from 'vue'
import { useSearchStore } from '../stores/search'
import { usePaneStore } from '../stores/pane'
import { useUiStore } from '../stores/ui'
import { SEARCH_CATEGORIES } from '../searchFilters'
import Icon from './icons/Icon.vue'

const search = useSearchStore()
const paneStore = usePaneStore()
const ui = useUiStore()

const text = ref('')
const scope = ref<'global' | 'current'>(ui.searchScope)
const open = ref(false)
const inputEl = ref<HTMLInputElement | null>(null)
const activeTab = computed(() => paneStore.activeTab)

/** 聚焦输入框并选中已有内容（再次按 Ctrl+F 时可直接覆盖输入） */
function focusInput() {
  void nextTick(() => {
    const el = inputEl.value
    if (!el) return
    el.focus()
    el.select()
  })
}

/** 消费一次性聚焦标记：为真则聚焦并清除 */
function consumeFocusRequest() {
  if (!ui.searchFocusPending) return
  ui.searchFocusPending = false
  focusInput()
}

// 面板已挂载时再次触发快捷键（如 Ctrl+F）也能把焦点交回输入框
watch(() => ui.searchFocusRequest, consumeFocusRequest)

// 索引状态（统一在此展示：条目数 / 已扫描 / 状态）
const indexText = computed(() =>
  ui.scanning
    ? `${ui.scanned.toLocaleString()} / ${ui.indexTotal.toLocaleString()}`
    : `索引 ${ui.indexTotal.toLocaleString()} 项`,
)
const indexTitle = computed(
  () => `索引条目 ${ui.indexTotal.toLocaleString()}，已扫描 ${ui.scanned.toLocaleString()}，${ui.scanning ? '扫描中' : '空闲'}`,
)

const placeholder = computed(() => (scope.value === 'global' ? '搜索文件…' : '筛选当前目录…'))
const syntaxHint = computed(() =>
  scope.value === 'global' ? '支持 ext:pdf、size:>100mb 等语法' : '在当前目录内即时过滤',
)
const resultMeta = computed(() => {
  if (scope.value !== 'global' || search.searching || !search.searched) return ''
  return `${search.hits.length} 项 · ${Math.round(search.elapsedMs)} ms`
})

watch(
  () => [scope.value, paneStore.activePaneId, activeTab.value?.id],
  () => {
    // 切换标签 / 窗格 / 范围时清掉旧的目录过滤，避免串台；新的过滤等「搜索」/Enter 再应用
    paneStore.panes.forEach((pane) =>
      pane.tabs.forEach((tab) => {
        tab.filter = ''
        tab.typeFilter = null
      }),
    )
  },
)

let requestId = 0
let resultTabId: string | undefined

async function runGlobalSearch(value: string) {
  const trimmed = value.trim()
  // 无关键词时，只要选了类型筛选也照常查询（仅按条件过滤）
  if (!trimmed && !search.typeKey) return
  const currentRequest = ++requestId
  await search.run(trimmed)
  if (currentRequest !== requestId || scope.value !== 'global') return
  if (search.error) {
    ui.statusText = `搜索失败：${search.error}`
    return
  }
  const title = search.typeLabel ? `搜索结果 · ${search.typeLabel}` : '搜索结果'
  resultTabId = paneStore.showSearchResults(trimmed, search.hits, resultTabId, title)
}

/** 点击类型标签：只改选择，结果等「搜索」/ Enter 再更新；再次点击已选中的回到「全部」 */
function setType(key: string) {
  search.setType(search.typeKey === key || key === 'all' ? null : key)
}

function clearAll() {
  requestId += 1
  text.value = ''
  search.setType(null)
  if (scope.value === 'current' && activeTab.value) {
    activeTab.value.filter = ''
    activeTab.value.typeFilter = null
  }
  search.clear()
  // 清空后保持焦点，方便直接输入下一次关键词
  focusInput()
}

function setScope(next: 'global' | 'current') {
  if (scope.value === next) return
  scope.value = next
  ui.searchScope = next
  requestId += 1
  search.clear()
  paneStore.panes.forEach((pane) =>
    pane.tabs.forEach((tab) => {
      tab.filter = ''
      tab.typeFilter = null
    }),
  )
  open.value = false
}

watch(
  () => ui.searchCurrentRequest,
  () => {
    text.value = ''
    search.clear()
    setScope('current')
    // 已经在「当前目录」范围时 setScope 会提前返回，这里补一次过滤清理
    if (activeTab.value) {
      activeTab.value.filter = ''
      activeTab.value.typeFilter = null
    }
    consumeFocusRequest()
  },
)

// 面板重新挂载（如收起后经搜索入口再次打开）时也要聚焦
onMounted(consumeFocusRequest)

/** 输入法组合输入时按 Enter 是在确认候选词，不应触发搜索 */
function onEnter(event: KeyboardEvent) {
  if (event.isComposing) return
  void submit()
}

function onEscape() {
  open.value = false
  inputEl.value?.blur()
}

/** 「搜索」按钮与 Enter 共用的提交入口 */
async function submit() {
  if (scope.value === 'current') {
    requestId += 1
    if (activeTab.value) {
      activeTab.value.filter = text.value
      activeTab.value.typeFilter = search.typeKey
    }
    search.clear()
    return
  }
  await runGlobalSearch(text.value)
}
</script>

<template>
  <div class="search-wrap">
    <div class="search-controls">
      <div class="scope-switch" role="group" aria-label="搜索范围">
        <button
          type="button"
          :class="{ on: scope === 'global' }"
          :aria-pressed="scope === 'global'"
          @click="setScope('global')"
        >
          全局
        </button>
        <button
          type="button"
          :class="{ on: scope === 'current' }"
          :aria-pressed="scope === 'current'"
          @click="setScope('current')"
        >
          当前目录
        </button>
      </div>

      <div class="search-row">
        <div class="search-field" :class="{ focused: open }">
          <Icon name="search" class="field-icon" :size="15" />
          <input
            ref="inputEl"
            v-model="text"
            :placeholder="placeholder"
            :title="scope === 'global' ? '支持 ext:pdf、size:>100mb 等语法' : '在当前目录内按名称过滤'"
            spellcheck="false"
            aria-label="搜索关键词"
            @focus="open = true"
            @blur="open = false"
            @keydown.enter="onEnter"
            @keydown.esc="onEscape"
          />
          <span v-if="scope === 'global' && search.searching" class="spinner" />
          <button
            v-if="text"
            type="button"
            class="clear"
            title="清空"
            aria-label="清空搜索"
            @mousedown.prevent
            @click="clearAll"
          >
            <Icon name="x" :size="12" />
          </button>
        </div>
        <button
          type="button"
          class="go"
          title="搜索（Enter）"
          aria-label="搜索"
          :disabled="scope === 'global' && !text.trim() && !search.typeKey"
          @mousedown.prevent
          @click="submit"
        >
          <Icon name="arrow-right" :size="16" />
        </button>
      </div>

      <div class="search-meta">
        <span class="hint">{{ syntaxHint }}</span>
        <span v-if="resultMeta" class="meta">{{ resultMeta }}</span>
      </div>
    </div>

    <div class="type-filter" role="group" aria-label="按文件类型筛选">
      <div class="filter-head">
        <span class="type-label">类型</span>
        <button
          v-if="search.typeKey"
          type="button"
          class="reset"
          title="清除类型筛选"
          @click="setType('all')"
        >
          清除
        </button>
      </div>
      <div class="chips">
        <button
          v-for="c in SEARCH_CATEGORIES"
          :key="c.key"
          type="button"
          class="chip"
          :class="{ on: (search.typeKey ?? 'all') === c.key }"
          :aria-pressed="(search.typeKey ?? 'all') === c.key"
          :title="c.key === 'all' ? '不按类型筛选' : `只显示${c.label}`"
          @click="setType(c.key)"
        >
          {{ c.label }}
        </button>
      </div>
    </div>

    <footer class="search-status" :title="indexTitle" aria-label="索引状态">
      <span class="dot" :class="{ scanning: ui.scanning }" />
      <span class="txt">{{ indexText }}</span>
      <span class="spacer" />
      <span class="state" :class="{ busy: ui.scanning }">{{ ui.scanning ? '扫描中' : '空闲' }}</span>
    </footer>
  </div>
</template>

<style scoped>
.search-wrap {
  position: relative;
  /* 内容超出可用高度时由侧栏滚动，而不是压扁类型标签 */
  flex: 1 0 auto;
  min-height: 0;
  display: flex;
  flex-direction: column;
  background: var(--sidebar-bg);
}

/* ---- 搜索控件区 ---- */
.search-controls {
  flex: none;
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 10px 10px 8px;
}

/* 范围切换：轻量分段控件 */
.scope-switch {
  display: flex;
  gap: 2px;
  padding: 2px;
  border-radius: 10px;
  background: var(--bg-hover);
}
.scope-switch button {
  flex: 1;
  height: 26px;
  padding: 0 8px;
  border-radius: 8px;
  color: var(--fg-dim);
  font-size: 0.9167em;
  font-weight: 500;
  white-space: nowrap;
}
.scope-switch button:hover {
  color: var(--fg);
}
.scope-switch button.on {
  background: var(--accent-soft);
  color: var(--fg);
  font-weight: 600;
}

/* 输入框 + 提交按钮 */
.search-row {
  display: flex;
  align-items: center;
  gap: 6px;
}
.search-field {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: center;
  gap: 6px;
  height: 34px;
  padding: 0 8px 0 10px;
  border: 1px solid var(--border);
  border-radius: 10px;
  background: var(--bg-panel);
  transition:
    border-color var(--t-fast),
    background var(--t-fast),
    box-shadow var(--t-fast);
}
.search-field:hover {
  border-color: var(--border-strong);
}
.search-field.focused {
  border-color: var(--border-strong);
  background: var(--bg-hover);
  box-shadow: 0 0 0 3px var(--accent-soft);
}
.field-icon {
  color: var(--fg-dim);
  transition: color var(--t-fast);
}
.search-field.focused .field-icon {
  color: var(--fg-muted);
}
.search-field input {
  flex: 1;
  min-width: 0;
  height: 100%;
  padding: 0;
  border: none;
  border-radius: 0;
  background: transparent;
  box-shadow: none;
  color: var(--fg);
  font: inherit;
  outline: none;
}
.search-field input:focus {
  border: none;
  box-shadow: none;
}
.search-field input::placeholder {
  color: var(--fg-dim);
}
.clear {
  display: grid;
  place-items: center;
  width: 20px;
  height: 20px;
  padding: 0;
  border-radius: 5px;
  color: var(--fg-dim);
}
.clear:hover {
  background: var(--bg-active);
  color: var(--fg);
}
.go {
  flex: none;
  display: grid;
  place-items: center;
  width: 34px;
  height: 34px;
  border-radius: 10px;
  background: var(--accent);
  color: var(--accent-fg);
  transition:
    opacity var(--t-fast),
    background var(--t-fast),
    color var(--t-fast);
}
.go:hover:not(:disabled) {
  opacity: 0.86;
}
.go:disabled {
  background: var(--bg-active);
  color: var(--fg-dim);
  opacity: 0.6;
  cursor: default;
}

/* 辅助提示 + 结果统计 */
.search-meta {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 6px;
  min-height: 14px;
  padding: 0 2px;
  color: var(--fg-muted);
  font-size: 0.8333em;
  line-height: 1.35;
}
.search-meta .hint {
  min-width: 0;
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}
.search-meta .meta {
  flex: none;
  color: var(--fg-muted);
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}

/* ---- 类型筛选 ---- */
.type-filter {
  flex: none;
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 10px;
  border-top: 1px solid var(--border);
}
.filter-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 0 2px;
}
.type-label {
  color: var(--fg-muted);
  font-size: 0.8333em;
  font-weight: 600;
  letter-spacing: 0.08em;
}
.reset {
  padding: 0 2px;
  border-radius: 4px;
  color: var(--fg-muted);
  font-size: 0.8333em;
}
.reset:hover {
  color: var(--fg);
}
.chips {
  display: flex;
  flex-wrap: wrap;
  gap: 5px;
}
.chip {
  height: 24px;
  padding: 0 9px;
  border: 1px solid transparent;
  border-radius: 7px;
  background: var(--bg-hover);
  color: var(--fg-muted);
  font-size: 0.875em;
  white-space: nowrap;
}
.chip:hover {
  background: var(--bg-active);
  color: var(--fg);
}
.chip.on {
  background: var(--accent-soft);
  border-color: var(--border-strong);
  color: var(--fg);
  font-weight: 600;
}

.spinner {
  flex: none;
  width: 13px;
  height: 13px;
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

/* ---- 底部索引状态栏（索引状态统一在此展示）---- */
.search-status {
  margin-top: auto;
  flex: none;
  height: var(--status-bar-h);
  display: flex;
  align-items: center;
  gap: 7px;
  padding: 0 10px;
  border-top: 1px solid var(--border);
  background: var(--bg-panel);
  font-size: 0.8333em;
  color: var(--fg-muted);
  overflow: hidden;
}
.search-status .spacer {
  flex: 1;
}
.search-status .dot {
  flex: none;
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--ok);
  box-shadow: 0 0 0 3px var(--ok-soft);
}
.search-status .dot.scanning {
  background: var(--warn);
  box-shadow: 0 0 0 3px var(--warn-soft);
  animation: pulse 1s ease-in-out infinite;
}
.search-status .txt {
  min-width: 0;
  font-family: var(--mono);
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.search-status .state {
  flex: none;
  color: var(--fg-dim);
}
.search-status .state.busy {
  color: var(--warn);
}
@keyframes pulse {
  0%,
  100% {
    opacity: 1;
  }
  50% {
    opacity: 0.25;
  }
}
</style>

<script setup lang="ts">
// 全局搜索栏（Everything 式，走索引引擎）（WS-D）
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { useSearchStore } from '../stores/search'
import { usePaneStore } from '../stores/pane'
import { useUiStore } from '../stores/ui'
import { SEARCH_CATEGORIES } from '../searchFilters'
import { basename } from './Pane/utils'
import Icon from './icons/Icon.vue'
import type { IconName } from './icons/paths'

const search = useSearchStore()
const paneStore = usePaneStore()
const ui = useUiStore()

const text = ref('')
const scope = ref<'global' | 'current'>(ui.searchScope)
const open = ref(false)
const typeMenuOpen = ref(false)
const inputEl = ref<HTMLInputElement | null>(null)
const typePickerEl = ref<HTMLElement | null>(null)
const typeBtnEl = ref<HTMLButtonElement | null>(null)
const activeTab = computed(() => paneStore.activeTab)

/** 聚焦输入框；默认选中已有内容（再次按 Ctrl+F 时可直接覆盖输入） */
function focusInput(select = true) {
  void nextTick(() => {
    const el = inputEl.value
    if (!el) return
    el.focus()
    if (select) el.select()
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

/** 索引状态（统一在此展示：条目数 / 已扫描 / 状态） */
const isWindows = typeof navigator !== 'undefined' && /Windows/i.test(navigator.userAgent)
const indexText = computed(() => {
  if (isWindows && ui.usingEverything) return 'Everything 索引'
  if (isWindows && ui.waitingForEverything) return '正在连接 Everything…'
  return ui.scanning ? `正在扫描 ${ui.scanned.toLocaleString()} 项` : `索引 ${ui.indexTotal.toLocaleString()} 项`
})
const indexTitle = computed(
  () => (isWindows && ui.usingEverything)
    ? '正在使用本机 Everything 的实时索引'
    : (isWindows && ui.waitingForEverything)
      ? '正在等待 Everything 启动，不会扫描本地目录'
      : `索引条目 ${ui.indexTotal.toLocaleString()}，已扫描 ${ui.scanned.toLocaleString()}，${ui.scanning ? '扫描中' : '空闲'}`,
)
/** 扫描进度（仅在已知总量时展示百分比） */
const scanPercent = computed(() => {
  if (!ui.scanning || ui.indexTotal <= 0) return ''
  return `${Math.min(100, Math.round((ui.scanned / ui.indexTotal) * 100))}%`
})

const placeholder = computed(() => (scope.value === 'global' ? '搜索文件…' : '筛选当前目录…'))
const syntaxHint = computed(() =>
  scope.value === 'global' ? 'Enter 搜索 · 支持 ext: size: 语法' : 'Enter 应用筛选',
)
const resultMeta = computed(() => {
  if (scope.value !== 'global' || search.searching || !search.searched) return ''
  return `${search.hits.length} 项 · ${Math.round(search.elapsedMs)} ms`
})
/** 输入框右侧的提交按钮：仅在有可执行条件时出现 */
const canSearch = computed(() =>
  scope.value === 'global' ? !!(text.value.trim() || search.typeKey) : !!text.value.trim(),
)
/** 没有输入、也没有筛选条件时展示引导区 */
const showEmpty = computed(() => !text.value.trim() && !search.typeKey && !search.searching)

const currentPath = computed(() => activeTab.value?.path ?? '')
const currentPathLabel = computed(() => {
  const name = currentPath.value ? basename(currentPath.value) : ''
  // 盘根 basename 会得到 `C:`，补回分隔符更符合路径习惯
  return /^[A-Za-z]:$/.test(name) ? `${name}\\` : name
})

// ---- 类型筛选 ----

interface CategoryVisual {
  icon?: IconName
  badge?: string
  color: string
}

const CATEGORY_VISUALS: Record<string, CategoryVisual> = {
  all: { icon: 'view-icons', color: 'var(--fg-dim)' },
  folder: { icon: 'folder', color: 'var(--ico-folder)' },
  doc: { icon: 'file-text', color: 'var(--ico-doc)' },
  image: { icon: 'image', color: 'var(--ico-image)' },
  audio: { icon: 'music', color: 'var(--ico-audio)' },
  video: { icon: 'video', color: 'var(--ico-video)' },
  archive: { icon: 'archive', color: 'var(--ico-archive)' },
  app: { icon: 'executable', color: 'var(--ico-app)' },
  code: { icon: 'code', color: 'var(--ico-code)' },
  word: { badge: 'W', color: '#5b9bef' },
  excel: { badge: 'X', color: '#48b57d' },
  ppt: { badge: 'P', color: '#e3805c' },
  pdf: { badge: 'PDF', color: '#ef6b70' },
  markdown: { badge: 'MD', color: 'var(--fg-muted)' },
}
const FALLBACK_VISUAL: CategoryVisual = { icon: 'file', color: 'var(--fg-muted)' }

function visualOf(key: string): CategoryVisual {
  return CATEGORY_VISUALS[key] ?? FALLBACK_VISUAL
}

// 办公文档单独成组，避免和通用分类混在一起显得杂乱
const OFFICE_KEYS = new Set(['word', 'excel', 'ppt', 'pdf', 'markdown'])
const TYPE_GROUPS = [
  SEARCH_CATEGORIES.filter((c) => !OFFICE_KEYS.has(c.key)),
  SEARCH_CATEGORIES.filter((c) => OFFICE_KEYS.has(c.key)),
].map((group) => group.map((c) => ({ ...c, visual: visualOf(c.key) })))

const activeVisual = computed(() => (search.typeKey ? visualOf(search.typeKey) : null))

const syntaxExamples = [
  { label: '按类型', value: 'ext:pdf' },
  { label: '按大小', value: 'size:>100mb' },
  { label: '按时间', value: 'modified:today' },
]

/** 点击语法示例：填入并立即检索 */
async function applyExample(value: string) {
  text.value = value
  await submit()
  focusInput(false)
}

function toggleTypeMenu() {
  typeMenuOpen.value = !typeMenuOpen.value
}

function closeTypeMenu() {
  typeMenuOpen.value = false
}

function onDocPointerDown(event: PointerEvent) {
  if (!typeMenuOpen.value) return
  const el = typePickerEl.value
  if (el && !el.contains(event.target as Node)) closeTypeMenu()
}

watch(
  () => [scope.value, paneStore.activePaneId, activeTab.value?.id],
  () => {
    // 切换标签 / 窗格 / 范围时清掉旧的目录过滤，避免串台；新的过滤等「搜索」/Enter 再应用
    paneStore.panes.forEach((pane) =>
      pane.tabs.forEach((tab) => paneStore.clearTabFilter(tab)),
    )
    closeTypeMenu()
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

/**
 * 选择类型：点击已选中的分类会回到「全部」。
 * 全局搜索下立即刷新结果；当前目录下立即应用到文件列表，免去再按一次 Enter。
 */
async function setType(key: string) {
  const next = !key || key === 'all' || search.typeKey === key ? null : key
  const changed = next !== search.typeKey
  typeMenuOpen.value = false
  search.setType(next)
  if (!changed) return
  if (scope.value === 'current') {
    if (activeTab.value) {
      paneStore.setSimpleFilter(activeTab.value, activeTab.value.filter, next)
    }
    return
  }
  if (!next && !text.value.trim()) {
    // 只剩「全部」条件：清掉上一轮结果统计，回到引导态
    search.clear()
    return
  }
  await runGlobalSearch(text.value)
}

function pickType(key: string) {
  void setType(key)
  focusInput(false)
}

/** 类型下拉的键盘关闭：焦点回到触发按钮 */
function onTypeEscape() {
  closeTypeMenu()
  typeBtnEl.value?.focus()
}

function clearAll() {
  requestId += 1
  text.value = ''
  search.setType(null)
  closeTypeMenu()
  if (scope.value === 'current' && activeTab.value) {
    paneStore.clearTabFilter(activeTab.value)
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
  closeTypeMenu()
  search.clear()
  paneStore.panes.forEach((pane) =>
    pane.tabs.forEach((tab) => paneStore.clearTabFilter(tab)),
  )
  open.value = false
}

watch(
  () => ui.searchCurrentRequest,
  () => {
    text.value = ''
    search.clear()
    closeTypeMenu()
    setScope('current')
    // 已经在「当前目录」范围时 setScope 会提前返回，这里补一次过滤清理
    if (activeTab.value) {
      paneStore.clearTabFilter(activeTab.value)
    }
    consumeFocusRequest()
  },
)

// 面板重新挂载（如收起后经搜索入口再次打开）时也要聚焦
onMounted(() => {
  document.addEventListener('pointerdown', onDocPointerDown, true)
  consumeFocusRequest()
})
onUnmounted(() => document.removeEventListener('pointerdown', onDocPointerDown, true))

/** 输入法组合输入时按 Enter 是在确认候选词，不应触发搜索 */
function onEnter(event: KeyboardEvent) {
  if (event.isComposing) return
  void submit()
}

function onEscape() {
  if (typeMenuOpen.value) {
    closeTypeMenu()
    return
  }
  open.value = false
  inputEl.value?.blur()
}

/** 「搜索」按钮与 Enter 共用的提交入口 */
async function submit() {
  closeTypeMenu()
  if (scope.value === 'current') {
    requestId += 1
    if (activeTab.value) {
      paneStore.setSimpleFilter(activeTab.value, text.value, search.typeKey)
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
      <div class="search-field" :class="{ focused: open }">
        <Icon name="search" class="field-icon" :size="16" />
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
        <span v-if="scope === 'global' && search.searching" class="spinner" aria-hidden="true" />
        <template v-else>
          <button
            v-if="text"
            type="button"
            class="field-btn clear"
            title="清空"
            aria-label="清空搜索"
            @mousedown.prevent
            @click="clearAll"
          >
            <Icon name="x" :size="12" />
          </button>
          <button
            v-if="canSearch"
            type="button"
            class="field-btn go"
            title="搜索（Enter）"
            aria-label="搜索"
            @mousedown.prevent
            @click="submit"
          >
            <Icon name="arrow-right" :size="14" />
          </button>
        </template>
      </div>

      <div class="toolbar">
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

        <div ref="typePickerEl" class="type-picker" @keydown.esc="onTypeEscape">
          <button
            ref="typeBtnEl"
            type="button"
            class="type-btn"
            :class="{ on: !!search.typeKey, open: typeMenuOpen }"
            title="按文件类型筛选"
            aria-haspopup="true"
            :aria-expanded="typeMenuOpen"
            @mousedown.prevent
            @click="toggleTypeMenu"
          >
            <span>类型</span>
            <Icon name="chevron-down" :size="13" class="chev" />
          </button>

          <transition name="pop">
            <div v-if="typeMenuOpen" class="type-menu" role="group" aria-label="按文件类型筛选">
              <div class="type-menu-head">
                <span class="t">文件类型</span>
                <button
                  v-if="search.typeKey"
                  type="button"
                  class="reset"
                  @mousedown.prevent
                  @click="pickType('all')"
                >
                  重置
                </button>
              </div>
              <div class="type-grid">
                <template v-for="(group, gi) in TYPE_GROUPS" :key="gi">
                  <div v-if="gi > 0" class="type-sep" />
                  <button
                    v-for="c in group"
                    :key="c.key"
                    type="button"
                    class="type-item"
                    :class="{ on: (search.typeKey ?? 'all') === c.key }"
                    :aria-pressed="(search.typeKey ?? 'all') === c.key"
                    @mousedown.prevent
                    @click="pickType(c.key)"
                  >
                    <Icon
                      v-if="c.visual.icon"
                      :name="c.visual.icon ?? 'file'"
                      :size="15"
                      class="ti-icon"
                      :style="{ color: c.visual.color }"
                    />
                    <span v-else class="ti-badge" :style="{ color: c.visual.color }">
                      {{ c.visual.badge }}
                    </span>
                    <span class="ti-label">{{ c.label }}</span>
                    <Icon
                      v-if="(search.typeKey ?? 'all') === c.key"
                      name="check"
                      :size="13"
                      class="ti-check"
                    />
                  </button>
                </template>
              </div>
            </div>
          </transition>
        </div>
      </div>

      <div class="search-meta">
        <button
          v-if="search.typeKey"
          type="button"
          class="active-filter"
          :title="`清除类型筛选：${search.typeLabel}`"
          @click="setType('all')"
        >
          <Icon
            v-if="activeVisual?.icon"
            :name="activeVisual.icon ?? 'file'"
            :size="13"
            class="af-icon"
            :style="{ color: activeVisual.color }"
          />
          <span v-else class="af-badge" :style="{ color: activeVisual?.color }">
            {{ activeVisual?.badge }}
          </span>
          <span class="af-label">{{ search.typeLabel }}</span>
          <Icon name="x" :size="11" class="af-x" />
        </button>
        <span v-else class="hint">{{ syntaxHint }}</span>
        <span v-if="resultMeta" class="meta">{{ resultMeta }}</span>
      </div>
    </div>

    <div class="search-body">
      <div v-if="showEmpty" class="empty">
        <div class="empty-icon" :class="{ folder: scope === 'current' }">
          <Icon :name="scope === 'current' ? 'folder' : 'search'" :size="18" />
        </div>
        <p class="empty-title">{{ scope === 'global' ? '搜索这台电脑' : '筛选当前目录' }}</p>
        <p class="empty-sub">
          {{ scope === 'global' ? '输入文件名，或组合语法精确筛选' : '输入关键词，仅在当前文件夹内过滤' }}
        </p>

        <div v-if="scope === 'current' && currentPathLabel" class="scope-path" :title="currentPath">
          <Icon name="folder" :size="12" />
          <span>{{ currentPathLabel }}</span>
        </div>

        <div v-else-if="scope === 'global'" class="syntax-card">
          <div class="syntax-head">
            <span>快捷语法</span>
            <span class="note">点击立即搜索</span>
          </div>
          <button
            v-for="ex in syntaxExamples"
            :key="ex.value"
            type="button"
            class="syntax-row"
            @mousedown.prevent
            @click="applyExample(ex.value)"
          >
            <span class="name">{{ ex.label }}</span>
            <code>{{ ex.value }}</code>
          </button>
        </div>
      </div>
    </div>

    <footer class="search-status" :title="indexTitle" aria-label="索引状态">
      <span class="dot" :class="{ scanning: ui.scanning }" />
      <span class="txt">{{ indexText }}</span>
      <span class="spacer" />
      <span v-if="scanPercent" class="state busy">{{ scanPercent }}</span>
    </footer>
  </div>
</template>

<style scoped>
.search-wrap {
  position: relative;
  /* 铺满侧栏：引导区可垂直居中，底部状态栏贴边 */
  flex: 1;
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
  padding: 12px 10px 8px;
}

/* 输入框 */
.search-field {
  display: flex;
  align-items: center;
  gap: 6px;
  height: 38px;
  padding: 0 7px 0 10px;
  border: 1px solid var(--border);
  border-radius: 10px;
  background: var(--bg-input);
  transition:
    border-color var(--t-fast),
    box-shadow var(--t-fast);
}
.search-field:hover {
  border-color: var(--border-strong);
}
.search-field.focused {
  border-color: var(--border-strong);
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
  font-size: 1em;
  outline: none;
}
.search-field input:focus {
  border: none;
  box-shadow: none;
}
.search-field input::placeholder {
  color: var(--fg-muted);
}

.field-btn {
  flex: none;
  display: grid;
  place-items: center;
  padding: 0;
  border-radius: 6px;
  color: var(--fg-dim);
}
.field-btn.clear {
  width: 22px;
  height: 22px;
}
.field-btn.clear:hover {
  background: var(--bg-active);
  color: var(--fg);
}
.field-btn.go {
  width: 24px;
  height: 24px;
  border-radius: 50%;
  background: var(--accent);
  color: var(--accent-fg);
}
.field-btn.go:hover {
  opacity: 0.86;
}

/* 范围 + 类型：一行次级控件 */
.toolbar {
  display: flex;
  align-items: center;
  gap: 6px;
}
.scope-switch {
  flex: 1;
  min-width: 0;
  display: flex;
  gap: 2px;
  padding: 2px;
  border-radius: 9px;
  background: var(--bg-hover);
}
.scope-switch button {
  flex: 1;
  min-width: 0;
  height: 28px;
  padding: 0 6px;
  border-radius: 7px;
  color: var(--fg-muted);
  font-size: 0.9375em;
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

.type-picker {
  position: relative;
  flex: none;
}
.type-btn {
  display: flex;
  align-items: center;
  gap: 3px;
  height: 32px;
  padding: 0 8px;
  border-radius: 9px;
  background: var(--bg-hover);
  color: var(--fg-muted);
  font-size: 0.9375em;
  font-weight: 500;
  white-space: nowrap;
}
.type-btn:hover {
  background: var(--bg-active);
  color: var(--fg);
}
.type-btn.on {
  background: var(--accent-soft);
  color: var(--fg);
}
.type-btn .chev {
  color: var(--fg-dim);
  transition: transform var(--t-fast);
}
.type-btn.open .chev {
  transform: rotate(180deg);
}

/* 类型下拉面板 */
.type-menu {
  position: absolute;
  top: calc(100% + 6px);
  right: 0;
  z-index: 30;
  /* 与搜索控件同宽，避免菜单左侧漏出下层提示文字 */
  width: calc(var(--sidebar-w) - 20px);
  max-height: min(360px, calc(100vh - 160px));
  padding: 6px;
  overflow: auto;
  border: 1px solid var(--border-strong);
  border-radius: 12px;
  background: var(--bg-elevated);
  box-shadow: var(--shadow-md);
}
.type-menu-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  height: 26px;
  padding: 0 4px 2px;
}
.type-menu-head .t {
  color: var(--fg-muted);
  font-size: 0.875em;
  font-weight: 600;
  letter-spacing: 0.04em;
}
.type-menu-head .reset {
  padding: 1px 6px;
  border-radius: 5px;
  color: var(--fg-muted);
  font-size: 0.875em;
}
.type-menu-head .reset:hover {
  background: var(--bg-hover);
  color: var(--fg);
}
.type-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 2px;
}
.type-item {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  height: 32px;
  padding: 0 6px;
  border-radius: 7px;
  color: var(--fg-muted);
  font-size: 1em;
  text-align: left;
}
.type-item:hover {
  background: var(--bg-hover);
  color: var(--fg);
}
.type-item.on {
  background: var(--accent-soft);
  color: var(--fg);
  font-weight: 600;
}
.ti-icon,
.ti-badge {
  flex: none;
  width: 16px;
  text-align: center;
}
.ti-badge {
  font-family: var(--mono);
  font-size: 0.68em;
  font-weight: 700;
  letter-spacing: 0.02em;
}
.ti-label {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}
.ti-check {
  flex: none;
  color: var(--fg);
}
.type-sep {
  grid-column: 1 / -1;
  height: 1px;
  margin: 4px 2px;
  background: var(--border);
}

/* 已选筛选 + 结果统计 */
.search-meta {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  min-height: 22px;
  padding: 0 2px;
}
.hint {
  min-width: 0;
  overflow: hidden;
  color: var(--fg-muted);
  font-size: 0.9167em;
  line-height: 1.35;
  white-space: nowrap;
  text-overflow: ellipsis;
}
.meta {
  flex: none;
  color: var(--fg-muted);
  font-size: 0.9167em;
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}
.active-filter {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  min-width: 0;
  height: 22px;
  padding: 0 6px;
  border: 1px solid var(--border);
  border-radius: 6px;
  background: var(--bg-hover);
  color: var(--fg);
  font-size: 0.9167em;
}
.active-filter:hover {
  border-color: var(--border-strong);
  background: var(--bg-active);
}
.af-icon,
.af-badge {
  flex: none;
}
.af-badge {
  font-family: var(--mono);
  font-size: 0.88em;
  font-weight: 700;
}
.af-label {
  min-width: 0;
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}
.af-x {
  flex: none;
  color: var(--fg-dim);
}
.active-filter:hover .af-x {
  color: var(--fg);
}

/* ---- 引导区（未输入时） ---- */
.search-body {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  justify-content: center;
  padding: 8px 12px 40px;
  overflow: auto;
}
.empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 5px;
  text-align: center;
}
.empty-icon {
  display: grid;
  place-items: center;
  width: 40px;
  height: 40px;
  margin-bottom: 3px;
  border: 1px solid var(--border);
  border-radius: 12px;
  background: var(--bg-hover);
  color: var(--fg-dim);
}
.empty-icon.folder {
  color: var(--ico-folder);
}
.empty-title {
  margin: 0;
  color: var(--fg);
  font-size: 1.0769em;
  font-weight: 600;
}
.empty-sub {
  margin: 0;
  max-width: 210px;
  color: var(--fg-muted);
  font-size: 0.9167em;
  line-height: 1.5;
}
.scope-path {
  display: flex;
  align-items: center;
  gap: 5px;
  max-width: 100%;
  margin-top: 6px;
  padding: 3px 9px;
  border-radius: 999px;
  background: var(--bg-hover);
  color: var(--fg-muted);
  font-size: 0.9167em;
}
.scope-path :deep(.icon) {
  color: var(--ico-folder);
}
.scope-path span {
  min-width: 0;
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}
.syntax-card {
  width: 100%;
  margin-top: 14px;
  padding: 4px;
  border: 1px solid var(--border);
  border-radius: 10px;
  background: var(--bg-hover);
}
.syntax-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 4px 6px 5px;
  color: var(--fg-muted);
  font-size: 0.875em;
  letter-spacing: 0.04em;
}
.syntax-head .note {
  color: var(--fg-muted);
  letter-spacing: 0;
}
.syntax-row {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  height: 30px;
  padding: 0 6px;
  border-radius: 7px;
  text-align: left;
}
.syntax-row:hover {
  background: var(--bg-active);
}
.syntax-row .name {
  flex: 1;
  color: var(--fg-muted);
  font-size: 0.9375em;
}
.syntax-row code {
  color: var(--fg-muted);
  font-family: var(--mono);
  font-size: 0.875em;
  background: transparent;
}
.syntax-row:hover .name,
.syntax-row:hover code {
  color: var(--fg);
}

/* ---- 底部索引状态 ---- */
.search-status {
  flex: none;
  height: 30px;
  display: flex;
  align-items: center;
  gap: 7px;
  padding: 0 10px;
  border-top: 1px solid var(--border);
  background: var(--bg-panel);
  color: var(--fg-muted);
  font-size: 0.9167em;
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
  background: var(--fg-dim);
  opacity: 0.7;
}
.search-status .dot.scanning {
  background: var(--warn);
  box-shadow: 0 0 0 3px var(--warn-soft);
  opacity: 1;
  animation: pulse 1s ease-in-out infinite;
}
.search-status .txt {
  min-width: 0;
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.search-status .state {
  flex: none;
}
.search-status .state.busy {
  color: var(--warn);
}

/* 下拉动效 */
.pop-enter-active,
.pop-leave-active {
  transition:
    opacity 0.12s ease,
    transform 0.12s ease;
  transform-origin: top right;
}
.pop-enter-from,
.pop-leave-to {
  opacity: 0;
  transform: translateY(-4px) scale(0.98);
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

<script setup lang="ts">
// 全局搜索栏（Everything 式，走索引引擎）（WS-D）
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { useSearchStore } from '../stores/search'
import { usePaneStore } from '../stores/pane'
import { useUiStore } from '../stores/ui'
import Icon from './icons/Icon.vue'

const search = useSearchStore()
const paneStore = usePaneStore()
const ui = useUiStore()

const text = ref('')
const scope = ref<'global' | 'current'>(ui.searchScope)
const open = ref(false)
const activeTab = computed(() => paneStore.activeTab)

// 索引状态（统一在此展示：条目数 / 已扫描 / 状态）
const indexText = computed(() =>
  ui.scanning
    ? `${ui.scanned.toLocaleString()} / ${ui.indexTotal.toLocaleString()}`
    : `索引 ${ui.indexTotal.toLocaleString()} 项`,
)
const indexTitle = computed(
  () => `索引条目 ${ui.indexTotal.toLocaleString()}，已扫描 ${ui.scanned.toLocaleString()}，${ui.scanning ? '扫描中' : '空闲'}`,
)

watch(
  () => [scope.value, paneStore.activePaneId, activeTab.value?.id],
  ([mode]) => {
    paneStore.panes.forEach((pane) => pane.tabs.forEach((tab) => (tab.filter = '')))
    if (mode === 'current' && activeTab.value) activeTab.value.filter = text.value
  },
)

let timer: ReturnType<typeof setTimeout> | null = null

let requestId = 0
let resultTabId: string | undefined

async function runGlobalSearch(value: string) {
  const trimmed = value.trim()
  if (!trimmed) return
  const currentRequest = ++requestId
  await search.run(trimmed)
  if (currentRequest !== requestId || scope.value !== 'global') return
  if (search.error) {
    ui.statusText = `搜索失败：${search.error}`
    return
  }
  resultTabId = paneStore.showSearchResults(trimmed, search.hits, resultTabId)
}

function onInput() {
  if (scope.value === 'current') {
    requestId += 1
    if (activeTab.value) activeTab.value.filter = text.value
    search.clear()
    open.value = false
    return
  }
  if (!text.value.trim()) requestId += 1
  open.value = true
  if (timer) clearTimeout(timer)
  timer = setTimeout(() => void runGlobalSearch(text.value), 150)
}

function clearAll() {
  requestId += 1
  text.value = ''
  if (scope.value === 'current' && activeTab.value) activeTab.value.filter = ''
  search.clear()
  open.value = false
}

function setScope(next: 'global' | 'current') {
  if (scope.value === next) return
  scope.value = next
  ui.searchScope = next
  if (next === 'current') {
    requestId += 1
    search.clear()
    if (activeTab.value) activeTab.value.filter = text.value
    open.value = false
  } else {
    paneStore.panes.forEach((pane) => pane.tabs.forEach((tab) => (tab.filter = '')))
    if (text.value.trim()) {
      open.value = true
      void runGlobalSearch(text.value)
    }
  }
}

watch(
  () => ui.searchCurrentRequest,
  () => {
    text.value = ''
    search.clear()
    setScope('current')
  },
)

function onEnter() {
  if (scope.value === 'current') return
  void runGlobalSearch(text.value)
}

onBeforeUnmount(() => {
  if (timer) clearTimeout(timer)
})
</script>

<template>
  <div class="search-wrap">
    <div class="search-bar" :class="{ focused: open }">
      <div class="scope-switch" role="group" aria-label="搜索范围">
        <button :class="{ on: scope === 'global' }" @click="setScope('global')">全局</button>
        <button :class="{ on: scope === 'current' }" @click="setScope('current')">当前目录</button>
      </div>
      <input
        v-model="text"
        :placeholder="scope === 'global' ? '全局搜索…（支持 ext:pdf  size:>100mb）' : '搜索当前 panel 目录…'"
        spellcheck="false"
        @input="onInput"
        @focus="open = true"
        @keyup.enter="onEnter"
        @keyup.esc="open = false"
      />
      <span v-if="scope === 'global' && search.searching" class="spinner" />
      <span v-else-if="scope === 'global' && text" class="meta">{{ search.hits.length }} 项</span>
      <button v-if="text" class="clear" title="清空" @click="clearAll">
        <Icon name="x" :size="13" />
      </button>
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
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
}
.search-bar {
  min-height: 60px;
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 6px;
  padding: 8px;
  background: var(--bg-panel);
  border-bottom: 1px solid var(--border);
}
.search-bar.focused {
  border-bottom-color: transparent;
}
.search-bar input {
  flex: 1 1 150px;
  min-width: 0;
  border: 1px solid var(--border);
  border-radius: 999px;
  background: var(--bg-panel);
  padding: 8px 14px;
  height: 38px;
}
.search-bar input:focus {
  border-color: var(--border-strong);
  box-shadow: none;
}
.scope-switch {
  flex: 1 1 100%;
  display: flex;
  align-items: center;
  height: 38px;
  padding: 3px;
  gap: 1px;
  border: 1px solid var(--border);
  border-radius: 999px;
}
.scope-switch button {
  height: 30px;
  flex: 1;
  padding: 0 8px;
  border-radius: 999px;
  color: var(--fg-muted);
  font-size: 0.9167rem;
  white-space: nowrap;
}
.scope-switch button.on {
  background: var(--bg-active);
  color: var(--accent);
}
.meta {
  font-size: 0.9167rem;
  color: var(--fg-dim);
  white-space: nowrap;
}
.clear {
  display: flex;
  align-items: center;
  color: var(--fg-dim);
  padding: 0 4px;
}
.clear:hover {
  color: var(--fg);
}
.spinner {
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
  gap: 6px;
  padding: 0 10px;
  border-top: 1px solid var(--border);
  background: var(--bg-panel);
  font-size: 0.9167rem;
  color: var(--fg-muted);
  overflow: hidden;
}
.search-status .spacer {
  flex: 1;
}
.search-status .dot {
  flex: none;
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--ok);
}
.search-status .dot.scanning {
  background: var(--warn);
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

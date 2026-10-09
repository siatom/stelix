<script setup lang="ts">
// 全局搜索栏（Everything 式，走索引引擎）（WS-D）
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useSearchStore } from '../stores/search'
import { usePaneStore } from '../stores/pane'
import { useUiStore } from '../stores/ui'
import type { SearchHit } from '../types'
import { dirname, fileIcon, fileKind, formatSize } from './Pane/utils'
import Icon from './icons/Icon.vue'

const search = useSearchStore()
const paneStore = usePaneStore()
const ui = useUiStore()

const text = ref('')
const scope = ref<'global' | 'current'>('global')
const open = ref(false)
const rootRef = ref<HTMLElement | null>(null)

const activeTab = computed(() => paneStore.activeTab)

watch(
  () => [scope.value, paneStore.activePaneId, activeTab.value?.id],
  ([mode]) => {
    paneStore.panes.forEach((pane) => pane.tabs.forEach((tab) => (tab.filter = '')))
    if (mode === 'current' && activeTab.value) activeTab.value.filter = text.value
  },
)

let timer: ReturnType<typeof setTimeout> | null = null

const showPanel = computed(
  () => scope.value === 'global' && open.value && (search.searching || search.hits.length > 0 || !!search.error),
)

function onInput() {
  if (scope.value === 'current') {
    if (activeTab.value) activeTab.value.filter = text.value
    search.clear()
    open.value = false
    return
  }
  open.value = true
  if (timer) clearTimeout(timer)
  timer = setTimeout(() => void search.run(text.value), 150)
}

function clearAll() {
  text.value = ''
  if (scope.value === 'current' && activeTab.value) activeTab.value.filter = ''
  search.clear()
  open.value = false
}

function setScope(next: 'global' | 'current') {
  if (scope.value === next) return
  scope.value = next
  if (next === 'current') {
    search.clear()
    if (activeTab.value) activeTab.value.filter = text.value
    open.value = false
  } else {
    paneStore.panes.forEach((pane) => pane.tabs.forEach((tab) => (tab.filter = '')))
    if (text.value.trim()) {
      open.value = true
      void search.run(text.value)
    }
  }
}

async function goTo(hit: SearchHit) {
  open.value = false
  try {
    await paneStore.reveal(hit.entry.path)
  } catch (e) {
    ui.statusText = `无法定位：${String(e)}`
  }
}

function onEnter() {
  if (scope.value === 'current') return
  const first = search.hits[0]
  if (first) void goTo(first)
  else void search.run(text.value)
}

function onDocClick(ev: MouseEvent) {
  if (rootRef.value && !rootRef.value.contains(ev.target as Node)) open.value = false
}

onMounted(() => document.addEventListener('click', onDocClick))
onBeforeUnmount(() => {
  document.removeEventListener('click', onDocClick)
  if (timer) clearTimeout(timer)
})
</script>

<template>
  <div ref="rootRef" class="search-wrap">
    <div class="search-bar" :class="{ focused: open }">
      <span class="icon"><Icon name="search" :size="15" /></span>
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
      <span v-else-if="scope === 'global' && text" class="meta">
        {{ search.hits.length }} 项 / {{ search.elapsedMs.toFixed(1) }}ms
      </span>
      <button v-if="text" class="clear" title="清空" @click="clearAll">
        <Icon name="x" :size="13" />
      </button>
    </div>

    <div v-if="showPanel" class="panel">
      <p v-if="search.error" class="panel-state err">搜索失败：{{ search.error }}</p>
      <p v-else-if="search.searching && !search.hits.length" class="panel-state">搜索中…</p>
      <p v-else-if="!search.hits.length" class="panel-state">没有找到匹配项</p>
      <template v-else>
        <button
          v-for="(hit, i) in search.hits.slice(0, 200)"
          :key="hit.entry.path"
          class="hit"
          :class="{ first: i === 0 }"
          @click="goTo(hit)"
        >
          <span class="hit-ico" :class="`k-${fileKind(hit.entry)}`">
            <Icon :name="fileIcon(hit.entry)" :size="14" />
          </span>
          <span class="hit-main">
            <span class="hit-name" :class="{ 'is-folder': hit.entry.isDir }">{{ ui.displayFileName(hit.entry.name, hit.entry.isDir) }}</span>
            <span class="hit-path" :title="dirname(hit.entry.path)">{{ dirname(hit.entry.path) }}</span>
          </span>
          <span class="hit-size">{{ formatSize(hit.entry.size, hit.entry.isDir) }}</span>
        </button>
        <p v-if="search.hits.length > 200" class="panel-state">仅显示前 200 条（共 {{ search.hits.length }} 条）</p>
      </template>
    </div>
  </div>
</template>

<style scoped>
.search-wrap {
  position: relative;
  flex: none;
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
.icon {
  display: flex;
  align-items: center;
  color: var(--fg-muted);
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
  font-size: 11px;
  white-space: nowrap;
}
.scope-switch button.on {
  background: var(--bg-active);
  color: var(--accent);
}
.meta {
  font-size: 11px;
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

.panel {
  position: static;
  max-height: 52vh;
  overflow: auto;
  background: var(--bg-elevated);
  border: 1px solid var(--border-strong);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-md);
}
.hit {
  width: 100%;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 5px 10px;
  text-align: left;
}
.hit:hover,
.hit.first {
  background: var(--bg-hover);
}
.hit-ico {
  flex: none;
  display: flex;
  align-items: center;
  color: var(--fg-muted);
}
.hit-main {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
}
.hit-name {
  font-size: 12.5px;
  color: var(--fg);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.hit-name.is-folder {
  color: var(--fg-folder);
}
.hit-path {
  font-size: 10.5px;
  color: var(--fg-dim);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.hit-size {
  flex: none;
  font-size: 11px;
  color: var(--fg-muted);
}
.panel-state {
  margin: 0;
  padding: 10px 12px;
  font-size: 12px;
  color: var(--fg-dim);
}
.panel-state.err {
  color: var(--danger);
}

/* 与 FileList 保持一致的文件类型配色 */
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

<script setup lang="ts">
// 全局搜索栏（Everything 式，走索引引擎）（WS-D）
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
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
const open = ref(false)
const rootRef = ref<HTMLElement | null>(null)

let timer: ReturnType<typeof setTimeout> | null = null

const showPanel = computed(
  () => open.value && (search.searching || search.hits.length > 0 || !!search.error),
)

function onInput() {
  open.value = true
  if (timer) clearTimeout(timer)
  timer = setTimeout(() => void search.run(text.value), 150)
}

function clearAll() {
  text.value = ''
  search.clear()
  open.value = false
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
      <input
        v-model="text"
        placeholder="全盘搜索…（支持 ext:pdf  size:>100mb）"
        spellcheck="false"
        @input="onInput"
        @focus="open = true"
        @keyup.enter="onEnter"
        @keyup.esc="open = false"
      />
      <span v-if="search.searching" class="spinner" />
      <span v-else-if="text" class="meta">
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
            <span class="hit-name">{{ hit.entry.name }}</span>
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
  height: var(--search-bar-h);
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 0 10px;
  background: var(--bg-panel);
  border-bottom: 1px solid var(--border);
}
.search-bar.focused {
  border-bottom-color: var(--accent);
}
.search-bar input {
  flex: 1;
  border: none;
  background: transparent;
  padding: 0;
}
.search-bar input:focus {
  border: none;
}
.icon {
  display: flex;
  align-items: center;
  color: var(--fg-muted);
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
  position: absolute;
  left: 0;
  right: 0;
  top: 100%;
  z-index: 150;
  max-height: 52vh;
  overflow: auto;
  background: var(--bg-elevated);
  border: 1px solid var(--border-strong);
  border-top: none;
  border-radius: 0 0 6px 6px;
  box-shadow: 0 10px 26px rgba(0, 0, 0, 0.45);
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

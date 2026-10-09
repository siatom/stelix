<script setup lang="ts">
// TODO(WS-D / feat/ui-panes): 全局搜索栏（Everything 式，走索引引擎）
import { ref } from 'vue'
import { useSearchStore } from '../stores/search'

const search = useSearchStore()
const text = ref('')

let timer: ReturnType<typeof setTimeout> | null = null
function onInput() {
  if (timer) clearTimeout(timer)
  timer = setTimeout(() => void search.run(text.value), 120)
}
</script>

<template>
  <div class="search-bar">
    <span class="icon">🔍</span>
    <input v-model="text" placeholder="全盘搜索文件名…" @input="onInput" />
    <span class="meta">{{ search.hits.length }} 项 / {{ search.elapsedMs.toFixed(1) }}ms</span>
  </div>
</template>

<style scoped>
.search-bar {
  height: var(--search-bar-h);
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 0 10px;
  background: var(--bg-panel);
  border-bottom: 1px solid var(--border);
}
.search-bar input {
  flex: 1;
}
.icon {
  color: var(--fg-muted);
}
.meta {
  font-size: 11px;
  color: var(--fg-dim);
  white-space: nowrap;
}
</style>

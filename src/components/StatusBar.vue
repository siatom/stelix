<script setup lang="ts">
// 底部状态栏：当前路径 / 选中数 / 条目数 / 索引总数 / 状态文本
import { computed } from 'vue'
import { usePaneStore } from '../stores/pane'
import { useUiStore } from '../stores/ui'

const pane = usePaneStore()
const ui = useUiStore()

const currentPath = computed(() => pane.activeTab?.path ?? '')
const selectedCount = computed(() => pane.activeTab?.selected.length ?? 0)
const entryCount = computed(() => pane.activeTab?.entries.length ?? 0)
</script>

<template>
  <footer class="status-bar">
    <span class="seg path" :title="currentPath">{{ currentPath || '—' }}</span>
    <span class="sep" />
    <span class="seg">选中 {{ selectedCount }} 项</span>
    <span class="sep" />
    <span class="seg">条目 {{ entryCount }}</span>

    <div class="spacer" />

    <span class="seg status">{{ ui.statusText }}</span>
    <span class="sep" />
    <span class="seg index">
      <span v-if="ui.scanning" class="dot scanning" />
      <span v-else class="dot" />
      索引 {{ ui.indexTotal.toLocaleString() }} 项
    </span>
  </footer>
</template>

<style scoped>
.status-bar {
  height: var(--status-bar-h);
  background: var(--bg-panel);
  border-top: 1px solid var(--border);
  display: flex;
  align-items: center;
  padding: 0 8px;
  font-size: 12px;
  color: var(--fg-muted);
  overflow: hidden;
}
.spacer {
  flex: 1;
}
.seg {
  padding: 0 6px;
  white-space: nowrap;
}
.path {
  max-width: 40vw;
  overflow: hidden;
  text-overflow: ellipsis;
  color: var(--fg);
  font-family: var(--mono);
  font-size: 11px;
}
.status {
  color: var(--fg-muted);
}
.index {
  display: inline-flex;
  align-items: center;
  gap: 5px;
}
.dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--ok);
  flex: none;
}
.dot.scanning {
  background: var(--warn);
  animation: pulse 1s ease-in-out infinite;
}
.sep {
  width: 1px;
  height: 12px;
  background: var(--border);
  flex: none;
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

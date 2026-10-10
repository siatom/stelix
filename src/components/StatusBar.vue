<script setup lang="ts">
// 底部状态栏：窗格选中信息 + 仅在活动窗格显示的全局状态
import { computed } from 'vue'
import { usePaneStore } from '../stores/pane'
import { useUiStore } from '../stores/ui'
import Icon from './icons/Icon.vue'

const props = defineProps<{ paneId: string }>()
const paneStore = usePaneStore()
const ui = useUiStore()

const tab = computed(() => {
  const pane = paneStore.panes.find((item) => item.id === props.paneId)
  return pane?.tabs.find((item) => item.id === pane.activeTabId) ?? pane?.tabs[0]
})
const folderStats = computed(() => {
  const entries = tab.value?.entries ?? []
  const selected = new Set(tab.value?.selected ?? [])
  const folders = entries.filter((entry) => entry.isDir)
  return {
    selected: folders.filter((entry) => selected.has(entry.path)).length,
    total: folders.length,
  }
})
const fileStats = computed(() => {
  const entries = tab.value?.entries ?? []
  const selected = new Set(tab.value?.selected ?? [])
  const files = entries.filter((entry) => !entry.isDir)
  return {
    selected: files.filter((entry) => selected.has(entry.path)).length,
    total: files.length,
  }
})
const isActivePane = computed(() => paneStore.activePaneId === props.paneId)
</script>

<template>
  <footer class="status-bar">
    <span class="seg count" :title="`文件夹 ${folderStats.selected} / ${folderStats.total}`" aria-label="文件夹数量">
      <Icon name="folder" :size="14" />
      <span>{{ folderStats.selected }} / {{ folderStats.total }}</span>
    </span>
    <span class="seg count" :title="`文件 ${fileStats.selected} / ${fileStats.total}`" aria-label="文件数量">
      <Icon name="file" :size="14" />
      <span>{{ fileStats.selected }} / {{ fileStats.total }}</span>
    </span>

    <div class="spacer" />

    <template v-if="isActivePane">
      <span class="seg status">{{ ui.statusText }}</span>
      <span class="sep" />
      <span class="seg index">
        <span v-if="ui.scanning" class="dot scanning" />
        <span v-else class="dot" />
        索引 {{ ui.indexTotal.toLocaleString() }} 项
      </span>
    </template>
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
  font-size: 1rem;
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
.count {
  display: inline-flex;
  align-items: center;
  gap: 5px;
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

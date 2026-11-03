<script setup lang="ts">
// 底部状态栏：窗格选中信息 + 仅在活动窗格显示的全局状态
import { computed } from 'vue'
import { usePaneStore } from '../stores/pane'
import { useUiStore } from '../stores/ui'
import Icon from './icons/Icon.vue'
import FileIcon from './icons/FileIcon.vue'
import { RECYCLE_BIN_SHELL_PATH } from './Pane/utils'

const props = defineProps<{
  paneId: string
  /** 回收站标签的条目总数 / 选中数（由该窗格的 TrashView 提供）。 */
  trashCount?: number
  trashSelectedCount?: number
}>()
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
    <template v-if="tab?.kind === 'trash'">
      <span
        class="seg count"
        :title="`回收站 ${trashSelectedCount ?? 0} / ${trashCount ?? 0} 项`"
        aria-label="回收站条目数量"
      >
        <FileIcon
          :path="RECYCLE_BIN_SHELL_PATH"
          :is-dir="false"
          :ext="null"
          :use-path-icon="true"
          fallback="trash"
          :size="14"
        />
        <span>{{ trashSelectedCount ?? 0 }} / {{ trashCount ?? 0 }}</span>
      </span>
    </template>
    <template v-else-if="tab?.kind !== 'terminal'">
      <span class="seg count" :title="`文件夹 ${folderStats.selected} / ${folderStats.total}`" aria-label="文件夹数量">
        <Icon name="folder" :size="14" />
        <span>{{ folderStats.selected }} / {{ folderStats.total }}</span>
      </span>
      <span class="seg count" :title="`文件 ${fileStats.selected} / ${fileStats.total}`" aria-label="文件数量">
        <Icon name="file" :size="14" />
        <span>{{ fileStats.selected }} / {{ fileStats.total }}</span>
      </span>
    </template>

    <div class="spacer" />

    <template v-if="isActivePane">
      <span class="seg status">{{ ui.statusText }}</span>
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
</style>

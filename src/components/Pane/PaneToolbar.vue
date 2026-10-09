<script setup lang="ts">
// 窗格内嵌工具栏（替代传统顶部菜单）：导航 / 文件操作 / 视图 / 本地过滤（WS-D）
import { computed } from 'vue'
import type { TabState, ViewMode } from '../../types'
import { usePaneStore } from '../../stores/pane'
import { useUiStore } from '../../stores/ui'
import Icon from '../icons/Icon.vue'
import type { IconName } from '../icons/paths'

const props = defineProps<{ paneId: string; tab: TabState }>()
const emit = defineEmits<{
  (e: 'new-folder'): void
  (e: 'rename', path: string): void
  (e: 'delete', paths: string[]): void
}>()

const paneStore = usePaneStore()
const ui = useUiStore()

const pane = computed(() => paneStore.panes.find((p) => p.id === props.paneId))

const modes: { key: ViewMode; icon: IconName; title: string }[] = [
  { key: 'details', icon: 'view-details', title: '详细' },
  { key: 'list', icon: 'view-list', title: '列表' },
  { key: 'icons', icon: 'view-icons', title: '图标' },
]

const selectedCount = computed(() => props.tab.selected.length)

function requestRename() {
  if (props.tab.selected.length === 1) emit('rename', props.tab.selected[0])
  else ui.statusText = '请先选择单个条目再重命名'
}

function requestDelete() {
  if (props.tab.selected.length) emit('delete', [...props.tab.selected])
  else ui.statusText = '请先选择要删除的条目'
}
</script>

<template>
  <div class="pane-toolbar">
    <div class="group">
      <button title="后退" :disabled="!paneStore.canGoBack(paneId)" @click="paneStore.goBack(paneId)">
        <Icon name="arrow-left" />
      </button>
      <button title="前进" :disabled="!paneStore.canGoForward(paneId)" @click="paneStore.goForward(paneId)">
        <Icon name="arrow-right" />
      </button>
      <button title="上级目录" @click="paneStore.goUp(paneId)">
        <Icon name="arrow-up" />
      </button>
      <button title="刷新" @click="paneStore.refresh(paneId)">
        <Icon name="refresh" />
      </button>
    </div>

    <div class="group">
      <button title="新建文件夹" @click="emit('new-folder')">
        <Icon name="folder-plus" />
      </button>
      <button title="重命名选中项" @click="requestRename">
        <Icon name="pencil" />
      </button>
      <button title="删除选中项" @click="requestDelete">
        <Icon name="trash" />
      </button>
      <button title="复制到对侧窗格" @click="paneStore.copyToOtherPane(paneId)">
        <Icon name="swap" />
      </button>
    </div>

    <div class="group views">
      <button
        v-for="m in modes"
        :key="m.key"
        :class="{ on: pane?.viewMode === m.key }"
        :title="m.title"
        @click="paneStore.setViewMode(paneId, m.key)"
      >
        <Icon :name="m.icon" />
      </button>
    </div>

    <span v-if="selectedCount" class="sel">已选 {{ selectedCount }}</span>
  </div>
</template>

<style scoped>
.pane-toolbar {
  display: flex;
  align-items: center;
  gap: 6px;
  height: var(--toolbar-h);
  padding: 0 6px;
  background: var(--bg-panel);
  border-bottom: 1px solid var(--border);
  overflow: hidden;
}
.group {
  display: flex;
  align-items: center;
  gap: 1px;
  flex: none;
  padding-right: 6px;
  border-right: 1px solid var(--border);
}
.group button {
  width: 24px;
  height: 22px;
  border-radius: var(--radius);
  color: var(--fg-muted);
  font-size: 13px;
  display: flex;
  align-items: center;
  justify-content: center;
}
.group button:hover:not(:disabled) {
  background: var(--bg-hover);
  color: var(--fg);
}
.group button:disabled {
  opacity: 0.35;
  cursor: default;
}
.views button.on {
  background: var(--bg-active);
  color: var(--accent);
}
.sel {
  flex: none;
  font-size: 11px;
  color: var(--accent);
  white-space: nowrap;
}
</style>

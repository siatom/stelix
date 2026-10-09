<script setup lang="ts">
// 窗格内嵌工具栏（替代传统顶部菜单）：导航 / 文件操作 / 视图 / 排序 / 本地过滤（WS-D）
import { computed } from 'vue'
import type { TabState, ViewMode } from '../../types'
import { usePaneStore } from '../../stores/pane'
import { useUiStore } from '../../stores/ui'

const props = defineProps<{ paneId: string; tab: TabState }>()
const emit = defineEmits<{
  (e: 'new-folder'): void
  (e: 'rename', path: string): void
  (e: 'delete', paths: string[]): void
}>()

const paneStore = usePaneStore()
const ui = useUiStore()

const pane = computed(() => paneStore.panes.find((p) => p.id === props.paneId))

const modes: { key: ViewMode; icon: string; title: string }[] = [
  { key: 'details', icon: '☰', title: '详细' },
  { key: 'list', icon: '≣', title: '列表' },
  { key: 'icons', icon: '▦', title: '图标' },
]

const sortOptions: { key: TabState['sortKey']; label: string }[] = [
  { key: 'name', label: '名称' },
  { key: 'size', label: '大小' },
  { key: 'modified', label: '修改时间' },
  { key: 'ext', label: '类型' },
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
      <button title="后退" :disabled="!paneStore.canGoBack(paneId)" @click="paneStore.goBack(paneId)">←</button>
      <button title="前进" :disabled="!paneStore.canGoForward(paneId)" @click="paneStore.goForward(paneId)">→</button>
      <button title="上级目录" @click="paneStore.goUp(paneId)">↑</button>
      <button title="刷新" @click="paneStore.refresh(paneId)">⟳</button>
    </div>

    <div class="group">
      <button title="新建文件夹" @click="emit('new-folder')">＋📁</button>
      <button title="重命名选中项" @click="requestRename">✎</button>
      <button title="删除选中项" @click="requestDelete">🗑</button>
      <button title="复制到对侧窗格" @click="paneStore.copyToOtherPane(paneId)">⇄</button>
    </div>

    <div class="group views">
      <button
        v-for="m in modes"
        :key="m.key"
        :class="{ on: pane?.viewMode === m.key }"
        :title="m.title"
        @click="paneStore.setViewMode(paneId, m.key)"
      >
        {{ m.icon }}
      </button>
    </div>

    <select
      class="sort"
      :value="tab.sortKey"
      title="排序字段"
      @change="paneStore.toggleSort(tab, ($event.target as HTMLSelectElement).value as TabState['sortKey'])"
    >
      <option v-for="o in sortOptions" :key="o.key" :value="o.key">{{ o.label }}</option>
    </select>
    <button class="order" :title="tab.sortAsc ? '升序' : '降序'" @click="paneStore.toggleSort(tab, tab.sortKey)">
      {{ tab.sortAsc ? '▲' : '▼' }}
    </button>

    <input v-model="tab.filter" class="filter" placeholder="过滤当前目录…" spellcheck="false" />
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
.sort {
  flex: none;
  height: 22px;
  background: var(--bg-input);
  color: var(--fg);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  font-size: 12px;
  padding: 0 2px;
}
.order {
  flex: none;
  width: 20px;
  height: 22px;
  color: var(--fg-muted);
  font-size: 10px;
}
.order:hover {
  color: var(--fg);
}
.filter {
  flex: 1;
  min-width: 60px;
  height: 22px;
  font-size: 12px;
}
.sel {
  flex: none;
  font-size: 11px;
  color: var(--accent);
  white-space: nowrap;
}
</style>

<script setup lang="ts">
// 窗格内嵌工具栏：导航 / 新建 / 视图
import { computed } from 'vue'
import type { ViewMode } from '../../types'
import { usePaneStore } from '../../stores/pane'
import Icon from '../icons/Icon.vue'
import type { IconName } from '../icons/paths'

const props = defineProps<{ paneId: string }>()
const emit = defineEmits<{ (e: 'create', kind: 'mkdir' | 'createFile'): void }>()

const paneStore = usePaneStore()

const pane = computed(() => paneStore.panes.find((p) => p.id === props.paneId))
const activeTab = computed(() => {
  const p = pane.value
  if (!p) return undefined
  return p.tabs.find((t) => t.id === p.activeTabId) ?? p.tabs[0]
})
// 搜索结果与远程标签都没有本地「目录」写权限，禁用新建入口。
const canCreate = computed(
  () => !!activeTab.value && activeTab.value.kind !== 'search' && activeTab.value.kind !== 'remote',
)

const modes: { key: ViewMode; icon: IconName; title: string }[] = [
  { key: 'details', icon: 'view-details', title: '详细' },
  { key: 'list', icon: 'view-list', title: '列表' },
  { key: 'icons', icon: 'view-icons', title: '图标' },
]
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
      <button title="新建文件夹" :disabled="!canCreate" @click="emit('create', 'mkdir')">
        <Icon name="folder-plus" />
      </button>
      <button title="新建文件" :disabled="!canCreate" @click="emit('create', 'createFile')">
        <Icon name="file-plus" />
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
  </div>
</template>

<style scoped>
.pane-toolbar {
  display: flex;
  align-items: center;
  gap: 6px;
  height: var(--pane-bar-h);
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
  font-size: 1.0833rem;
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
  color: var(--fg);
}
</style>

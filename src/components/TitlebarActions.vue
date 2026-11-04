<script setup lang="ts">
// 顶部标题栏的文件操作区：新建文件夹 / 新建文件 / 删除 + 视图切换。
// 全局唯一入口，始终作用于**当前活动窗格** —— 哪个窗格活动，这些按钮就控制哪个窗格。
import { computed } from 'vue'
import { api } from '../api'
import type { ViewMode } from '../types'
import { usePaneStore } from '../stores/pane'
import { useUiStore } from '../stores/ui'
import Icon from './icons/Icon.vue'
import type { IconName } from './icons/paths'

const paneStore = usePaneStore()
const ui = useUiStore()

const modes: { key: ViewMode; icon: IconName; title: string }[] = [
  { key: 'details', icon: 'view-details', title: '详细' },
  { key: 'list', icon: 'view-list', title: '列表' },
  { key: 'icons', icon: 'view-icons', title: '图标' },
]

const viewMode = computed(() => paneStore.activePane?.viewMode ?? 'details')
/** 当前活动窗格已选中的条目数（「移动到新建文件夹」按钮的提示与可用态）。 */
const selectedCount = computed(() => paneStore.activeTab?.selected.length ?? 0)
const moveCountHint = computed(() => (selectedCount.value ? `（${selectedCount.value} 项）` : ''))
const canOpenTerminal = computed(() => {
  const tab = paneStore.activeTab
  return !!tab && (!tab.kind || tab.kind === 'files')
})
/** 双窗格下在 tooltip 里点明作用对象，避免误以为按钮属于某一侧。 */
const scopeHint = computed(() => {
  if (paneStore.singlePane) return ''
  return paneStore.activePaneId === 'right' ? '（右侧窗格）' : '（左侧窗格）'
})

async function openTerminalHere() {
  const tab = paneStore.activeTab
  if (!tab || !canOpenTerminal.value) return
  try {
    await api.openInTerminal(tab.path)
    ui.setStatusText('已在当前文件夹打开系统终端')
  } catch (error) {
    ui.setStatusText(`无法打开系统终端：${String(error)}`)
  }
}
</script>

<template>
  <div class="titlebar-actions">
    <div class="group">
      <button
        type="button"
        :title="`新建文件夹${scopeHint}`"
        :aria-label="`新建文件夹${scopeHint}`"
        :disabled="!paneStore.canCreateInActivePane"
        @click="paneStore.requestAction('mkdir')"
      >
        <Icon name="folder-plus" :size="15" />
      </button>
      <button
        type="button"
        :title="`移动到新建文件夹${moveCountHint}${scopeHint}`"
        :aria-label="`移动到新建文件夹${scopeHint}`"
        :disabled="!paneStore.canMoveToNewFolderInActivePane"
        @click="paneStore.requestAction('mkdirMove')"
      >
        <Icon name="folder-input" :size="15" />
      </button>
      <button
        type="button"
        :title="`新建文件${scopeHint}`"
        :aria-label="`新建文件${scopeHint}`"
        :disabled="!paneStore.canCreateInActivePane"
        @click="paneStore.requestAction('createFile')"
      >
        <Icon name="file-plus" :size="15" />
      </button>
      <button
        type="button"
        class="danger"
        :title="`删除所选（移到回收站，Shift 点击永久删除）${scopeHint}`"
        :aria-label="`删除所选（移到回收站，Shift 点击永久删除）${scopeHint}`"
        :disabled="!paneStore.canDeleteInActivePane"
        @click="paneStore.requestAction('delete', $event.shiftKey)"
      >
        <Icon name="trash" :size="15" />
      </button>
    </div>

    <span class="divider" aria-hidden="true" />

    <div class="group">
      <button
        v-for="m in modes"
        :key="m.key"
        type="button"
        class="toggle"
        :class="{ on: viewMode === m.key }"
        :title="`${m.title}视图${scopeHint}`"
        :aria-label="`${m.title}视图${scopeHint}`"
        :aria-pressed="viewMode === m.key"
        @click="paneStore.setViewMode(paneStore.activePaneId, m.key)"
      >
        <Icon :name="m.icon" :size="15" />
      </button>
    </div>

    <button
      type="button"
      :title="`在当前文件夹打开系统终端${scopeHint}`"
      :aria-label="`在当前文件夹打开系统终端${scopeHint}`"
      :disabled="!canOpenTerminal"
      @click="openTerminalHere"
    >
      <Icon name="terminal" :size="15" />
    </button>
  </div>
</template>

<style scoped>
.titlebar-actions {
  display: flex;
  align-items: center;
  gap: 4px;
  flex: none;
  margin-right: auto;
}
.group {
  display: flex;
  align-items: center;
  gap: 2px;
}
.group button {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 30px;
  height: 26px;
  border-radius: var(--radius-sm);
  color: var(--fg-muted);
}
.group button:hover:not(:disabled) {
  background: var(--bg-hover);
  color: var(--fg);
}
.group button:disabled {
  opacity: 0.35;
  cursor: default;
}
.group button.danger:hover:not(:disabled) {
  background: var(--danger-soft);
  color: var(--danger);
}
.toggle.on {
  background: var(--bg-active);
  color: var(--fg);
}
.divider {
  flex: none;
  width: 1px;
  height: 14px;
  background: var(--border-strong);
}
</style>

<script setup lang="ts">
// 顶部标题栏的工具区，按「创建 → 删除 → 工具 → 视图」分组。
// 全局唯一入口，始终作用于**当前活动窗格** —— 哪个窗格活动，这些按钮就控制哪个窗格。
import { computed } from 'vue'
import { api } from '../api'
import type { ViewMode } from '../types'
import { usePaneStore } from '../stores/pane'
import { useUiStore } from '../stores/ui'
import Icon from './icons/Icon.vue'
import type { IconName } from './icons/paths'
import { t } from '../i18n'

const paneStore = usePaneStore()
const ui = useUiStore()

const modes: { key: ViewMode; icon: IconName; title: string; titleEn: string }[] = [
  { key: 'details', icon: 'view-details', title: '详细', titleEn: 'Details' },
  { key: 'list', icon: 'view-list', title: '列表（多列）', titleEn: 'List (columns)' },
  { key: 'icons', icon: 'view-icons', title: '图标', titleEn: 'Icons' },
]

const viewMode = computed(() => paneStore.activePane?.viewMode ?? 'details')
/** 当前活动窗格已选中的条目数（「移动到新建文件夹」按钮的提示与可用态）。 */
const selectedCount = computed(() => paneStore.activeTab?.selected.length ?? 0)
const moveCountHint = computed(() => (selectedCount.value ? t(`（${selectedCount.value} 项）`, ` (${selectedCount.value} items)`) : ''))
const canOpenTerminal = computed(() => {
  const tab = paneStore.activeTab
  return !!tab && (!tab.kind || tab.kind === 'files')
})
/** 同步入口只在双窗格、且两侧都是本地目录时可用。 */
const syncDisabledReason = computed(() => paneStore.syncDirs.reason ?? '')
const canSync = computed(() => !syncDisabledReason.value)
/** 双窗格下在 tooltip 里点明作用对象，避免误以为按钮属于某一侧。 */
const scopeHint = computed(() => {
  if (paneStore.singlePane) return ''
  return paneStore.activePaneId === 'right' ? t('（右侧窗格）') : t('（左侧窗格）')
})

async function openTerminalHere() {
  const tab = paneStore.activeTab
  if (!tab || !canOpenTerminal.value) return
  try {
    await api.openInTerminal(tab.path)
    ui.setStatusText(t('已在当前文件夹打开系统终端'))
  } catch (error) {
    ui.setStatusText(t(`无法打开系统终端：${String(error)}`, `Could not open the system terminal: ${String(error)}`))
  }
}
</script>

<template>
  <div class="titlebar-actions" role="toolbar" :aria-label="t('文件操作与视图')">
    <!-- 创建 -->
    <div class="group">
      <button
        type="button"
        :title="t(`新建文件${scopeHint}`, `New file${scopeHint}`)"
        :aria-label="t(`新建文件${scopeHint}`, `New file${scopeHint}`)"
        :disabled="!paneStore.canCreateInActivePane"
        @click="paneStore.requestAction('createFile')"
      >
        <Icon name="file-plus" :size="15" />
      </button>
      <button
        type="button"
        :title="t(`新建文件夹${scopeHint}`, `New folder${scopeHint}`)"
        :aria-label="t(`新建文件夹${scopeHint}`, `New folder${scopeHint}`)"
        :disabled="!paneStore.canCreateInActivePane"
        @click="paneStore.requestAction('mkdir')"
      >
        <Icon name="folder-plus" :size="15" />
      </button>
      <button
        type="button"
        :title="t(`移动到新建文件夹${moveCountHint}${scopeHint}`, `Move to new folder${moveCountHint}${scopeHint}`)"
        :aria-label="t(`移动到新建文件夹${scopeHint}`, `Move to new folder${scopeHint}`)"
        :disabled="!paneStore.canMoveToNewFolderInActivePane"
        @click="paneStore.requestAction('mkdirMove')"
      >
        <Icon name="folder-symlink" :size="15" />
      </button>
    </div>

    <span class="divider" aria-hidden="true" />

    <!-- 删除：单独成组，和常规操作保持距离 -->
    <div class="group">
      <button
        type="button"
        class="danger"
        :title="t(`删除所选（移到回收站，Shift 点击永久删除）${scopeHint}`, `Delete selected (move to Recycle Bin; Shift-click to delete permanently)${scopeHint}`)"
        :aria-label="t(`删除所选（移到回收站，Shift 点击永久删除）${scopeHint}`, `Delete selected (move to Recycle Bin; Shift-click to delete permanently)${scopeHint}`)"
        :disabled="!paneStore.canDeleteInActivePane"
        @click="paneStore.requestAction('delete', $event.shiftKey)"
      >
        <Icon name="trash" :size="15" />
      </button>
    </div>

    <span class="divider" aria-hidden="true" />

    <!-- 工具 -->
    <div class="group">
      <button
        type="button"
        :title="canSync ? t('同步左右两个文件夹') : t(`无法同步：${syncDisabledReason}`, `Cannot sync: ${syncDisabledReason}`)"
        :aria-label="t('同步左右两个文件夹')"
        :disabled="!canSync"
        @click="ui.openSync()"
      >
        <Icon name="folder-sync" :size="15" />
      </button>
      <button
        type="button"
        :title="t(`在当前文件夹打开系统终端${scopeHint}`, `Open system terminal in current folder${scopeHint}`)"
        :aria-label="t(`在当前文件夹打开系统终端${scopeHint}`, `Open system terminal in current folder${scopeHint}`)"
        :disabled="!canOpenTerminal"
        @click="openTerminalHere"
      >
        <Icon name="terminal" :size="15" />
      </button>
    </div>

    <span class="divider" aria-hidden="true" />

    <!-- 视图 -->
    <div class="group">
      <button
        v-for="m in modes"
        :key="m.key"
        type="button"
        class="toggle"
        :class="{ on: viewMode === m.key }"
        :title="t(`${m.title}视图${scopeHint}`, `${m.titleEn} view${scopeHint}`)"
        :aria-label="t(`${m.title}视图${scopeHint}`, `${m.titleEn} view${scopeHint}`)"
        :aria-pressed="viewMode === m.key"
        @click="paneStore.setViewMode(paneStore.activePaneId, m.key)"
      >
        <Icon :name="m.icon" :size="15" />
      </button>
    </div>
  </div>
</template>

<style scoped>
.titlebar-actions {
  display: flex;
  align-items: center;
  gap: 4px;
  flex: none;
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

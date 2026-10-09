<script setup lang="ts">
// 二级面板容器：按 activePanel 渲染目录树 / 快速访问 / 工作区 / 设置
import { computed } from 'vue'
import type { Component } from 'vue'
import { useUiStore } from '../../stores/ui'
import type { PanelKey } from '../../stores/ui'
import FolderTree from './FolderTree.vue'
import QuickAccess from './QuickAccess.vue'
import TabGroups from './TabGroups.vue'
import SettingsPanel from './SettingsPanel.vue'

const ui = useUiStore()

const titles: Record<PanelKey, string> = {
  files: '文件导航',
  quick: '快速访问',
  recent: '最近 / 标签页组',
  settings: '设置',
}

const panels: Record<PanelKey, Component> = {
  files: FolderTree,
  quick: QuickAccess,
  recent: TabGroups,
  settings: SettingsPanel,
}

const title = computed(() => titles[ui.activePanel])
const current = computed(() => panels[ui.activePanel])
</script>

<template>
  <aside class="sidebar-panel">
    <header class="sidebar-head">{{ title }}</header>
    <div class="sidebar-body">
      <component :is="current" />
    </div>
  </aside>
</template>

<style scoped>
.sidebar-panel {
  width: var(--sidebar-w);
  height: 100%;
  background: var(--bg-panel);
  border-right: 1px solid var(--border);
  display: flex;
  flex-direction: column;
  overflow: hidden;
}
.sidebar-head {
  height: var(--toolbar-h);
  display: flex;
  align-items: center;
  padding: 0 10px;
  font-size: 11px;
  text-transform: uppercase;
  letter-spacing: 0.06em;
  color: var(--fg-muted);
  border-bottom: 1px solid var(--border);
  flex: none;
}
.sidebar-body {
  flex: 1;
  overflow: auto;
  padding: 8px;
}
</style>

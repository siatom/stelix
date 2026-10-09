<script setup lang="ts">
// 二级面板容器：按 activePanel 渲染文件导航 / 工作区 / 设置
import { computed } from 'vue'
import type { Component } from 'vue'
import { useUiStore } from '../../stores/ui'
import type { PanelKey } from '../../stores/ui'
import FolderTree from './FolderTree.vue'
import TabGroups from './TabGroups.vue'
import SettingsPanel from './SettingsPanel.vue'
import SearchBar from '../SearchBar.vue'

const ui = useUiStore()

const titles: Record<PanelKey, string> = {
  files: 'File Pivot',
  recent: '最近 / 标签页组',
  search: '搜索',
  settings: '设置',
}

const panels: Record<PanelKey, Component> = {
  files: FolderTree,
  recent: TabGroups,
  search: SearchBar,
  settings: SettingsPanel,
}

const title = computed(() => titles[ui.activePanel])
const current = computed(() => panels[ui.activePanel])
</script>

<template>
  <aside class="sidebar-panel">
    <header class="sidebar-head">{{ title }}</header>
    <div class="sidebar-body" :class="{ 'files-panel': ui.activePanel === 'files' }">
      <component :is="current" />
    </div>
  </aside>
</template>

<style scoped>
.sidebar-panel {
  width: var(--sidebar-w);
  height: 100%;
  background: var(--bg-panel);
  border: 1px solid var(--border);
  border-top: 0;
  border-left: 0;
  border-radius: 0;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}
.sidebar-head {
  height: var(--toolbar-h);
  display: flex;
  align-items: center;
  padding: 0 10px;
  font-size: 20px;
  font-weight: 700;
  color: var(--fg);
  border-bottom: 1px solid transparent;
  flex: none;
}
.sidebar-body {
  flex: 1;
  overflow: auto;
  padding: 6px;
}
.sidebar-body.files-panel {
  padding-top: 14px;
}
</style>

<script setup lang="ts">
// 二级面板容器：按 activePanel 渲染文件导航 / 定时任务 / 搜索
import { computed } from 'vue'
import type { Component } from 'vue'
import { useUiStore } from '../../stores/ui'
import type { PanelKey } from '../../stores/ui'
import FolderTree from './FolderTree.vue'
import TaskPanel from './TaskPanel.vue'
import SearchBar from '../SearchBar.vue'

const ui = useUiStore()

const titles: Record<PanelKey, string> = {
  files: '文件导航',
  tasks: '定时任务',
  search: '搜索',
}

const panels: Record<PanelKey, Component> = {
  files: FolderTree,
  tasks: TaskPanel,
  search: SearchBar,
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
  font-size: 1.1667rem;
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
</style>

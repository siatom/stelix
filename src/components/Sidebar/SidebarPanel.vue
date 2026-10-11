<script setup lang="ts">
// 二级面板容器：按 activePanel 渲染文件导航 / 工作区 / 设置
import { computed } from 'vue'
import type { Component } from 'vue'
import { useUiStore } from '../../stores/ui'
import type { PanelKey } from '../../stores/ui'
import FolderTree from './FolderTree.vue'
import TabGroups from './TabGroups.vue'
import SearchBar from '../SearchBar.vue'
import AiPanel from '../AiPanel.vue'

const ui = useUiStore()

const titles: Record<PanelKey, string> = {
  files: '文件导航',
  recent: '最近 / 标签页组',
  search: '搜索',
  ai: 'AI 助手',
}

const panels: Record<PanelKey, Component> = {
  files: FolderTree,
  recent: TabGroups,
  search: SearchBar,
  ai: AiPanel,
}

const title = computed(() => titles[ui.activePanel])
const current = computed(() => panels[ui.activePanel])
const showHead = computed(() => ui.activePanel !== 'ai')
</script>

<template>
  <aside class="sidebar-panel">
    <header v-if="showHead" class="sidebar-head">{{ title }}</header>
    <div
      class="sidebar-body"
      :class="{
        'files-panel': ui.activePanel === 'files',
        'search-panel': ui.activePanel === 'search',
        'ai-panel-body': ui.activePanel === 'ai',
      }"
    >
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
  min-height: 0;
  overflow: auto;
  padding: 6px;
}
/* 搜索面板：内容自带内边距，底部状态栏需贴边 */
.sidebar-body.search-panel {
  padding: 0;
  display: flex;
  flex-direction: column;
}
/* AI 面板：铺满侧栏，自带头部与输入区内边距 */
.sidebar-body.ai-panel-body {
  padding: 0;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}
</style>

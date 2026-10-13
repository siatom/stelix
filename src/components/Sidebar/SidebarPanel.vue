<script setup lang="ts">
// 二级面板容器：按 activePanel 渲染文件导航 / 定时任务 / 搜索
import { computed } from 'vue'
import type { Component } from 'vue'
import { useUiStore } from '../../stores/ui'
import type { PanelKey } from '../../stores/ui'
import FolderTree from './FolderTree.vue'
import TaskPanel from './TaskPanel.vue'
import SearchBar from '../SearchBar.vue'
import AiPanel from '../AiPanel.vue'

const ui = useUiStore()

const titles: Record<PanelKey, string> = {
  files: '文件导航',
  tasks: '定时任务',
  search: '搜索',
  ai: 'AI 助手',
}

const panels: Record<PanelKey, Component> = {
  files: FolderTree,
  tasks: TaskPanel,
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
  /* 侧边栏字号基准：内部组件一律用 em 相对它，设置面板的「侧边栏字体大小」才生效 */
  font-size: var(--sidebar-font-size);
  /* 与主区靠背景深浅区分（不留间隙、不加描边）；侧栏内部一律跟随侧栏底色 */
  --bg-panel: var(--sidebar-bg);
  --bg-hover: var(--sidebar-hover);
  --bg-active: var(--sidebar-active);
  background: var(--sidebar-bg);
  display: flex;
  flex-direction: column;
  overflow: hidden;
}
.sidebar-head {
  height: var(--toolbar-h);
  display: flex;
  align-items: center;
  padding: 0 10px;
  font-size: 1.1667em;
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

<script setup lang="ts">
// 根布局 —— 由 Phase 0 拥有并冻结。
// 各 workstream 只实现自己被挂载的组件，不要修改本文件的组件树与挂载点。
import { useUiStore } from './stores/ui'
import ActivityBar from './components/Sidebar/ActivityBar.vue'
import SidebarPanel from './components/Sidebar/SidebarPanel.vue'
import SearchBar from './components/SearchBar.vue'
import DualPane from './components/Pane/DualPane.vue'
import AiPanel from './components/AiPanel.vue'
import StatusBar from './components/StatusBar.vue'
import CommandPalette from './components/CommandPalette.vue'
import Icon from './components/icons/Icon.vue'

const ui = useUiStore()
</script>

<template>
  <div class="app-root">
    <ActivityBar class="area-activity" />
    <SidebarPanel v-if="!ui.sidebarCollapsed" class="area-sidebar" />

    <div class="area-main">
      <SearchBar />
      <DualPane />
    </div>

    <AiPanel v-if="ui.aiVisible" class="area-ai" />
    <button
      v-else
      type="button"
      class="area-ai ai-rail"
      title="展开 AI 助手"
      aria-label="展开 AI 助手"
      @click="ui.toggleAi()"
    >
      <Icon name="bot" :size="16" />
    </button>

    <StatusBar class="area-status" />
    <CommandPalette />
  </div>
</template>

<style scoped>
/* AI 面板收起时，右侧保留一个窄条作为展开入口 */
.ai-rail {
  width: 30px;
  height: 100%;
  display: flex;
  align-items: flex-start;
  justify-content: center;
  padding-top: 8px;
  background: var(--bg-panel);
  border-left: 1px solid var(--border);
  color: var(--fg-muted);
  transition:
    background 0.12s ease,
    color 0.12s ease;
}
.ai-rail:hover {
  background: var(--bg-hover);
  color: var(--accent);
}
</style>

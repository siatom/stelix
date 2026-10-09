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

    <StatusBar class="area-status" />
    <CommandPalette />
  </div>
</template>

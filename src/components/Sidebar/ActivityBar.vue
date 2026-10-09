<script setup lang="ts">
// TODO(WS-C / feat/ui-shell): 左侧图标导航栏（文件导航/快速访问/最近/设置）
import { useUiStore } from '../../stores/ui'
import type { PanelKey } from '../../stores/ui'

const ui = useUiStore()

const items: { key: PanelKey; icon: string; title: string }[] = [
  { key: 'files', icon: '📁', title: '文件导航' },
  { key: 'quick', icon: '⚡', title: '快速访问' },
  { key: 'recent', icon: '🕐', title: '最近 / 标签页组' },
  { key: 'settings', icon: '⚙', title: '设置' },
]
</script>

<template>
  <nav class="activity-bar">
    <button
      v-for="it in items"
      :key="it.key"
      class="activity-btn"
      :class="{ active: ui.activePanel === it.key && !ui.sidebarCollapsed }"
      :title="it.title"
      @click="ui.setPanel(it.key)"
    >
      {{ it.icon }}
    </button>
    <div class="spacer" />
    <button class="activity-btn" title="AI 助手" @click="ui.toggleAi()">🤖</button>
  </nav>
</template>

<style scoped>
.activity-bar {
  width: var(--activity-bar-w);
  height: 100%;
  background: var(--bg-panel);
  border-right: 1px solid var(--border);
  display: flex;
  flex-direction: column;
  align-items: center;
  padding: 6px 0;
  gap: 2px;
}
.activity-btn {
  width: 38px;
  height: 38px;
  font-size: 18px;
  border-radius: var(--radius);
  display: flex;
  align-items: center;
  justify-content: center;
}
.activity-btn:hover {
  background: var(--bg-hover);
}
.activity-btn.active {
  background: var(--bg-active);
  box-shadow: inset 2px 0 0 var(--accent);
}
.spacer {
  flex: 1;
}
</style>

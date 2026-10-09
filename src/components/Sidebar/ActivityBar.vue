<script setup lang="ts">
// 左侧图标导航栏：文件导航 / 最近&工作区 / 设置 + AI 助手开关
import { useUiStore } from '../../stores/ui'
import type { PanelKey } from '../../stores/ui'
import Icon from '../icons/Icon.vue'
import type { IconName } from '../icons/paths'

const ui = useUiStore()

const items: { key: PanelKey; icon: IconName; title: string }[] = [
  { key: 'files', icon: 'folder', title: '文件导航' },
  { key: 'recent', icon: 'clock', title: '最近 / 标签页组' },
  { key: 'settings', icon: 'settings', title: '设置' },
]
</script>

<template>
  <nav class="activity-bar" aria-label="主工具栏">
    <button
      v-for="it in items"
      :key="it.key"
      type="button"
      class="activity-btn"
      :class="{ active: ui.activePanel === it.key && !ui.sidebarCollapsed }"
      :title="it.title"
      :aria-label="it.title"
      @click="ui.setPanel(it.key)"
    >
      <Icon :name="it.icon" :size="19" />
    </button>

    <div class="spacer" />

    <div class="divider" />
    <button
      type="button"
      class="activity-btn"
      :class="{ active: ui.aiVisible }"
      :title="ui.aiVisible ? '隐藏 AI 助手' : '显示 AI 助手'"
      aria-label="AI 助手"
      @click="ui.toggleAi()"
    >
      <Icon name="bot" :size="19" />
    </button>

    <button
      type="button"
      class="activity-btn"
      :title="ui.theme === 'dark' ? '切换到浅色主题' : '切换到深色主题'"
      aria-label="切换主题"
      @click="ui.toggleTheme()"
    >
      <Icon :name="ui.theme === 'dark' ? 'sun' : 'moon'" :size="19" />
    </button>
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
  border-radius: var(--radius);
  color: var(--fg-muted);
  display: flex;
  align-items: center;
  justify-content: center;
  position: relative;
  transition:
    background 0.12s ease,
    color 0.12s ease;
}
.activity-btn:hover {
  background: var(--bg-hover);
  color: var(--fg);
}
.activity-btn.active {
  background: var(--bg-active);
  color: var(--accent);
  box-shadow: inset 2px 0 0 var(--accent);
}
.spacer {
  flex: 1;
}
.divider {
  width: 24px;
  height: 1px;
  background: var(--border);
  margin: 4px 0;
}
</style>

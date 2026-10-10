<script setup lang="ts">
// 左侧图标导航栏：文件导航 / 最近&工作区 / 设置 + AI 助手开关
import { useUiStore } from '../../stores/ui'
import type { PanelKey } from '../../stores/ui'
import Icon from '../icons/Icon.vue'
import type { IconName } from '../icons/paths'

const ui = useUiStore()
const isDev = import.meta.env.DEV

const items: { key: PanelKey; icon: IconName; title: string }[] = [
  { key: 'files', icon: 'folder', title: '文件导航' },
  { key: 'recent', icon: 'clock', title: '最近 / 标签页组' },
  { key: 'search', icon: 'search', title: '搜索' },
  { key: 'settings', icon: 'settings', title: '设置' },
]
</script>

<template>
  <nav class="activity-bar" aria-label="主工具栏">
    <div class="brand" :class="{ collapsed: ui.sidebarCollapsed }" aria-label="File Pivot">
      <img v-if="ui.sidebarCollapsed" class="brand-logo" src="/app-icon.svg" alt="" />
      <span v-else class="brand-title">
        File Pivot
        <span v-if="isDev" class="dev-badge">dev</span>
      </span>
    </div>
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
      :title="`主题：${ui.theme === 'system' ? `跟随系统（当前${ui.resolvedTheme === 'dark' ? '深色' : '浅色'}）` : ui.theme === 'dark' ? '深色' : '浅色'}，点击切换`"
      aria-label="切换主题模式"
      @click="ui.toggleTheme()"
    >
      <Icon :name="ui.theme === 'system' ? 'monitor' : ui.theme === 'dark' ? 'sun' : 'moon'" :size="19" />
    </button>
    <button
      type="button"
      class="activity-btn"
      title="收起侧边栏"
      aria-label="收起侧边栏"
      @click="ui.toggleSidebar()"
    >
      <Icon name="panel-left" :size="19" />
    </button>
  </nav>
</template>

<style scoped>
.activity-bar {
  position: relative;
  z-index: 2;
  width: var(--activity-bar-w);
  height: 100%;
  background: var(--bg-panel);
  display: flex;
  flex-direction: column;
  align-items: center;
  padding: 46px 0 6px;
  gap: 2px;
}
.brand {
  position: absolute;
  z-index: 3;
  top: 0;
  left: 0;
  width: calc(var(--activity-bar-w) + var(--sidebar-w));
  /* 高度与侧栏顶部留白一致（.area-sidebar padding-top），避免露出底色形成横线 */
  height: 46px;
  padding: 0 16px;
  display: flex;
  align-items: center;
  color: var(--fg);
  font-size: 1.8333rem;
  font-weight: 700;
  white-space: nowrap;
  background: var(--bg-panel);
}
.brand.collapsed {
  width: var(--activity-bar-w);
  justify-content: center;
  padding: 0;
}
.brand-logo {
  width: 30px;
  height: 30px;
  object-fit: contain;
}
.brand-title {
  position: relative;
}
.dev-badge {
  position: absolute;
  top: -5px;
  left: calc(100% + 5px);
  padding: 2px 4px;
  border-radius: 4px;
  background: var(--bg-active);
  color: var(--fg-muted);
  font-size: 0.75rem;
  font-weight: 600;
  line-height: 1;
  letter-spacing: 0.04em;
  text-transform: uppercase;
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
  color: var(--fg);
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

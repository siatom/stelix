<script setup lang="ts">
// TODO(WS-C / feat/ui-shell): 二级面板容器（目录树 / 快速访问 / 最近 / 设置），可拖拽调宽
import { useUiStore } from '../../stores/ui'

const ui = useUiStore()
</script>

<template>
  <aside class="sidebar-panel">
    <header class="sidebar-head">{{ ui.activePanel }}</header>
    <div class="sidebar-body">
      <!-- TODO(WS-C): 按 activePanel 渲染 FolderTree / QuickAccess / TabGroups / Settings -->
      <p class="hint">面板内容由 WS-C 实现</p>
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
}
.sidebar-body {
  flex: 1;
  overflow: auto;
  padding: 8px;
}
.hint {
  color: var(--fg-dim);
  font-size: 12px;
}
</style>

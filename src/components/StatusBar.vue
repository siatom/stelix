<script setup lang="ts">
// TODO(WS-C / feat/ui-shell): 底部状态栏（选中数 / 条目数 / 索引总数 / 状态文本）
import { useUiStore } from '../stores/ui'

const ui = useUiStore()
</script>

<template>
  <footer class="status-bar">
    <span class="seg">{{ ui.statusText }}</span>
    <div class="spacer" />
    <span class="seg">索引 {{ ui.indexTotal.toLocaleString() }} 项</span>
  </footer>
</template>

<style scoped>
.status-bar {
  height: var(--status-bar-h);
  background: var(--bg-panel);
  border-top: 1px solid var(--border);
  display: flex;
  align-items: center;
  padding: 0 10px;
  font-size: 12px;
  color: var(--fg-muted);
}
.spacer {
  flex: 1;
}
.seg {
  padding: 0 8px;
}
</style>

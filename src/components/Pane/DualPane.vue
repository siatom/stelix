<script setup lang="ts">
// TODO(WS-D / feat/ui-panes): 双窗格容器（左右各一个 Pane，含可拖拽分隔条）
import Pane from './Pane.vue'
</script>

<template>
  <div class="dual-pane">
    <Pane :pane-id="'left'" />
    <div class="divider" />
    <Pane :pane-id="'right'" />
  </div>
</template>

<style scoped>
.dual-pane {
  flex: 1;
  display: grid;
  grid-template-columns: 1fr 1px 1fr;
  min-height: 0;
  overflow: hidden;
}
.divider {
  background: var(--border);
  cursor: col-resize;
}
</style>

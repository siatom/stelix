<script setup lang="ts">
// TODO(WS-C / feat/ui-shell): Ctrl+K 命令面板（承载原顶部菜单的全部能力）
import { ref } from 'vue'

const open = ref(false)
</script>

<template>
  <div v-if="open" class="palette-mask">
    <div class="palette">
      <input placeholder="输入命令…" />
    </div>
  </div>
</template>

<style scoped>
.palette-mask {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.45);
  display: flex;
  justify-content: center;
  padding-top: 12vh;
  z-index: 100;
}
.palette {
  width: 560px;
  background: var(--bg-elevated);
  border: 1px solid var(--border-strong);
  border-radius: 6px;
  padding: 10px;
}
.palette input {
  width: 100%;
}
</style>

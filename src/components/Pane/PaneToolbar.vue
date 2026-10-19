<script setup lang="ts">
// 窗格内嵌工具栏：目录导航。
// 新建 / 删除 / 视图切换已上移到应用顶部标题栏（见 components/TitlebarActions.vue），
// 这里只保留与当前窗格强绑定的前进后退与刷新。
import { usePaneStore } from '../../stores/pane'
import Icon from '../icons/Icon.vue'

const props = defineProps<{ paneId: string }>()

const paneStore = usePaneStore()
</script>

<template>
  <div class="pane-toolbar">
    <div class="group">
      <button title="后退" :disabled="!paneStore.canGoBack(paneId)" @click="paneStore.goBack(paneId)">
        <Icon name="arrow-left" />
      </button>
      <button title="前进" :disabled="!paneStore.canGoForward(paneId)" @click="paneStore.goForward(paneId)">
        <Icon name="arrow-right" />
      </button>
      <button title="上级目录" @click="paneStore.goUp(paneId)">
        <Icon name="arrow-up" />
      </button>
      <button title="刷新" @click="paneStore.refresh(paneId)">
        <Icon name="refresh" />
      </button>
    </div>
  </div>
</template>

<style scoped>
.pane-toolbar {
  display: flex;
  align-items: center;
  gap: 6px;
  height: var(--pane-bar-h);
  padding: 0 6px;
  background: var(--bg-panel);
  border-bottom: 1px solid var(--border);
  overflow: hidden;
}
.group {
  display: flex;
  align-items: center;
  gap: 1px;
  flex: none;
}
.group button {
  width: 24px;
  height: 22px;
  border-radius: var(--radius);
  color: var(--fg-muted);
  font-size: 1.0833rem;
  display: flex;
  align-items: center;
  justify-content: center;
}
.group button:hover:not(:disabled) {
  background: var(--bg-hover);
  color: var(--fg);
}
.group button:disabled {
  opacity: 0.35;
  cursor: default;
}
</style>

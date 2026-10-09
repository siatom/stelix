<script setup lang="ts">
// TODO(WS-D / feat/ui-panes): 单个窗格 = 标签栏 + 内嵌工具栏 + 文件列表（虚拟滚动）
import { computed } from 'vue'
import { usePaneStore } from '../../stores/pane'

const props = defineProps<{ paneId: string }>()

const paneStore = usePaneStore()
const pane = computed(() => paneStore.panes.find((p) => p.id === props.paneId))
const tab = computed(() => {
  const p = pane.value
  if (!p) return undefined
  return p.tabs.find((t) => t.id === p.activeTabId) ?? p.tabs[0]
})
</script>

<template>
  <section class="pane" :class="{ active: paneStore.activePaneId === paneId }" @mousedown="paneStore.activePaneId = paneId">
    <div class="tab-bar">
      <button
        v-for="t in pane?.tabs ?? []"
        :key="t.id"
        class="tab"
        :class="{ active: t.id === pane?.activeTabId }"
        @click="pane && (pane.activeTabId = t.id)"
      >
        {{ t.path.split('\\').pop() || t.path }}
      </button>
      <button class="tab-add" @click="paneStore.addTab(paneId)">+</button>
    </div>
    <div class="toolbar">
      <span class="path">{{ tab?.path ?? '' }}</span>
    </div>
    <div class="list">
      <p class="hint">文件列表由 WS-D 实现（虚拟滚动 + 多视图）</p>
    </div>
  </section>
</template>

<style scoped>
.pane {
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
  background: var(--bg-app);
  border: 1px solid transparent;
}
.pane.active {
  border-color: var(--accent);
}
.tab-bar {
  display: flex;
  align-items: center;
  gap: 2px;
  height: var(--toolbar-h);
  padding: 0 4px;
  background: var(--bg-panel);
  border-bottom: 1px solid var(--border);
  overflow-x: auto;
}
.tab {
  padding: 4px 10px;
  border-radius: var(--radius);
  white-space: nowrap;
  color: var(--fg-muted);
}
.tab.active {
  background: var(--bg-active);
  color: var(--fg);
}
.tab-add {
  padding: 2px 8px;
  color: var(--fg-muted);
}
.toolbar {
  height: var(--toolbar-h);
  display: flex;
  align-items: center;
  padding: 0 10px;
  border-bottom: 1px solid var(--border);
  color: var(--fg-muted);
  font-size: 12px;
}
.path {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.list {
  flex: 1;
  overflow: auto;
  padding: 8px;
}
.hint {
  color: var(--fg-dim);
  font-size: 12px;
}
</style>

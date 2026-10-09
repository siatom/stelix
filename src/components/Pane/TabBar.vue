<script setup lang="ts">
// 标签栏：显示目录名，点击切换，中键 / × 关闭，末尾 + 新建（WS-D）
import { computed } from 'vue'
import { usePaneStore } from '../../stores/pane'
import { basename } from './utils'
import Icon from '../icons/Icon.vue'

const props = defineProps<{ paneId: string }>()
const paneStore = usePaneStore()

const pane = computed(() => paneStore.panes.find((p) => p.id === props.paneId))

function label(path: string): string {
  return basename(path) || path
}

function onMiddle(ev: MouseEvent, tabId: string) {
  if (ev.button === 1) {
    ev.preventDefault()
    paneStore.closeTab(props.paneId, tabId)
  }
}

function onContext(ev: MouseEvent, tabId: string) {
  ev.preventDefault()
  paneStore.duplicateTab(props.paneId, tabId)
}
</script>

<template>
  <div class="tab-bar">
    <button
      v-for="t in pane?.tabs ?? []"
      :key="t.id"
      class="tab"
      :class="{ active: t.id === pane?.activeTabId }"
      :title="t.path"
      @click="paneStore.setActiveTab(paneId, t.id)"
      @mousedown.middle="onMiddle($event, t.id)"
      @contextmenu="onContext($event, t.id)"
    >
      <span class="tab-name">{{ label(t.path) }}</span>
      <span
        v-if="(pane?.tabs.length ?? 0) > 1"
        class="tab-close"
        title="关闭标签"
        @click.stop="paneStore.closeTab(paneId, t.id)"
      >
        <Icon name="x" :size="12" />
      </span>
    </button>
    <button class="tab-add" title="新建标签" @click="paneStore.addTab(paneId)">
      <Icon name="plus" :size="14" />
    </button>
  </div>
</template>

<style scoped>
.tab-bar {
  display: flex;
  align-items: stretch;
  gap: 2px;
  height: var(--toolbar-h);
  padding: 3px 4px 0;
  background: var(--bg-panel);
  border-bottom: 1px solid var(--border);
  overflow-x: auto;
  overflow-y: hidden;
}
.tab-bar::-webkit-scrollbar {
  height: 0;
}
.tab {
  display: flex;
  align-items: center;
  gap: 6px;
  max-width: 180px;
  padding: 0 10px;
  border-radius: var(--radius) var(--radius) 0 0;
  white-space: nowrap;
  color: var(--fg-muted);
  border: 1px solid transparent;
  border-bottom: none;
}
.tab:hover {
  background: var(--bg-hover);
}
.tab.active {
  background: var(--bg-app);
  color: var(--fg);
  border-color: var(--border);
}
.tab-name {
  overflow: hidden;
  text-overflow: ellipsis;
}
.tab-close {
  display: flex;
  align-items: center;
  padding: 1px;
  border-radius: 3px;
  color: var(--fg-dim);
}
.tab-close:hover {
  background: var(--bg-active);
  color: var(--fg);
}
.tab-add {
  display: flex;
  align-items: center;
  padding: 0 10px;
  color: var(--fg-muted);
  border-radius: var(--radius);
}
.tab-add:hover {
  background: var(--bg-hover);
  color: var(--fg);
}
</style>

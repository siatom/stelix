<script setup lang="ts">
// 标签栏（窗格标题栏）：标签切换/关闭 + 新建；右侧双栏动作按钮；活动窗格用底色区分焦点（WS-D）
import { computed } from 'vue'
import { usePaneStore } from '../../stores/pane'
import { combos, formatCombo } from '../../stores/shortcuts'
import { basename, RECYCLE_BIN_SHELL_PATH } from './utils'
import Icon from '../icons/Icon.vue'
import FileIcon from '../icons/FileIcon.vue'

const props = defineProps<{ paneId: string }>()
const paneStore = usePaneStore()

const pane = computed(() => paneStore.panes.find((p) => p.id === props.paneId))
const isActive = computed(() => paneStore.activePaneId === props.paneId)
function label(tab: { path: string; title?: string }): string {
  return tab.title || basename(tab.path) || tab.path
}

/** 动作按钮的提示文案：按窗格方向区分「另一栏」，并附带当前快捷键。 */
function actionTitle(id: string, text: string): string {
  const combo = combos.value[id]
  return combo?.key ? `${text}（${formatCombo(combo)}）` : text
}

const openInOtherPaneTitle = computed(() =>
  actionTitle('open-in-other-pane', `在${props.paneId === 'left' ? '右' : '左'}侧栏打开当前文件夹`),
)
const swapPanesTitle = computed(() => actionTitle('swap-panes', '交换左右栏'))

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
  <div class="tab-bar" :class="{ 'is-active': isActive }">
    <div class="tabs">
      <button
        v-for="t in pane?.tabs ?? []"
        :key="t.id"
        class="tab"
        :class="{ active: t.id === pane?.activeTabId }"
        :title="t.title || t.path"
        @click="paneStore.setActiveTab(paneId, t.id)"
        @mousedown.middle="onMiddle($event, t.id)"
        @contextmenu="onContext($event, t.id)"
      >
        <Icon v-if="t.kind === 'terminal'" class="tab-kind" name="terminal" :size="15" />
        <FileIcon
          v-else-if="t.kind === 'trash'"
          class="tab-kind"
          :path="RECYCLE_BIN_SHELL_PATH"
          :is-dir="false"
          :ext="null"
          :trash-state="true"
          fallback="trash"
          :size="15"
        />
        <span class="tab-name">{{ label(t) }}</span>
        <span
          v-if="(pane?.tabs.length ?? 0) > 1"
          class="tab-close"
          title="关闭标签"
          @click.stop="paneStore.closeTab(paneId, t.id)"
        >
          <Icon name="x" :size="15" />
        </span>
      </button>
      <button class="tab-add" title="新建标签" @click="paneStore.addTab(paneId)">
        <Icon name="plus" :size="15" />
      </button>
    </div>

    <div class="pane-actions">
      <button :title="openInOtherPaneTitle" :aria-label="openInOtherPaneTitle" @click="paneStore.showInOtherPane(paneId)">
        <span class="directional-icon" :class="{ reversed: paneId === 'right' }">
          <Icon name="pane-open-other" :size="15" />
        </span>
      </button>
      <button :title="swapPanesTitle" :aria-label="swapPanesTitle" @click="paneStore.swapPanes()">
        <span class="directional-icon" :class="{ reversed: paneId === 'right' }">
          <Icon name="pane-swap" :size="15" />
        </span>
      </button>
      <button
        title="单窗口"
        aria-label="单窗口"
        :aria-pressed="paneStore.singlePane"
        :class="{ selected: paneStore.singlePane }"
        @click="paneStore.setSinglePane(true)"
      >
        <Icon name="panel-single" :size="15" />
      </button>
      <button
        title="双窗口"
        aria-label="双窗口"
        :aria-pressed="!paneStore.singlePane"
        :class="{ selected: !paneStore.singlePane }"
        @click="paneStore.setSinglePane(false)"
      >
        <Icon name="columns" :size="15" />
      </button>
    </div>
  </div>
</template>

<style scoped>
.tab-bar {
  display: flex;
  align-items: stretch;
  gap: 2px;
  height: var(--pane-bar-h);
  padding: 3px 4px 0;
  background: var(--pane-title-bg);
  border-bottom: 1px solid var(--border);
  transition: background 0.12s ease;
  font-size: var(--file-font-size);
}
.tab-bar.is-active {
  background: var(--pane-title-bg-active);
}
.tabs {
  display: flex;
  align-items: stretch;
  gap: 2px;
  flex: 1;
  min-width: 0;
  overflow-x: auto;
  overflow-y: hidden;
}
.tabs::-webkit-scrollbar {
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
  font-size: inherit;
}
.tab-kind {
  flex: none;
  color: var(--fg-dim);
}
.tab-close {
  display: flex;
  align-items: center;
  padding: 1px;
  border-radius: var(--radius-sm);
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

/* 非活动窗格：标签与动作按钮整体压暗，配合标题栏底色区分焦点 */
.tab-bar:not(.is-active) .tab:not(.active):not(:hover),
.tab-bar:not(.is-active) .tab-add:not(:hover),
.tab-bar:not(.is-active) .pane-actions button:not(:hover) {
  color: var(--fg-dim);
}

.pane-actions {
  display: flex;
  align-items: center;
  gap: 1px;
  flex: none;
  padding-left: 4px;
  border-left: 1px solid var(--border);
}
.pane-actions button {
  width: 22px;
  height: 22px;
  display: flex;
  align-items: center;
  justify-content: center;
  border-radius: var(--radius);
  color: var(--fg-muted);
}
.pane-actions button:hover {
  background: var(--bg-hover);
  color: var(--fg);
}
.pane-actions button.selected {
  color: var(--fg);
  background: var(--bg-active);
}
.directional-icon {
  display: flex;
  transform-origin: center;
}
.directional-icon.reversed {
  transform: scaleX(-1);
}
</style>

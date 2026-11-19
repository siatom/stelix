<script setup lang="ts">
// 设置 · 快捷键：集中列出全部可配置快捷键，支持改键 / 单条恢复 / 全部恢复。
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import Icon from '../icons/Icon.vue'
import { useUiStore } from '../../stores/ui'
import {
  SHORTCUT_GROUP_LABELS,
  assignCombo,
  capturingId,
  combos,
  eventToCombo,
  formatCombo,
  isCustomized,
  resetAllShortcuts,
  resetShortcut,
  shortcutItems,
  setCapturing,
} from '../../stores/shortcuts'
import type { ShortcutGroup, ShortcutItem } from '../../stores/shortcuts'
import { t } from '../../i18n'

const GROUP_ORDER: ShortcutGroup[] = ['tabs', 'files', 'window', 'global']
const labelEn: Record<string, string> = {
  '标签页': 'Tabs', '文件': 'Files', '文件操作': 'File operations', '窗口': 'Window', '全局': 'Global',
  '新建标签页': 'New tab', '关闭当前标签页': 'Close current tab',
  '复制': 'Copy', '剪切': 'Cut', '粘贴': 'Paste', '复制路径': 'Copy path', '复制文件名': 'Copy file name',
  '复制到…': 'Copy to…', '移动到…': 'Move to…', '撤销上一步操作': 'Undo last action', '重做': 'Redo',
  '复制副本': 'Duplicate', '重命名': 'Rename', '删除（移到回收站）': 'Delete (move to Recycle Bin)',
  '永久删除（跳过回收站）': 'Delete permanently (skip Recycle Bin)', '预览文件': 'Preview file',
  '关闭窗口': 'Close window', '在另一栏打开当前文件夹': 'Open current folder in other pane',
  '交换左右栏': 'Swap panes', '打开搜索面板': 'Open search panel', '打开命令面板': 'Open command palette',
}
function localizedLabel(value: string): string {
  return t(value, labelEn[value] ?? value)
}

const ui = useUiStore()
const error = ref<string | null>(null)

// 依赖 combos 触发刷新：改键后当前键位即时更新。
const items = computed<ShortcutItem[]>(() => {
  void combos.value
  return shortcutItems()
})

const groups = computed(() =>
  GROUP_ORDER.map((group) => ({
    group,
    label: localizedLabel(SHORTCUT_GROUP_LABELS[group]),
    items: items.value.filter((item) => item.group === group),
  })).filter((g) => g.items.length > 0),
)

const customizedCount = computed(() => items.value.filter((i) => isCustomized(i.id)).length)

function beginCapture(item: ShortcutItem) {
  error.value = null
  setCapturing(capturingId.value === item.id ? null : item.id)
}

function onCaptureKeydown(event: KeyboardEvent, item: ShortcutItem) {
  // 捕获态吞掉一切按键，避免触发浏览器默认行为 / 应用其它快捷键。
  event.preventDefault()
  event.stopPropagation()

  if (event.key === 'Escape') {
    setCapturing(null)
    return
  }
  if (event.key === 'Backspace') {
    const res = assignCombo(item.id, { key: '' })
    error.value = res.ok ? null : res.message
    setCapturing(null)
    return
  }

  const combo = eventToCombo(event)
  if (!combo) return // 仅按下了修饰键，继续等待
  const res = assignCombo(item.id, combo)
  if (res.ok) {
    error.value = null
    setCapturing(null)
  } else {
    error.value = res.message
  }
}

function resetOne(item: ShortcutItem) {
  error.value = null
  setCapturing(null)
  resetShortcut(item.id)
}

/** 焦点离开键位按钮时退出捕获态，避免失焦后仍拦截全局按键。 */
function onComboBlur(item: ShortcutItem) {
  if (capturingId.value === item.id) setCapturing(null)
}

function resetAll() {
  error.value = null
  setCapturing(null)
  resetAllShortcuts()
}

// 离开该分类 / 组件卸载时退出捕获态，避免拦截全局按键。
watch(
  () => ui.settingsTab,
  (tab) => {
    if (tab !== 'shortcuts') setCapturing(null)
  },
)
onBeforeUnmount(() => setCapturing(null))
</script>

<template>
  <section class="sc-card">
    <header class="sc-head">
      <span>{{ t('__legacy_21a782d018bb') }}</span>
      <button
        v-if="customizedCount"
        class="head-action"
        type="button"
        :title="t('全部恢复默认')"
        @click="resetAll"
      >
        {{ t('全部恢复默认') }}
      </button>
    </header>
    <div class="sc-body">
      <div v-for="g in groups" :key="g.group" class="group">
        <div class="group-label">{{ localizedLabel(g.label) }}</div>
        <div
          v-for="item in g.items"
          :key="item.id"
          class="sc-row"
          :class="{ capturing: capturingId === item.id }"
        >
          <span class="sc-label">{{ localizedLabel(item.label) }}</span>
          <div class="sc-actions">
            <button
              v-if="isCustomized(item.id)"
              class="sc-reset"
              type="button"
              :title="t(`恢复「${item.label}」为默认`, `Reset ${localizedLabel(item.label)} to default`)"
              @click="resetOne(item)"
            >
              <Icon name="refresh" :size="12" />
            </button>
            <button
              class="sc-combo"
              type="button"
              :class="{ capture: capturingId === item.id, unset: !item.current.key }"
              :title="capturingId === item.id ? t('按下新的组合键，Esc 取消，退格清除') : t('点击修改键位')"
              @click="beginCapture(item)"
              @keydown="capturingId === item.id ? onCaptureKeydown($event, item) : undefined"
              @blur="onComboBlur(item)"
            >
              {{ capturingId === item.id ? t('按下新键…') : formatCombo(item.current) }}
            </button>
          </div>
        </div>
      </div>
      <p v-if="error" class="sc-error">{{ error }}</p>
      <p class="sc-hint">{{ t('点击键位后按下新的组合键；退格清除绑定，Esc 取消。') }}</p>
    </div>
  </section>
</template>

<style scoped>
.sc-card {
  background: var(--bg-elevated);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  overflow: hidden;
  box-shadow: var(--shadow-sm);
}
.sc-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 10px 12px;
  font-size: 1rem;
  font-weight: 600;
  color: var(--fg-muted);
  border-bottom: 1px solid var(--border);
}
.head-action {
  padding: 2px 8px;
  border-radius: var(--radius-sm);
  color: var(--fg-muted);
  font-size: 0.9167rem;
  font-weight: 400;
}
.head-action:hover {
  background: var(--bg-hover);
  color: var(--fg);
}
.sc-body {
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding: 6px;
}
.group {
  display: flex;
  flex-direction: column;
  gap: 1px;
}
.group + .group {
  margin-top: 6px;
}
.group-label {
  padding: 6px 8px 2px;
  font-size: 0.9167rem;
  color: var(--fg-dim);
}
.sc-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 4px 8px;
  min-height: 32px;
  box-sizing: border-box;
  border-radius: var(--radius-sm);
}
.sc-row:hover {
  background: var(--bg-hover);
}
.sc-row.capturing {
  background: var(--bg-active);
}
.sc-label {
  font-size: 1rem;
  color: var(--fg);
}
.sc-actions {
  display: flex;
  align-items: center;
  gap: 6px;
}
.sc-reset {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 22px;
  height: 22px;
  border-radius: var(--radius-sm);
  color: var(--fg-dim);
}
.sc-reset:hover {
  background: var(--bg-hover);
  color: var(--fg);
}
.sc-combo {
  flex: none;
  min-width: 108px;
  height: 24px;
  padding: 0 8px;
  border: 1px solid var(--border-strong);
  border-radius: var(--radius-sm);
  background: var(--bg-input);
  color: var(--fg);
  font-family: var(--mono);
  font-size: 0.9167rem;
}
.sc-combo:hover {
  border-color: var(--accent);
}
.sc-combo.capture {
  border-color: var(--accent);
  color: var(--accent);
}
.sc-combo.unset {
  color: var(--fg-dim);
}
.sc-error {
  margin: 4px 0 0;
  padding: 4px 8px;
  border-radius: var(--radius-sm);
  background: var(--danger-soft);
  color: var(--danger);
  font-size: 0.9167rem;
}
.sc-hint {
  margin: 0;
  padding: 4px 8px 0;
  font-size: 0.9167rem;
  color: var(--fg-dim);
}
</style>

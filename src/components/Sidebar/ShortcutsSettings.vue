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

const GROUP_ORDER: ShortcutGroup[] = ['tabs', 'files', 'window', 'global']

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
    label: SHORTCUT_GROUP_LABELS[group],
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
      <span>快捷键</span>
      <button
        v-if="customizedCount"
        class="head-action"
        type="button"
        title="全部恢复默认"
        @click="resetAll"
      >
        全部恢复默认
      </button>
    </header>
    <div class="sc-body">
      <div v-for="g in groups" :key="g.group" class="group">
        <div class="group-label">{{ g.label }}</div>
        <div
          v-for="item in g.items"
          :key="item.id"
          class="sc-row"
          :class="{ capturing: capturingId === item.id }"
        >
          <span class="sc-label">{{ item.label }}</span>
          <div class="sc-actions">
            <button
              v-if="isCustomized(item.id)"
              class="sc-reset"
              type="button"
              :title="`恢复「${item.label}」为默认`"
              @click="resetOne(item)"
            >
              <Icon name="refresh" :size="12" />
            </button>
            <button
              class="sc-combo"
              type="button"
              :class="{ capture: capturingId === item.id, unset: !item.current.key }"
              :title="capturingId === item.id ? '按下新的组合键，Esc 取消，退格清除' : '点击修改键位'"
              @click="beginCapture(item)"
              @keydown="capturingId === item.id ? onCaptureKeydown($event, item) : undefined"
              @blur="onComboBlur(item)"
            >
              {{ capturingId === item.id ? '按下新键…' : formatCombo(item.current) }}
            </button>
          </div>
        </div>
      </div>
      <p v-if="error" class="sc-error">{{ error }}</p>
      <p class="sc-hint">点击键位后按下新的组合键；退格清除绑定，Esc 取消。</p>
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

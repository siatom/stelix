<script setup lang="ts">
// 文件筛选面板：关键词 / 类型 / 扩展名 / 大小 / 修改时间，可选递归子文件夹。
import { computed, nextTick, onBeforeUnmount, reactive, ref, watch } from 'vue'
import type { TabState } from '../../types'
import { usePaneStore } from '../../stores/pane'
import { useUiStore } from '../../stores/ui'
import { SEARCH_CATEGORIES, categoryOf } from '../../searchFilters'
import Icon from '../icons/Icon.vue'

const props = defineProps<{ tab: TabState }>()
const paneStore = usePaneStore()
const ui = useUiStore()

type SizeUnit = 'b' | 'kb' | 'mb' | 'gb'
const UNIT_SIZE: Record<SizeUnit, number> = {
  b: 1,
  kb: 1024,
  mb: 1024 * 1024,
  gb: 1024 * 1024 * 1024,
}
const UNIT_LABEL: Record<SizeUnit, string> = { b: 'B', kb: 'KB', mb: 'MB', gb: 'GB' }
const SIZE_UNITS: SizeUnit[] = ['b', 'kb', 'mb', 'gb']
const MODIFIED_OPTIONS = [
  { value: null, label: '不限' },
  { value: 1, label: '最近 1 天' },
  { value: 7, label: '最近 7 天' },
  { value: 30, label: '最近 30 天' },
  { value: 365, label: '最近一年' },
] as const

const triggerRef = ref<HTMLButtonElement | null>(null)
const panelRef = ref<HTMLElement | null>(null)
const open = ref(false)
const position = reactive({ top: 0, left: 0, width: 356 })
const form = reactive({
  text: '',
  type: 'all',
  exts: '',
  minSize: '',
  minUnit: 'mb' as SizeUnit,
  maxSize: '',
  maxUnit: 'mb' as SizeUnit,
  modifiedDays: null as number | null,
  recursive: true,
})

const isLocal = computed(() => (props.tab.kind ?? 'files') === 'files')
const active = computed(() => paneStore.hasTabFilter(props.tab))
const summary = computed(() => {
  const parts: string[] = []
  if (props.tab.filter.trim()) parts.push(`名称「${props.tab.filter.trim()}」`)
  const type = categoryOf(props.tab.typeFilter)
  if (type) parts.push(type.label)
  const rule = props.tab.filterRule
  if (rule?.exts.length) parts.push(`扩展名 ${rule.exts.join(', ')}`)
  if (rule?.minSize != null || rule?.maxSize != null) {
    parts.push(`大小 ${rule?.minSize != null ? formatBytes(rule.minSize) : '不限'}–${rule?.maxSize != null ? formatBytes(rule.maxSize) : '不限'}`)
  }
  if (rule?.modifiedWithinDays != null) parts.push(`最近 ${rule.modifiedWithinDays} 天`)
  if (rule?.recursive) parts.push('含子文件夹')
  return parts.join(' · ')
})
const triggerTitle = computed(() =>
  active.value ? `筛选中：${summary.value}` : '筛选当前文件夹',
)

function formatBytes(bytes: number): string {
  if (bytes >= UNIT_SIZE.gb) return `${trimNumber(bytes / UNIT_SIZE.gb)} GB`
  if (bytes >= UNIT_SIZE.mb) return `${trimNumber(bytes / UNIT_SIZE.mb)} MB`
  if (bytes >= UNIT_SIZE.kb) return `${trimNumber(bytes / UNIT_SIZE.kb)} KB`
  return `${bytes} B`
}

function trimNumber(value: number): string {
  return Number(value.toFixed(2)).toString()
}

function bestUnit(bytes: number | null): SizeUnit {
  if (bytes == null || bytes <= 0) return 'mb'
  if (bytes >= UNIT_SIZE.gb) return 'gb'
  if (bytes >= UNIT_SIZE.mb) return 'mb'
  if (bytes >= UNIT_SIZE.kb) return 'kb'
  return 'b'
}

function bytesToInput(bytes: number | null, unit: SizeUnit): string {
  if (bytes == null) return ''
  return trimNumber(bytes / UNIT_SIZE[unit])
}

function parseSize(value: string, unit: SizeUnit): number | null {
  const text = value.trim().replace(',', '.')
  if (!text) return null
  const num = Number(text)
  if (!Number.isFinite(num) || num < 0) return null
  return Math.round(num * UNIT_SIZE[unit])
}

function parseExts(value: string): string[] {
  return Array.from(
    new Set(
      value
        .split(/[\s,;]+/)
        .map((ext) => ext.trim().replace(/^\./, '').toLowerCase())
        .filter(Boolean),
    ),
  )
}

function syncForm() {
  const rule = props.tab.filterRule
  form.text = props.tab.filter
  form.type = props.tab.typeFilter ?? 'all'
  form.exts = rule?.exts.join(', ') ?? ''
  const minUnit = bestUnit(rule?.minSize ?? null)
  const maxUnit = bestUnit(rule?.maxSize ?? null)
  form.minUnit = minUnit
  form.maxUnit = maxUnit
  form.minSize = bytesToInput(rule?.minSize ?? null, minUnit)
  form.maxSize = bytesToInput(rule?.maxSize ?? null, maxUnit)
  form.modifiedDays = rule?.modifiedWithinDays ?? null
  // 首次打开（没有已保存规则）默认递归，用户可手动关闭；远程目录不支持递归。
  form.recursive = isLocal.value && (rule ? !!rule.recursive : true)
}

function resetForm() {
  form.text = ''
  form.type = 'all'
  form.exts = ''
  form.minSize = ''
  form.maxSize = ''
  form.minUnit = 'mb'
  form.maxUnit = 'mb'
  form.modifiedDays = null
  form.recursive = true
}

/** 面板贴着按钮右下角展开；空间不足时向上翻转，并始终限制在窗口内。 */
function updatePosition() {
  const trigger = triggerRef.value
  if (!trigger) return
  const rect = trigger.getBoundingClientRect()
  const margin = 8
  const width = Math.min(356, window.innerWidth - margin * 2)
  let left = rect.right - width
  left = Math.max(margin, Math.min(left, window.innerWidth - width - margin))
  const height = panelRef.value?.offsetHeight ?? 380
  let top = rect.bottom + 6
  if (top + height > window.innerHeight - margin && rect.top - height - 6 >= margin) {
    top = rect.top - height - 6
  }
  position.top = Math.round(top)
  position.left = Math.round(left)
  position.width = Math.round(width)
}

async function togglePanel() {
  if (open.value) {
    closePanel()
    return
  }
  syncForm()
  open.value = true
  window.addEventListener('pointerdown', onDocPointerDown, true)
  window.addEventListener('keydown', onKeyDown, true)
  window.addEventListener('resize', updatePosition)
  window.addEventListener('scroll', updatePosition, true)
  await nextTick()
  updatePosition()
}

function closePanel() {
  if (!open.value) return
  open.value = false
  window.removeEventListener('pointerdown', onDocPointerDown, true)
  window.removeEventListener('keydown', onKeyDown, true)
  window.removeEventListener('resize', updatePosition)
  window.removeEventListener('scroll', updatePosition, true)
}

function onDocPointerDown(event: PointerEvent) {
  const target = event.target as Node | null
  if (!target) return
  if (panelRef.value?.contains(target) || triggerRef.value?.contains(target)) return
  closePanel()
}

function onKeyDown(event: KeyboardEvent) {
  if (event.key !== 'Escape') return
  event.stopPropagation()
  closePanel()
}

async function apply() {
  const minSize = parseSize(form.minSize, form.minUnit)
  const maxSize = parseSize(form.maxSize, form.maxUnit)
  if (minSize !== null && maxSize !== null && minSize > maxSize) {
    ui.setStatusText('筛选失败：最小大小不能大于最大大小')
    return
  }
  const rule = {
    text: form.text,
    type: form.type === 'all' ? null : form.type,
    exts: parseExts(form.exts),
    minSize,
    maxSize,
    modifiedWithinDays: form.modifiedDays,
    // 远程 / 搜索结果没有本地目录树，递归选项不生效。
    recursive: isLocal.value && form.recursive,
  }
  closePanel()
  await paneStore.applyTabFilter(props.tab, rule)
}

function reset() {
  if (active.value) {
    paneStore.clearTabFilter(props.tab)
    closePanel()
    return
  }
  resetForm()
}

watch(
  () => [props.tab.id, props.tab.path],
  () => closePanel(),
)
watch(
  () => props.tab.kind,
  () => {
    if (!isLocal.value) form.recursive = false
  },
)

onBeforeUnmount(closePanel)
</script>

<template>
  <div class="filter-picker">
    <button
      ref="triggerRef"
      type="button"
      class="filter-button"
      :class="{ active }"
      :title="triggerTitle"
      aria-label="筛选当前文件夹"
      :aria-expanded="open"
      @click="togglePanel"
    >
      <Icon name="filter" :size="15" />
      <span v-if="active" class="active-dot" aria-hidden="true" />
    </button>

    <Teleport to="body">
      <transition name="filter-pop">
        <form
          v-if="open"
          ref="panelRef"
          class="filter-panel"
          :style="{
            top: `${position.top}px`,
            left: `${position.left}px`,
            width: `${position.width}px`,
          }"
          role="dialog"
          aria-label="筛选当前文件夹"
          @submit.prevent="apply"
        >
          <header class="fp-head">
            <span class="fp-title">
              <Icon name="filter" :size="14" />
              筛选文件
            </span>
            <button
              type="button"
              class="fp-close"
              title="关闭"
              aria-label="关闭筛选面板"
              @click="closePanel"
            >
              <Icon name="x" :size="14" />
            </button>
          </header>

          <div class="fp-body">
            <label class="fp-field">
              <span class="fp-label">名称</span>
              <input
                v-model="form.text"
                class="fp-input"
                spellcheck="false"
                placeholder="文件名包含…（空格分隔多个关键词）"
              />
            </label>

            <div class="fp-field">
              <span class="fp-label">类型</span>
              <select v-model="form.type" class="fp-select">
                <option v-for="category in SEARCH_CATEGORIES" :key="category.key" :value="category.key">
                  {{ category.label }}
                </option>
              </select>
            </div>

            <label class="fp-field">
              <span class="fp-label">扩展名</span>
              <input
                v-model="form.exts"
                class="fp-input"
                spellcheck="false"
                placeholder="如 psd, ai（留空则按类型）"
              />
            </label>

            <div class="fp-field">
              <span class="fp-label">大小</span>
              <div class="fp-size-row">
                <input
                  v-model="form.minSize"
                  class="fp-input size"
                  inputmode="decimal"
                  placeholder="最小"
                />
                <select v-model="form.minUnit" class="fp-unit" aria-label="最小大小的单位">
                  <option v-for="unit in SIZE_UNITS" :key="unit" :value="unit">
                    {{ UNIT_LABEL[unit] }}
                  </option>
                </select>
                <span class="fp-dash">–</span>
                <input
                  v-model="form.maxSize"
                  class="fp-input size"
                  inputmode="decimal"
                  placeholder="最大"
                />
                <select v-model="form.maxUnit" class="fp-unit" aria-label="最大大小的单位">
                  <option v-for="unit in SIZE_UNITS" :key="unit" :value="unit">
                    {{ UNIT_LABEL[unit] }}
                  </option>
                </select>
              </div>
            </div>

            <div class="fp-field">
              <span class="fp-label">修改时间</span>
              <select v-model="form.modifiedDays" class="fp-select">
                <option v-for="option in MODIFIED_OPTIONS" :key="String(option.value)" :value="option.value">
                  {{ option.label }}
                </option>
              </select>
            </div>

            <button
              type="button"
              class="fp-recursive"
              :class="{ on: form.recursive }"
              :disabled="!isLocal"
              :title="isLocal ? '同时扫描当前目录下的所有子文件夹' : '仅本地目录支持递归筛选'"
              :aria-pressed="form.recursive"
              @click="form.recursive = !form.recursive"
            >
              <span class="fp-switch" :class="{ on: form.recursive }" aria-hidden="true">
                <span class="fp-knob" />
              </span>
              <span class="fp-recursive-text">
                <span class="t">包含子文件夹</span>
                <span class="h">{{ isLocal ? '递归扫描所有层级' : '仅本地目录支持递归' }}</span>
              </span>
            </button>
          </div>

          <footer class="fp-foot">
            <span class="fp-hint">自定义扩展名优先于类型</span>
            <button type="button" class="fp-reset" @click="reset">重置</button>
            <button type="submit" class="fp-apply">应用</button>
          </footer>
        </form>
      </transition>
    </Teleport>
  </div>
</template>

<style scoped>
.filter-picker {
  flex: none;
  display: flex;
  align-items: center;
}
.filter-button {
  position: relative;
  flex: none;
  width: 28px;
  height: 28px;
  display: flex;
  align-items: center;
  justify-content: center;
  border-radius: 50%;
  color: var(--fg-muted);
}
.filter-button:hover {
  background: var(--bg-hover);
  color: var(--fg);
}
.filter-button.active {
  background: var(--accent-soft);
  color: var(--accent);
}
.active-dot {
  position: absolute;
  right: 5px;
  bottom: 5px;
  width: 5px;
  height: 5px;
  border-radius: 50%;
  background: var(--accent);
  box-shadow: 0 0 0 2px var(--bg-panel);
}

.filter-panel {
  position: fixed;
  z-index: 260;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  border: 1px solid var(--border-strong);
  border-radius: 12px;
  background: var(--bg-elevated);
  box-shadow: var(--shadow-lg);
  font-size: var(--file-font-size);
}
.fp-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  height: 38px;
  padding: 0 8px 0 12px;
  border-bottom: 1px solid var(--border);
}
.fp-title {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  color: var(--fg);
  font-weight: 600;
}
.fp-title :deep(svg) {
  color: var(--fg-muted);
}
.fp-close {
  display: grid;
  place-items: center;
  width: 24px;
  height: 24px;
  border-radius: 6px;
  color: var(--fg-dim);
}
.fp-close:hover {
  background: var(--bg-hover);
  color: var(--fg);
}
.fp-body {
  display: flex;
  flex-direction: column;
  gap: 9px;
  padding: 11px 12px 12px;
}
.fp-field {
  display: flex;
  align-items: center;
  gap: 10px;
  min-width: 0;
}
.fp-label {
  flex: none;
  width: 56px;
  color: var(--fg-muted);
  font-size: 0.9375em;
}
.fp-input,
.fp-select {
  flex: 1;
  min-width: 0;
  height: 30px;
  padding: 0 8px;
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--bg-input);
  color: var(--fg);
  font: inherit;
  font-size: 0.9375em;
  outline: none;
}
.fp-input::placeholder {
  color: var(--fg-dim);
}
.fp-input:focus,
.fp-select:focus {
  border-color: var(--border-strong);
  box-shadow: 0 0 0 3px var(--accent-soft);
}
.fp-select {
  appearance: none;
  padding-right: 24px;
  background-image: linear-gradient(45deg, transparent 50%, var(--fg-dim) 50%),
    linear-gradient(135deg, var(--fg-dim) 50%, transparent 50%);
  background-position:
    calc(100% - 13px) 13px,
    calc(100% - 9px) 13px;
  background-size: 4px 4px, 4px 4px;
  background-repeat: no-repeat;
  cursor: pointer;
}
.fp-size-row {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: center;
  gap: 4px;
}
.fp-input.size {
  width: 0;
}
.fp-unit {
  flex: none;
  height: 30px;
  padding: 0 4px;
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--bg-input);
  color: var(--fg-muted);
  font: inherit;
  font-size: 0.875em;
  outline: none;
}
.fp-unit:focus {
  border-color: var(--border-strong);
}
.fp-dash {
  flex: none;
  color: var(--fg-dim);
}
.fp-recursive {
  display: flex;
  align-items: center;
  gap: 9px;
  width: 100%;
  margin-top: 1px;
  padding: 7px 8px;
  border-radius: 9px;
  text-align: left;
}
.fp-recursive:hover:not(:disabled) {
  background: var(--bg-hover);
}
.fp-recursive:disabled {
  opacity: 0.5;
  cursor: default;
}
.fp-switch {
  flex: none;
  position: relative;
  width: 30px;
  height: 17px;
  border-radius: 999px;
  background: var(--border-strong);
  transition: background var(--t-fast);
}
.fp-switch.on {
  background: var(--accent);
}
.fp-knob {
  position: absolute;
  top: 2px;
  left: 2px;
  width: 13px;
  height: 13px;
  border-radius: 50%;
  background: #fff;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.25);
  transition: transform var(--t-fast);
}
.fp-switch.on .fp-knob {
  transform: translateX(13px);
}
.fp-recursive-text {
  display: flex;
  flex-direction: column;
  gap: 1px;
  min-width: 0;
}
.fp-recursive-text .t {
  color: var(--fg);
  font-size: 0.9375em;
}
.fp-recursive-text .h {
  color: var(--fg-dim);
  font-size: 0.875em;
}
.fp-foot {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 9px 12px;
  border-top: 1px solid var(--border);
  background: var(--bg-panel);
}
.fp-hint {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  color: var(--fg-dim);
  font-size: 0.875em;
  white-space: nowrap;
  text-overflow: ellipsis;
}
.fp-reset,
.fp-apply {
  flex: none;
  height: 28px;
  padding: 0 12px;
  border-radius: 7px;
  font-size: 0.9375em;
}
.fp-reset {
  color: var(--fg-muted);
}
.fp-reset:hover {
  background: var(--bg-hover);
  color: var(--fg);
}
.fp-apply {
  background: var(--accent);
  color: var(--accent-fg);
  font-weight: 600;
}
.fp-apply:hover {
  opacity: 0.88;
}

.filter-pop-enter-active,
.filter-pop-leave-active {
  transition:
    opacity 0.12s ease,
    transform 0.12s ease;
  transform-origin: top right;
}
.filter-pop-enter-from,
.filter-pop-leave-to {
  opacity: 0;
  transform: translateY(-4px) scale(0.98);
}
</style>

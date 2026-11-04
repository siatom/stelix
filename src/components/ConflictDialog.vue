<script setup lang="ts">
// 同名冲突处理：复制 / 移动前预检到目标已存在同名项时，让用户统一选择处理策略。
// 后端按单一策略处理整批冲突，所以这里只需一次选择（不需要逐项询问）。
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import type { ConflictItem, ConflictPolicy } from '../types'
import Icon from './icons/Icon.vue'
import type { IconName } from './icons/paths'

const props = defineProps<{
  /** 预检到的冲突项。 */
  items: ConflictItem[]
  /** 操作类型，用于文案（复制 / 移动）。 */
  kind: 'copy' | 'move'
}>()

const emit = defineEmits<{
  (e: 'resolve', policy: ConflictPolicy | 'cancel'): void
}>()

interface Option {
  value: ConflictPolicy
  label: string
  desc: string
  icon: IconName
}

const options: Option[] = [
  { value: 'keepBoth', label: '保留两者', desc: '自动为新项改名，如「报告 (1).docx」', icon: 'copy' },
  { value: 'replace', label: '替换目标', desc: '删除目标位置的同名项，写入新项', icon: 'refresh' },
  { value: 'skip', label: '跳过冲突项', desc: '保留目标位置的原文件，不改动它', icon: 'arrow-right' },
]

const policy = ref<ConflictPolicy>('keepBoth')

/** 只列前若干项，避免大批量操作把弹窗撑爆。 */
const preview = computed(() => props.items.slice(0, 5))
const restCount = computed(() => Math.max(0, props.items.length - preview.value.length))
const actionText = computed(() => (props.kind === 'move' ? '移动' : '复制'))

/** 路径末段（兼容 / 与 \\ 分隔符）。 */
function baseName(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean)
  return parts.length ? parts[parts.length - 1] : path
}

function choose(value: ConflictPolicy) {
  policy.value = value
}

function confirm() {
  emit('resolve', policy.value)
}

function onKeydown(event: KeyboardEvent) {
  if (event.isComposing) return
  if (event.key === 'Escape') {
    event.preventDefault()
    emit('resolve', 'cancel')
  } else if (event.key === 'Enter') {
    event.preventDefault()
    confirm()
  }
}

onMounted(() => window.addEventListener('keydown', onKeydown, true))
onBeforeUnmount(() => window.removeEventListener('keydown', onKeydown, true))
</script>

<template>
  <div class="mask" @mousedown.stop @click.self="emit('resolve', 'cancel')">
    <div class="dialog" role="dialog" aria-modal="true" aria-labelledby="conflict-title">
      <header class="dialog-head">
        <h3 id="conflict-title">
          <Icon name="alert-triangle" :size="14" class="head-icon" />
          目标位置已存在同名项
        </h3>
        <span class="count">{{ items.length }} 处冲突</span>
      </header>

      <p class="lead">
        继续{{ actionText }}前请选择处理方式，将对本次全部冲突统一生效。
      </p>

      <div class="preview" role="list">
        <div v-for="item in preview" :key="item.path" class="preview-row" role="listitem">
          <Icon :name="item.isDir ? 'folder' : 'file'" :size="13" class="kind" />
          <span class="name" :title="item.source">{{ baseName(item.source) }}</span>
          <Icon name="arrow-right" :size="12" class="arrow" />
          <span class="where" :title="item.path">{{ item.path }}</span>
        </div>
        <div v-if="restCount" class="preview-more">还有 {{ restCount }} 处冲突未列出</div>
      </div>

      <div class="options" role="radiogroup" aria-label="冲突处理方式">
        <button
          v-for="option in options"
          :key="option.value"
          type="button"
          class="option"
          :class="{ active: policy === option.value }"
          role="radio"
          :aria-checked="policy === option.value"
          @click="choose(option.value)"
        >
          <span class="radio" aria-hidden="true"><span class="dot" /></span>
          <Icon :name="option.icon" :size="14" class="opt-icon" />
          <span class="opt-text">
            <span class="opt-label">{{ option.label }}</span>
            <span class="opt-desc">{{ option.desc }}</span>
          </span>
        </button>
      </div>

      <footer class="actions">
        <button type="button" @click="emit('resolve', 'cancel')">取消</button>
        <button type="button" class="primary" @click="confirm">继续{{ actionText }}</button>
      </footer>
    </div>
  </div>
</template>

<style scoped>
.mask {
  position: fixed;
  inset: 0;
  z-index: 320;
  display: flex;
  align-items: center;
  justify-content: center;
  background: var(--mask);
}
.dialog {
  display: flex;
  flex-direction: column;
  gap: 10px;
  width: 460px;
  max-width: calc(100vw - 32px);
  padding: 14px;
  border: 1px solid var(--border-strong);
  border-radius: var(--radius-lg);
  background: var(--bg-elevated);
  box-shadow: var(--shadow-lg);
}
.dialog-head {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 8px;
}
.dialog-head h3 {
  display: flex;
  align-items: center;
  gap: 6px;
  margin: 0;
  font-size: 1.0833rem;
  font-weight: 600;
  color: var(--fg);
}
.head-icon {
  flex: none;
  color: var(--warn);
}
.count {
  flex: none;
  color: var(--fg-dim);
  font-size: 0.9167rem;
}
.lead {
  margin: 0;
  color: var(--fg-muted);
  font-size: 0.9167rem;
}
.preview {
  display: flex;
  flex-direction: column;
  max-height: 168px;
  padding: 4px;
  overflow: auto;
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  background: var(--bg-app);
}
.preview-row {
  display: flex;
  align-items: center;
  gap: 6px;
  min-height: 22px;
  padding: 1px 4px;
  border-radius: var(--radius-sm);
  font-size: 0.9167rem;
}
.kind {
  flex: none;
  color: var(--fg-dim);
}
.name {
  flex: 0 1 auto;
  max-width: 45%;
  overflow: hidden;
  color: var(--fg);
  white-space: nowrap;
  text-overflow: ellipsis;
}
.arrow {
  flex: none;
  color: var(--fg-dim);
}
.where {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  color: var(--fg-dim);
  white-space: nowrap;
  text-overflow: ellipsis;
  direction: rtl;
  text-align: left;
}
.preview-more {
  padding: 2px 4px;
  color: var(--fg-dim);
  font-size: 0.8333rem;
}
.options {
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.option {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  padding: 6px 8px;
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  background: var(--bg-app);
  color: var(--fg);
  text-align: left;
}
.option:hover {
  background: var(--bg-hover);
}
.option.active {
  border-color: var(--accent);
  background: color-mix(in srgb, var(--accent) 10%, var(--bg-app));
}
.radio {
  display: flex;
  align-items: center;
  justify-content: center;
  flex: none;
  width: 13px;
  height: 13px;
  border: 1px solid var(--border-strong);
  border-radius: 50%;
}
.option.active .radio {
  border-color: var(--accent);
}
.dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: transparent;
}
.option.active .dot {
  background: var(--accent);
}
.opt-icon {
  flex: none;
  color: var(--fg-muted);
}
.option.active .opt-icon {
  color: var(--accent);
}
.opt-text {
  display: flex;
  flex-direction: column;
  gap: 1px;
  min-width: 0;
}
.opt-label {
  font-size: 0.9167rem;
}
.opt-desc {
  color: var(--fg-dim);
  font-size: 0.8333rem;
}
.actions {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 8px;
}
.actions button {
  height: 27px;
  padding: 0 12px;
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  background: var(--bg-app);
  color: var(--fg);
  font-size: 0.9167rem;
}
.actions button:hover:not(:disabled) {
  background: var(--bg-hover);
}
.actions button.primary {
  border-color: var(--accent);
  background: var(--accent);
  color: var(--accent-fg);
}
.actions button.primary:hover:not(:disabled) {
  background: color-mix(in srgb, var(--accent) 86%, var(--fg));
}
</style>

<script setup lang="ts">
// 批量重命名：用模板 + 序号生成新名称，确认前实时预览与校验。
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import type { FileEntry } from '../../types'
import { dirname, joinPath } from './utils'
import Icon from '../icons/Icon.vue'

const props = defineProps<{
  /** 当前选中的条目。 */
  entries: FileEntry[]
  /** 当前目录的全部条目（用于检测与未选中项重名）。 */
  siblings: FileEntry[]
}>()

const emit = defineEmits<{
  (e: 'cancel'): void
  (e: 'confirm', plans: { from: string; to: string }[]): void
}>()

const template = ref('{name}{ext}')
const start = ref(1)
const width = ref(2)
const inputEl = ref<HTMLInputElement | null>(null)

/** Windows 文件名不允许的字符 / 保留名称（与后端校验保持一致）。 */
const INVALID_CHARS = /[\\/:*?"<>|\u0000-\u001f]/
const RESERVED_NAMES = /^(con|prn|aux|nul|com[1-9]|lpt[1-9])(?:\..*)?$/i

interface Plan {
  entry: FileEntry
  oldName: string
  newName: string
  changed: boolean
  error: string | null
}

function splitName(entry: FileEntry): { stem: string; ext: string } {
  if (entry.isDir) return { stem: entry.name, ext: '' }
  const dot = entry.name.lastIndexOf('.')
  // 以点开头的文件（.gitignore）没有扩展名语义。
  if (dot <= 0) return { stem: entry.name, ext: '' }
  return { stem: entry.name.slice(0, dot), ext: entry.name.slice(dot) }
}

const selectedPaths = computed(() => new Set(props.entries.map((entry) => entry.path)))
const siblingNames = computed(
  () =>
    new Set(
      props.siblings
        .filter((entry) => !selectedPaths.value.has(entry.path))
        .map((entry) => entry.name.toLowerCase()),
    ),
)

const plans = computed<Plan[]>(() => {
  const seqWidth = Math.min(6, Math.max(1, Math.round(Number(width.value) || 1)))
  const base = Math.max(0, Math.round(Number(start.value) || 0))
  const list = props.entries.map((entry, index) => {
    const { stem, ext } = splitName(entry)
    const seq = String(base + index).padStart(seqWidth, '0')
    const newName = template.value
      .replace(/\{name\}/g, () => stem)
      .replace(/\{ext\}/g, () => ext)
      .replace(/\{n\}/g, () => seq)
    const changed = newName !== entry.name
    let error: string | null = null
    if (!newName.trim()) error = '名称不能为空'
    else if (newName.length > 255) error = '名称过长'
    else if (INVALID_CHARS.test(newName)) error = '包含非法字符'
    else if (newName.endsWith('.') || newName.endsWith(' ')) error = '不能以句点或空格结尾'
    else if (RESERVED_NAMES.test(newName)) error = 'Windows 保留名称'
    else if (changed && siblingNames.value.has(newName.toLowerCase())) error = '已存在同名项'
    return { entry, oldName: entry.name, newName, changed, error }
  })

  // 目标名称重复：标记所有冲突项（大小写不敏感，符合 Windows 语义）。
  const counts = new Map<string, number>()
  for (const plan of list) {
    if (plan.error) continue
    const key = plan.newName.toLowerCase()
    counts.set(key, (counts.get(key) ?? 0) + 1)
  }
  for (const plan of list) {
    if (!plan.error && (counts.get(plan.newName.toLowerCase()) ?? 0) > 1) {
      plan.error = '目标名称重复'
    }
  }
  return list
})

const errorCount = computed(() => plans.value.filter((plan) => plan.error).length)
const changedPlans = computed(() => plans.value.filter((plan) => plan.changed && !plan.error))
const canApply = computed(() => errorCount.value === 0 && changedPlans.value.length > 0)
/** 模板没带扩展名时给出提示：文件会丢掉原扩展名。 */
const missingExt = computed(
  () => !template.value.includes('{ext}') && props.entries.some((entry) => !entry.isDir),
)

function confirm() {
  if (!canApply.value) return
  emit(
    'confirm',
    changedPlans.value.map((plan) => ({
      from: plan.entry.path,
      to: joinPath(dirname(plan.entry.path), plan.newName),
    })),
  )
}

function onKeydown(event: KeyboardEvent) {
  if (event.isComposing) return
  if (event.key === 'Escape') {
    event.preventDefault()
    emit('cancel')
  } else if (event.key === 'Enter') {
    event.preventDefault()
    confirm()
  }
}

onMounted(() => {
  window.addEventListener('keydown', onKeydown, true)
  inputEl.value?.focus()
  inputEl.value?.select()
})

onBeforeUnmount(() => window.removeEventListener('keydown', onKeydown, true))
</script>

<template>
  <div class="mask" @mousedown.stop @click.self="emit('cancel')">
    <div class="dialog" role="dialog" aria-modal="true" aria-labelledby="batch-rename-title">
      <header class="dialog-head">
        <h3 id="batch-rename-title">批量重命名</h3>
        <span class="count">{{ entries.length }} 项</span>
      </header>

      <label class="field">
        <span class="field-label">命名模板</span>
        <input ref="inputEl" v-model="template" spellcheck="false" placeholder="{name}{ext}" />
      </label>
      <p class="hint">
        占位符：<code>{name}</code> 原名、<code>{ext}</code> 扩展名、<code>{n}</code> 序号
      </p>
      <p v-if="missingExt" class="hint warn">模板未包含 <code>{ext}</code>，文件将失去原扩展名。</p>

      <div class="number-row">
        <label class="field compact">
          <span class="field-label">起始序号</span>
          <input v-model.number="start" type="number" min="0" step="1" />
        </label>
        <label class="field compact">
          <span class="field-label">序号位数</span>
          <input v-model.number="width" type="number" min="1" max="6" step="1" />
        </label>
      </div>

      <div class="preview" role="list">
        <div
          v-for="plan in plans"
          :key="plan.entry.path"
          class="preview-row"
          :class="{ bad: plan.error, dim: !plan.changed }"
          role="listitem"
        >
          <span class="old" :title="plan.oldName">{{ plan.oldName }}</span>
          <Icon name="arrow-right" :size="12" class="arrow" />
          <span class="new" :title="plan.newName">{{ plan.newName }}</span>
          <span v-if="plan.error" class="err">{{ plan.error }}</span>
        </div>
      </div>

      <footer class="actions">
        <span class="summary" :class="{ bad: errorCount > 0 }">
          {{ errorCount ? `${errorCount} 项有问题` : `${changedPlans.length} 项将被重命名` }}
        </span>
        <button type="button" @click="emit('cancel')">取消</button>
        <button type="button" class="primary" :disabled="!canApply" @click="confirm">重命名</button>
      </footer>
    </div>
  </div>
</template>

<style scoped>
.mask {
  position: fixed;
  inset: 0;
  z-index: 300;
  display: flex;
  align-items: center;
  justify-content: center;
  background: var(--mask);
}
.dialog {
  display: flex;
  flex-direction: column;
  gap: 9px;
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
  margin: 0;
  font-size: 1.0833rem;
  font-weight: 600;
  color: var(--fg);
}
.count {
  color: var(--fg-dim);
  font-size: 0.9167rem;
}
.field {
  display: flex;
  align-items: center;
  gap: 8px;
}
.field-label {
  flex: none;
  color: var(--fg-muted);
  font-size: 0.9167rem;
}
.field input {
  flex: 1;
  min-width: 0;
  height: 28px;
  padding: 0 8px;
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  background: var(--bg-app);
  color: var(--fg);
  font: inherit;
}
.field input:focus {
  outline: none;
  border-color: var(--accent);
}
.number-row {
  display: flex;
  gap: 14px;
}
.field.compact {
  flex: 1;
}
.field.compact input {
  max-width: 90px;
}
.hint {
  margin: 0;
  color: var(--fg-dim);
  font-size: 0.8333rem;
}
.hint.warn {
  color: var(--danger);
}
.hint code {
  padding: 0 3px;
  border-radius: 3px;
  background: var(--bg-hover);
  font-family: var(--font-mono, ui-monospace, Consolas, monospace);
}
.preview {
  display: flex;
  flex-direction: column;
  max-height: 240px;
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
.preview-row.dim {
  color: var(--fg-dim);
}
.preview-row.bad {
  color: var(--danger);
}
.old,
.new {
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}
.old {
  flex: 1;
  min-width: 0;
  color: var(--fg-muted);
}
.new {
  flex: 1;
  min-width: 0;
  color: var(--fg);
}
.preview-row.dim .new {
  color: var(--fg-dim);
}
.arrow {
  flex: none;
  color: var(--fg-dim);
}
.err {
  flex: none;
  font-size: 0.8333rem;
}
.actions {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 8px;
}
.summary {
  flex: 1;
  margin-right: auto;
  color: var(--fg-muted);
  font-size: 0.9167rem;
}
.summary.bad {
  color: var(--danger);
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
.actions button:disabled {
  opacity: 0.4;
  cursor: default;
}
</style>

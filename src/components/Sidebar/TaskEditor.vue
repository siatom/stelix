<script setup lang="ts">
// 定时任务编辑器：AI 自然语言生成草稿 → 表单确认/调整 → 预演 → 保存
import { ref } from 'vue'
import { useTasksStore } from '../../stores/tasks'
import { usePaneStore } from '../../stores/pane'
import { useUiStore } from '../../stores/ui'
import Icon from '../icons/Icon.vue'
import type { ScheduledTask, TaskAction, TaskPreview, TaskSchedule } from '../../types'

const props = defineProps<{ initial: ScheduledTask }>()
const emit = defineEmits<{ close: [] }>()

const tasks = useTasksStore()
const pane = usePaneStore()
const ui = useUiStore()

const WEEKDAYS = ['日', '一', '二', '三', '四', '五', '六']

function clone<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T
}

const form = ref<ScheduledTask>(clone(props.initial))
const extText = ref((props.initial.action.filters.extensions ?? []).join(', '))
const aiPrompt = ref('')
const planning = ref(false)
const planText = ref('')
const preview = ref<TaskPreview | null>(null)
const previewing = ref(false)
const saving = ref(false)
const saveError = ref('')

function textValue(e: Event): string {
  return (e.target as HTMLInputElement).value
}

function numOrNull(e: Event): number | null {
  const raw = (e.target as HTMLInputElement).value.trim()
  if (raw === '') return null
  const n = Number(raw)
  return Number.isFinite(n) ? n : null
}

function setScheduleKind(e: Event) {
  form.value.schedule.kind = (e.target as HTMLSelectElement).value as TaskSchedule['kind']
}

function setActionKind(e: Event) {
  form.value.action.kind = (e.target as HTMLSelectElement).value as TaskAction['kind']
}

/** 把逗号/空格分隔的扩展名文本解析为规范化数组。 */
function parseExts(): string[] {
  return extText.value
    .split(/[,，、\s]+/)
    .map((s) => s.trim().replace(/^\./, '').toLowerCase())
    .filter(Boolean)
}

function buildAction(): TaskAction {
  const action = clone(form.value.action)
  action.filters.extensions = parseExts()
  return action
}

async function generate() {
  const prompt = aiPrompt.value.trim()
  if (!prompt) return
  planning.value = true
  planText.value = ''
  try {
    const tab = pane.activeTab
    const reply = await tasks.plan(prompt, {
      activePath: tab?.path ?? null,
      selected: tab?.selected ?? [],
    })
    planText.value = reply.text
    if (reply.draft) {
      form.value.name = reply.draft.name
      form.value.schedule = reply.draft.schedule
      form.value.action = reply.draft.action
      extText.value = (reply.draft.action.filters.extensions ?? []).join(', ')
      preview.value = null
    }
  } catch (e) {
    planText.value = `规划失败：${String(e)}`
  } finally {
    planning.value = false
  }
}

async function doPreview() {
  previewing.value = true
  try {
    preview.value = await tasks.preview(buildAction())
  } catch (e) {
    ui.setStatusText(`预演失败：${String(e)}`)
  } finally {
    previewing.value = false
  }
}

function useCurrentDir() {
  form.value.action.root = pane.activeTab?.path ?? ''
}

async function submit() {
  const task = clone(form.value)
  task.action = buildAction()
  saving.value = true
  saveError.value = ''
  try {
    await tasks.save(task)
    ui.setStatusText('定时任务已保存')
    emit('close')
  } catch (e) {
    saveError.value = String(e)
  } finally {
    saving.value = false
  }
}

function formatSize(bytes: number): string {
  const units = ['B', 'KB', 'MB', 'GB']
  let value = bytes
  let unit = 0
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024
    unit += 1
  }
  return unit === 0 ? `${bytes} B` : `${value.toFixed(1)} ${units[unit]}`
}
</script>

<template>
  <div class="task-editor">
    <div class="te-head">
      <button class="te-back" @click="emit('close')">
        <Icon name="chevron-left" :size="13" />返回
      </button>
      <span class="te-title">{{ form.id ? '编辑任务' : '新建任务' }}</span>
    </div>

    <section class="te-card ai">
      <div class="te-label"><Icon name="sparkles" :size="12" /> 用 AI 生成</div>
      <textarea
        v-model="aiPrompt"
        rows="2"
        placeholder="例如：每天 9 点清理下载文件夹里超过 30 天的 .tmp 文件"
        spellcheck="false"
      />
      <button class="te-btn primary sm" :disabled="planning || !aiPrompt.trim()" @click="generate">
        {{ planning ? '生成中…' : 'AI 生成' }}
      </button>
      <p v-if="planText" class="te-hint">{{ planText }}</p>
    </section>

    <section class="te-card">
      <div class="te-label">基本信息</div>
      <label class="te-field">
        <span>名称</span>
        <input v-model="form.name" placeholder="任务名称" spellcheck="false" />
      </label>
      <label class="te-field row">
        <span>启用</span>
        <input
          type="checkbox"
          :checked="form.enabled"
          @change="form.enabled = ($event.target as HTMLInputElement).checked"
        />
      </label>
    </section>

    <section class="te-card">
      <div class="te-label">触发时间</div>
      <label class="te-field">
        <span>频率</span>
        <select :value="form.schedule.kind" @change="setScheduleKind">
          <option value="interval">每 N 分钟</option>
          <option value="daily">每天</option>
          <option value="weekly">每周</option>
          <option value="monthly">每月</option>
        </select>
      </label>
      <label v-if="form.schedule.kind === 'interval'" class="te-field">
        <span>间隔(分钟)</span>
        <input
          type="number"
          min="1"
          :value="form.schedule.intervalMinutes ?? 60"
          @change="form.schedule.intervalMinutes = numOrNull($event)"
        />
      </label>
      <template v-else>
        <label v-if="form.schedule.kind === 'weekly'" class="te-field">
          <span>星期</span>
          <select
            :value="form.schedule.weekday ?? 1"
            @change="form.schedule.weekday = Number(($event.target as HTMLSelectElement).value)"
          >
            <option v-for="(w, i) in WEEKDAYS" :key="i" :value="i">周{{ w }}</option>
          </select>
        </label>
        <label v-if="form.schedule.kind === 'monthly'" class="te-field">
          <span>日期</span>
          <input
            type="number"
            min="1"
            max="28"
            :value="form.schedule.dayOfMonth ?? 1"
            @change="form.schedule.dayOfMonth = numOrNull($event)"
          />
        </label>
        <div class="te-field">
          <span>时间</span>
          <div class="te-time">
            <input
              aria-label="小时"
              type="number"
              min="0"
              max="23"
              :value="form.schedule.hour ?? 0"
              @change="form.schedule.hour = numOrNull($event)"
            />
            <em>:</em>
            <input
              aria-label="分钟"
              type="number"
              min="0"
              max="59"
              :value="form.schedule.minute ?? 0"
              @change="form.schedule.minute = numOrNull($event)"
            />
          </div>
        </div>
      </template>
    </section>

    <section class="te-card">
      <div class="te-label">操作</div>
      <label class="te-field">
        <span>动作</span>
        <select :value="form.action.kind" @change="setActionKind">
          <option value="delete">删除</option>
          <option value="move">移动到</option>
        </select>
      </label>
      <label class="te-field">
        <span>目录</span>
        <input
          :value="form.action.root"
          placeholder="D:\下载"
          spellcheck="false"
          @input="form.action.root = textValue($event)"
        />
      </label>
      <button class="te-btn ghost sm" @click="useCurrentDir">使用当前目录</button>
      <label v-if="form.action.kind === 'move'" class="te-field">
        <span>目标</span>
        <input
          :value="form.action.moveTarget ?? ''"
          placeholder="D:\归档"
          spellcheck="false"
          @input="form.action.moveTarget = textValue($event)"
        />
      </label>
      <label class="te-field row">
        <span>含子目录</span>
        <input
          type="checkbox"
          :checked="form.action.recursive"
          @change="form.action.recursive = ($event.target as HTMLInputElement).checked"
        />
      </label>
    </section>

    <section class="te-card">
      <div class="te-label">筛选条件（至少一项）</div>
      <label class="te-field">
        <span>扩展名</span>
        <input v-model="extText" placeholder="tmp, log（逗号分隔）" spellcheck="false" />
      </label>
      <label class="te-field">
        <span>名称含</span>
        <input
          :value="form.action.filters.nameContains ?? ''"
          placeholder="可选"
          spellcheck="false"
          @input="form.action.filters.nameContains = textValue($event)"
        />
      </label>
      <label class="te-field">
        <span>早于(天)</span>
        <input
          type="number"
          min="0"
          :value="form.action.filters.minAgeDays ?? ''"
          @change="form.action.filters.minAgeDays = numOrNull($event)"
        />
      </label>
    </section>

    <section class="te-card">
      <button class="te-btn sm" :disabled="previewing" @click="doPreview">
        <Icon name="play" :size="12" />{{ previewing ? '预演中…' : '预演（不执行）' }}
      </button>
      <div v-if="preview" class="te-preview">
        <p class="te-hint">命中 {{ preview.count }} 个文件，共 {{ formatSize(preview.totalSize) }}</p>
        <ul v-if="preview.samples.length">
          <li v-for="s in preview.samples.slice(0, 8)" :key="s" :title="s">{{ s }}</li>
        </ul>
        <p v-if="preview.truncated" class="te-hint">…仅显示前 8 条</p>
      </div>
    </section>

    <p v-if="saveError" class="te-error">{{ saveError }}</p>

    <div class="te-actions">
      <button class="te-btn ghost" @click="emit('close')">取消</button>
      <button class="te-btn primary" :disabled="saving" @click="submit">
        {{ saving ? '保存中…' : '保存' }}
      </button>
    </div>
  </div>
</template>

<style scoped>
.task-editor {
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 4px;
}
.te-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 2px 0 4px;
}
.te-back {
  display: flex;
  align-items: center;
  gap: 4px;
  min-height: 28px;
  padding: 0 6px;
  border-radius: var(--radius-sm);
  color: var(--fg-muted);
  font-size: 1rem;
}
.te-back:hover {
  color: var(--fg);
  background: var(--bg-hover);
}
.te-title {
  font-size: 1.0833rem;
  font-weight: 600;
  color: var(--fg);
}
.te-card {
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 12px;
  min-width: 0;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--bg-elevated);
}
.te-card.ai {
  border-color: var(--border-strong);
  background: var(--bg-hover);
}
.te-card.ai > .te-btn {
  align-self: flex-end;
}
.te-label {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 1rem;
  font-weight: 600;
  color: var(--fg);
}
.te-field {
  display: flex;
  flex-direction: column;
  align-items: stretch;
  gap: 6px;
  font-size: 0.9167rem;
  color: var(--fg-muted);
}
.te-field > span {
  flex: none;
  line-height: 1.4;
}
.te-field input,
.te-field select {
  min-width: 0;
  width: 100%;
  height: 32px;
  padding: 0 9px;
  border: 1px solid var(--border-strong);
  border-radius: var(--radius-sm);
  background: var(--bg-input);
  color: var(--fg);
  font-family: inherit;
  font-size: 1rem;
  outline: none;
  transition: border-color var(--t-fast), box-shadow var(--t-fast);
}
.te-field input:focus-visible,
.te-field select:focus-visible,
.te-card textarea:focus-visible {
  border-color: var(--accent);
  box-shadow: 0 0 0 2px var(--accent-soft);
}
.te-field.row {
  flex-direction: row;
  align-items: center;
  justify-content: space-between;
  min-height: 28px;
}
.te-field.row input[type='checkbox'] {
  flex: none;
  width: 16px;
  height: 16px;
  accent-color: var(--accent);
  cursor: pointer;
}
.te-time {
  display: flex;
  align-items: center;
  gap: 8px;
}
.te-time input {
  width: 0;
  flex: 1;
  text-align: center;
}
.te-time em {
  color: var(--fg-dim);
  font-style: normal;
}
.te-card textarea {
  width: 100%;
  min-height: 80px;
  resize: vertical;
  font-family: inherit;
  font-size: 1rem;
  line-height: 1.6;
  color: var(--fg);
  background: var(--bg-input);
  border: 1px solid var(--border-strong);
  border-radius: var(--radius-sm);
  padding: 8px 10px;
  outline: none;
}
.te-card textarea:focus {
  border-color: var(--accent);
}
.te-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 4px;
  min-height: 32px;
  padding: 0 12px;
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  background: var(--bg-input);
  color: var(--fg-muted);
  font-size: 1rem;
  transition: background var(--t-fast), border-color var(--t-fast);
}
.te-btn:hover:not(:disabled) {
  background: var(--bg-hover);
  color: var(--fg);
}
.te-btn:disabled {
  opacity: 0.45;
  cursor: default;
}
.te-btn.sm {
  min-height: 30px;
  font-size: 0.9167rem;
}
.te-btn.primary {
  background: var(--accent);
  border-color: var(--accent);
  color: var(--accent-fg);
}
.te-btn.ghost {
  background: transparent;
}
.te-btn.primary:hover:not(:disabled) {
  background: var(--accent);
  color: var(--accent-fg);
  filter: brightness(0.9);
}
.te-btn:focus-visible,
.te-back:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: 2px;
}
.te-hint {
  margin: 0;
  font-size: 0.875rem;
  line-height: 1.45;
  color: var(--fg-dim);
  white-space: pre-wrap;
  word-break: break-word;
}
.te-error {
  margin: 0;
  padding: 5px 6px;
  border: 1px solid var(--danger);
  border-radius: var(--radius);
  font-size: 0.875rem;
  line-height: 1.45;
  color: var(--danger);
  word-break: break-word;
}
.te-preview ul {
  list-style: none;
  margin: 4px 0 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding: 8px;
  border-radius: var(--radius-sm);
  background: var(--bg-hover);
}
.te-preview li {
  font-size: 0.8333rem;
  color: var(--fg-muted);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.te-actions {
  position: sticky;
  bottom: -6px;
  z-index: 1;
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  padding: 10px 0;
  border-top: 1px solid var(--border);
  background: var(--bg-panel);
}
.te-actions .te-btn {
  flex: 1;
}
</style>

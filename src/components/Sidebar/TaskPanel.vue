<script setup lang="ts">
// 定时任务面板：任务列表 + 新建/编辑（AI 生成规格 + 预演）
import { onMounted, ref } from 'vue'
import { useTasksStore } from '../../stores/tasks'
import { usePaneStore } from '../../stores/pane'
import { useUiStore } from '../../stores/ui'
import Icon from '../icons/Icon.vue'
import TaskEditor from './TaskEditor.vue'
import type { ScheduledTask } from '../../types'

const tasks = useTasksStore()
const pane = usePaneStore()
const ui = useUiStore()

const editing = ref<ScheduledTask | null>(null)
const WEEKDAYS = ['日', '一', '二', '三', '四', '五', '六']

function blankTask(): ScheduledTask {
  return {
    id: '',
    name: '',
    enabled: true,
    schedule: { kind: 'daily', hour: 9, minute: 0 },
    action: {
      kind: 'delete',
      root: pane.activeTab?.path ?? '',
      recursive: true,
      moveTarget: null,
      filters: { extensions: [] },
    },
    createdAt: 0,
    runCount: 0,
  }
}

function startCreate() {
  editing.value = blankTask()
}

function startEdit(task: ScheduledTask) {
  editing.value = JSON.parse(JSON.stringify(task)) as ScheduledTask
}

async function onSave(task: ScheduledTask) {
  try {
    await tasks.save(task)
    editing.value = null
    ui.setStatusText('定时任务已保存')
  } catch (e) {
    ui.setStatusText(`保存失败：${String(e)}`)
  }
}

async function runNow(task: ScheduledTask) {
  try {
    ui.setStatusText(await tasks.runNow(task.id))
  } catch (e) {
    ui.setStatusText(`运行失败：${String(e)}`)
  }
}

async function toggle(task: ScheduledTask) {
  try {
    await tasks.setEnabled(task.id, !task.enabled)
  } catch (e) {
    ui.setStatusText(String(e))
  }
}

async function remove(task: ScheduledTask) {
  try {
    await tasks.remove(task.id)
    ui.setStatusText('已删除任务')
  } catch (e) {
    ui.setStatusText(String(e))
  }
}

function scheduleText(task: ScheduledTask): string {
  const s = task.schedule
  const hm = `${String(s.hour ?? 0).padStart(2, '0')}:${String(s.minute ?? 0).padStart(2, '0')}`
  switch (s.kind) {
    case 'interval':
      return `每 ${s.intervalMinutes ?? 0} 分钟`
    case 'weekly':
      return `每周${WEEKDAYS[s.weekday ?? 0]} ${hm}`
    case 'monthly':
      return `每月 ${s.dayOfMonth ?? 1} 日 ${hm}`
    default:
      return `每天 ${hm}`
  }
}

function actionText(task: ScheduledTask): string {
  const a = task.action
  const verb = a.kind === 'move' ? `移动到 ${a.moveTarget ?? ''}` : '删除'
  const conds: string[] = []
  if (a.filters.extensions.length) conds.push(a.filters.extensions.map((e) => `.${e}`).join('/'))
  if (a.filters.nameContains) conds.push(`含「${a.filters.nameContains}」`)
  if (a.filters.minAgeDays != null) conds.push(`早于 ${a.filters.minAgeDays} 天`)
  const cond = conds.length ? `中的 ${conds.join('、')} 文件` : '中的文件'
  return `${verb} ${a.root} ${cond}`
}

function formatTime(ms?: number | null): string {
  if (!ms) return '—'
  return new Date(ms).toLocaleString('zh-CN', { hour12: false })
}

onMounted(async () => {
  await tasks.load()
  await tasks.subscribe()
})
</script>

<template>
  <TaskEditor
    v-if="editing"
    :key="editing.id || 'new'"
    :initial="editing"
    @save="onSave"
    @close="editing = null"
  />
  <div v-else class="task-panel">
    <button class="new-btn" @click="startCreate">
      <Icon name="plus" :size="13" />
      <span>新建任务</span>
    </button>

    <p v-if="tasks.error" class="err">{{ tasks.error }}</p>
    <p v-else-if="!tasks.tasks.length" class="state">
      暂无定时任务。点击上方按钮，或用 AI 描述你的清理需求。
    </p>

    <ul v-else class="task-list">
      <li v-for="t in tasks.tasks" :key="t.id" class="task-item">
        <div class="ti-head">
          <span class="ti-name" :title="t.name">{{ t.name || '未命名任务' }}</span>
          <input
            type="checkbox"
            class="ti-toggle"
            :checked="t.enabled"
            :title="t.enabled ? '已启用，点击停用' : '已停用，点击启用'"
            @change="toggle(t)"
          />
        </div>
        <div class="ti-meta">
          <span class="ti-when"><Icon name="clock" :size="11" />{{ scheduleText(t) }}</span>
          <span class="ti-act" :title="actionText(t)">{{ actionText(t) }}</span>
          <span class="ti-run">
            {{ t.enabled ? `下次 ${formatTime(t.nextRun)}` : '已停用' }}
            <template v-if="t.lastResult"> · 上次：{{ t.lastResult }}</template>
          </span>
        </div>
        <div class="ti-actions">
          <button class="ti-btn" :disabled="tasks.runningId === t.id" @click="runNow(t)">
            <Icon name="play" :size="11" />{{ tasks.runningId === t.id ? '运行中…' : '立即运行' }}
          </button>
          <button class="ti-btn" @click="startEdit(t)"><Icon name="pencil" :size="11" />编辑</button>
          <button class="ti-btn danger" @click="remove(t)"><Icon name="trash" :size="11" />删除</button>
        </div>
      </li>
    </ul>
  </div>
</template>

<style scoped>
.task-panel {
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.new-btn {
  width: 100%;
  height: 34px;
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  border-radius: var(--radius-sm);
  border: 1px solid var(--border-strong);
  background: var(--bg-elevated);
  color: var(--fg);
  font-size: 1rem;
}
.new-btn:hover {
  background: var(--bg-hover);
  color: var(--fg);
  border-color: var(--accent);
}
.state {
  margin: 0;
  padding: 4px 6px;
  font-size: 0.9167rem;
  line-height: 1.5;
  color: var(--fg-dim);
}
.err {
  margin: 0;
  padding: 4px 6px;
  font-size: 0.9167rem;
  color: var(--danger);
}
.task-list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.task-item {
  display: flex;
  flex-direction: column;
  gap: 4px;
  padding: 6px;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--bg-elevated);
}
.ti-head {
  display: flex;
  align-items: center;
  gap: 6px;
}
.ti-name {
  flex: 1;
  min-width: 0;
  font-size: 1rem;
  font-weight: 600;
  color: var(--fg);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.ti-toggle {
  flex: none;
  width: 14px;
  height: 14px;
}
.ti-meta {
  display: flex;
  flex-direction: column;
  gap: 2px;
  font-size: 0.875rem;
  color: var(--fg-muted);
}
.ti-when {
  display: flex;
  align-items: center;
  gap: 4px;
  color: var(--accent);
}
.ti-act,
.ti-run {
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.ti-run {
  color: var(--fg-dim);
}
.ti-actions {
  display: flex;
  gap: 4px;
  margin-top: 2px;
}
.ti-btn {
  display: flex;
  align-items: center;
  gap: 3px;
  padding: 2px 8px;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  font-size: 0.875rem;
  color: var(--fg-muted);
}
.ti-btn:hover:not(:disabled) {
  background: var(--bg-hover);
  color: var(--fg);
}
.ti-btn:disabled {
  opacity: 0.5;
  cursor: default;
}
.ti-btn.danger:hover {
  border-color: var(--danger);
  color: var(--danger);
}
</style>

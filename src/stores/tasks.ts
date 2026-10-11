import { defineStore } from 'pinia'
import { ref } from 'vue'
import { api, events } from '../api'
import type { AiContext, ScheduledTask, TaskAction, TaskPlanReply, TaskPreview } from '../types'

/** 定时任务状态：规格列表 + 后端调度事件的订阅。 */
export const useTasksStore = defineStore('tasks', () => {
  const tasks = ref<ScheduledTask[]>([])
  const loading = ref(false)
  const error = ref<string | null>(null)
  const runningId = ref<string | null>(null)
  let unlisten: (() => void) | null = null

  async function load() {
    loading.value = true
    try {
      tasks.value = await api.taskList()
      error.value = null
    } catch (e) {
      error.value = String(e)
    } finally {
      loading.value = false
    }
  }

  /** 新增或更新任务（后端会校验并重算下次触发时间）。 */
  async function save(task: ScheduledTask): Promise<ScheduledTask> {
    const saved = await api.taskSave(task)
    await load()
    return saved
  }

  async function remove(id: string) {
    await api.taskDelete(id)
    await load()
  }

  async function setEnabled(id: string, enabled: boolean) {
    await api.taskSetEnabled(id, enabled)
    await load()
  }

  async function runNow(id: string): Promise<string> {
    runningId.value = id
    try {
      return await api.taskRunNow(id)
    } finally {
      runningId.value = null
      await load()
    }
  }

  function preview(action: TaskAction): Promise<TaskPreview> {
    return api.taskPreview(action)
  }

  function plan(prompt: string, context: AiContext): Promise<TaskPlanReply> {
    return api.aiPlanTask(prompt, context)
  }

  /** 订阅后端调度事件（幂等，只订阅一次）。 */
  async function subscribe() {
    if (unlisten) return
    try {
      unlisten = await events.onTaskUpdated(() => {
        void load()
      })
    } catch {
      unlisten = null
    }
  }

  return {
    tasks,
    loading,
    error,
    runningId,
    load,
    save,
    remove,
    setEnabled,
    runNow,
    preview,
    plan,
    subscribe,
  }
})

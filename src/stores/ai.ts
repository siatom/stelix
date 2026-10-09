import { defineStore } from 'pinia'
import { ref } from 'vue'
import { api, events } from '../api'
import type { AiConfig, AiMessage, AiToolCall, FsOp, ToolOutcome } from '../types'
import { usePaneStore } from './pane'
import { useUiStore } from './ui'

/** AI 对话状态 —— 由 **WS-D (feat/ui-panes)** 实现。 */
export const useAiStore = defineStore('ai', () => {
  const messages = ref<AiMessage[]>([])
  const streaming = ref(false)
  const pendingConfirm = ref<FsOp | null>(null)
  const pendingCall = ref<AiToolCall | null>(null)
  const config = ref<AiConfig>({ baseUrl: '', apiKey: '', model: '', enabled: false })
  const error = ref<string | null>(null)

  let configLoaded = false

  async function loadConfig() {
    if (configLoaded) return
    try {
      config.value = await api.aiConfigGet()
    } catch (e) {
      error.value = String(e)
    }
    configLoaded = true
  }

  async function saveConfig(next: AiConfig) {
    config.value = { ...next }
    try {
      await api.aiConfigSet(next)
      useUiStore().statusText = 'AI 配置已保存'
    } catch (e) {
      error.value = String(e)
      useUiStore().statusText = `配置保存失败：${String(e)}`
    }
  }

  function describeOutcome(o: ToolOutcome): string {
    switch (o.kind) {
      case 'text':
        return o.payload
      case 'fs':
        return o.payload.ok ? '操作已完成。' : `操作失败：${o.payload.errors.join('; ')}`
      case 'search':
        return `找到 ${o.payload.length} 个结果。`
      case 'listing':
        return `目录 ${o.payload.path} 共 ${o.payload.entries.length} 项。`
    }
  }

  async function send(text: string) {
    const content = text.trim()
    if (!content || streaming.value) return
    messages.value.push({ role: 'user', content })
    error.value = null
    streaming.value = true
    pendingConfirm.value = null
    pendingCall.value = null

    const payload = messages.value.map((m) => ({ ...m }))
    messages.value.push({ role: 'assistant', content: '' })
    const assistant = messages.value[messages.value.length - 1]

    let unlisten: (() => void) | null = null
    try {
      try {
        unlisten = await events.onAiToken((tok) => {
          assistant.content += tok
        })
      } catch {
        // 事件通道不可用时退化为非流式
      }
      const pane = usePaneStore()
      const tab = pane.activeTab
      const reply = await api.aiChat(payload, {
        activePath: tab?.path ?? null,
        selected: tab?.selected ?? [],
      })
      if (!assistant.content) assistant.content = reply.text
      pendingConfirm.value = reply.pendingConfirm ?? null
      pendingCall.value = reply.pendingConfirm ? (reply.toolCalls[0] ?? null) : null
    } catch (e) {
      error.value = String(e)
      if (!assistant.content) assistant.content = `出错了：${String(e)}`
    } finally {
      if (unlisten) unlisten()
      streaming.value = false
    }
  }

  async function confirmPending(approved: boolean) {
    const op = pendingConfirm.value
    const call = pendingCall.value
    pendingConfirm.value = null
    pendingCall.value = null
    if (!op && !call) return

    const pane = usePaneStore()
    if (!approved) {
      if (call) {
        try {
          await api.aiExecuteTool(call, false)
        } catch {
          // 取消路径失败可忽略
        }
      }
      messages.value.push({ role: 'assistant', content: '已取消该操作。' })
      return
    }

    try {
      if (call) {
        const outcome = await api.aiExecuteTool(call, true)
        messages.value.push({ role: 'assistant', content: describeOutcome(outcome) })
      } else if (op) {
        const res = await api.fsOperation(op)
        messages.value.push({
          role: 'assistant',
          content: res.ok ? '操作已完成。' : `操作失败：${res.errors.join('; ')}`,
        })
      }
      await pane.refresh(pane.activePaneId)
    } catch (e) {
      messages.value.push({ role: 'assistant', content: `操作失败：${String(e)}` })
    }
  }

  function clear() {
    messages.value = []
    pendingConfirm.value = null
    pendingCall.value = null
    error.value = null
  }

  return {
    messages,
    streaming,
    pendingConfirm,
    pendingCall,
    config,
    error,
    loadConfig,
    saveConfig,
    send,
    confirmPending,
    clear,
  }
})

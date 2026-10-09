import { defineStore } from 'pinia'
import { ref } from 'vue'
import { api } from '../api'
import type { AiConfig, AiMessage, FsOp } from '../types'
import { usePaneStore } from './pane'

/** AI 对话状态 —— 由 **WS-D (feat/ui-panes)** 实现。 */
export const useAiStore = defineStore('ai', () => {
  const messages = ref<AiMessage[]>([])
  const streaming = ref(false)
  const pendingConfirm = ref<FsOp | null>(null)
  const config = ref<AiConfig>({ baseUrl: '', apiKey: '', model: '', enabled: false })
  const error = ref<string | null>(null)

  async function loadConfig() {
    try {
      config.value = await api.aiConfigGet()
    } catch (e) {
      error.value = String(e)
    }
  }

  async function saveConfig(next: AiConfig) {
    config.value = next
    await api.aiConfigSet(next)
  }

  async function send(text: string) {
    if (!text.trim()) return
    messages.value.push({ role: 'user', content: text })
    streaming.value = true
    error.value = null
    try {
      const pane = usePaneStore()
      const tab = pane.activeTab
      const reply = await api.aiChat(messages.value, {
        activePath: tab?.path ?? null,
        selected: tab?.selected ?? [],
      })
      messages.value.push({ role: 'assistant', content: reply.text })
      pendingConfirm.value = reply.pendingConfirm ?? null
    } catch (e) {
      error.value = String(e)
      messages.value.push({ role: 'assistant', content: `出错了：${String(e)}` })
    } finally {
      streaming.value = false
    }
  }

  async function confirmPending(approved: boolean) {
    const op = pendingConfirm.value
    pendingConfirm.value = null
    if (!op) return
    if (!approved) {
      messages.value.push({ role: 'assistant', content: '已取消该操作。' })
      return
    }
    const result = await api.fsOperation(op)
    messages.value.push({
      role: 'assistant',
      content: result.ok ? '操作已完成。' : `操作失败：${result.errors.join('; ')}`,
    })
  }

  return { messages, streaming, pendingConfirm, config, error, loadConfig, saveConfig, send, confirmPending }
})

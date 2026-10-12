import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { api } from '../api'
import type { AiChatMessage, AiConfig, AiConversation, AiMessage, AiToolCall, FsOp, ToolOutcome } from '../types'
import { usePaneStore } from './pane'
import { useUiStore } from './ui'

const ACTIVE_KEY = 'stelix.ai.activeConversation'
const CONTEXT_KEY = 'stelix.ai.context'

type ContextToggles = { activeDir: boolean; selection: boolean }

function readToggles(): ContextToggles {
  try {
    const raw = localStorage.getItem(CONTEXT_KEY)
    if (raw) {
      const parsed = JSON.parse(raw) as Partial<ContextToggles>
      return { activeDir: parsed.activeDir !== false, selection: parsed.selection !== false }
    }
  } catch { /* 读取失败用默认 */ }
  return { activeDir: true, selection: true }
}

/** 对话按会话独立保存；请求始终回写到发起会话，不串入当前查看的历史。 */
export const useAiStore = defineStore('ai', () => {
  const conversations = ref<AiConversation[]>([])
  const activeId = ref('')
  const drafts = ref<Record<string, string>>({})
  const responding = ref(false)
  const executing = ref(false)
  const busyConversationId = ref('')
  const pendingConfirm = ref<FsOp | null>(null)
  const pendingCall = ref<AiToolCall | null>(null)
  const config = ref<AiConfig>({ providers: [], activeProviderId: '', activeModelId: '', enabled: false })
  const error = ref<string | null>(null)
  const historyError = ref<string | null>(null)
  const historyLoading = ref(false)
  const historyMutating = ref(false)
  /** 当前正在流式生成的请求 id，用于按 id 路由增量事件与取消。 */
  const activeRequestId = ref('')
  /** 当前工具执行步骤提示（仅在回复进行中有效）。 */
  const pendingStep = ref('')
  /** 已发出停止请求、等待后端收尾。 */
  const stopping = ref(false)
  const contextToggles = ref<ContextToggles>(readToggles())
  const manualPaths = ref<string[]>([])
  const current = computed(() => conversations.value.find(c => c.id === activeId.value) ?? null)
  const messages = computed(() => current.value?.messages ?? [])
  const history = computed(() => conversations.value.filter(c => c.messages.length).slice().sort((a, b) => b.updatedAt - a.updatedAt))
  const busy = computed(() => responding.value || executing.value)
  const modelLabel = computed(() => {
    const provider = config.value.providers.find(p => p.id === config.value.activeProviderId)
    const model = provider?.models.find(m => m.id === config.value.activeModelId)
    return config.value.enabled && provider && model ? `${provider.name} · ${model.name || model.id}` : '离线规则模式'
  })

  let configLoaded = false
  let historyLoaded = false
  let historyPromise: Promise<void> | null = null
  let saveQueue: Promise<unknown> = Promise.resolve()
  let pendingMessageId = ''

  async function loadConfig() {
    if (configLoaded) return
    try {
      config.value = await api.aiConfigGet()
      error.value = null
      configLoaded = true
    } catch (e) { error.value = String(e) }
  }

  async function saveConfig(next: AiConfig) {
    try {
      await api.aiConfigSet(next)
      config.value = JSON.parse(JSON.stringify(next)) as AiConfig
      error.value = null
      configLoaded = true
      useUiStore().statusText = 'AI 配置已保存'
    } catch (e) {
      error.value = String(e)
      useUiStore().statusText = `配置保存失败：${String(e)}`
      throw e
    }
  }

  function rememberActive() {
    try { localStorage.setItem(ACTIVE_KEY, activeId.value) } catch { /* 选择状态不影响本地历史保存 */ }
  }

  async function persist(conversation: AiConversation): Promise<boolean> {
    const snapshot = JSON.parse(JSON.stringify(conversation)) as AiConversation
    const write = saveQueue.then(() => api.aiConversationSave(snapshot))
    saveQueue = write.catch(() => {})
    try {
      await write
      return true
    } catch (e) {
      historyError.value = `历史保存失败，当前消息仍保留在内存中：${String(e)}`
      return false
    }
  }

  function expirePending() {
    const message = messages.value.find(m => m.id === pendingMessageId)
    if (message?.operation === 'pending') {
      message.operation = 'expired'
      if (current.value) void persist(current.value)
    }
    pendingConfirm.value = null
    pendingCall.value = null
    pendingMessageId = ''
  }

  function newConversation() {
    if (executing.value || historyMutating.value || !historyLoaded) return
    expirePending()
    if (current.value && !current.value.messages.length) return
    const now = Date.now()
    const conversation: AiConversation = { id: crypto.randomUUID(), title: '新对话', createdAt: now, updatedAt: now, messages: [] }
    conversations.value.push(conversation)
    activeId.value = conversation.id
    rememberActive()
  }

  function selectConversation(id: string) {
    if (executing.value || historyMutating.value || !conversations.value.some(c => c.id === id) || id === activeId.value) return
    expirePending()
    activeId.value = id
    rememberActive()
  }

  async function loadHistory() {
    if (historyLoaded) return
    if (historyPromise) return historyPromise
    historyLoading.value = true
    historyPromise = (async () => {
      try {
        conversations.value = await api.aiConversationsList()
        // 重新打开的历史只能继续对话，不能恢复待执行的操作。
        for (const conversation of conversations.value) {
          for (const message of conversation.messages) {
            if (message.status === 'pending') {
              message.status = 'interrupted'
              if (!message.content) message.content = '上次回复未完成，可以重试。'
            }
            if (message.operation === 'pending') message.operation = 'expired'
          }
        }
        let savedId = ''
        try { savedId = localStorage.getItem(ACTIVE_KEY) ?? '' } catch { /* 使用最近对话 */ }
        activeId.value = conversations.value.find(c => c.id === savedId)?.id ?? history.value[0]?.id ?? ''
        historyLoaded = true
        historyError.value = null
        if (!activeId.value) newConversation()
      } catch (e) { historyError.value = `读取历史对话失败：${String(e)}` }
      finally { historyLoading.value = false; historyPromise = null }
    })()
    return historyPromise
  }

  async function retryHistory() {
    if (!historyLoaded) { await loadHistory(); return }
    historyError.value = null
    for (const conversation of conversations.value.filter(c => c.messages.length)) {
      if (!await persist(conversation)) break
    }
  }

  async function renameConversation(id: string, title: string): Promise<boolean> {
    const conversation = conversations.value.find(c => c.id === id)
    const name = title.trim()
    if (!conversation || !name || name.length > 120 || historyMutating.value) return false
    historyMutating.value = true
    const previous = conversation.title
    conversation.title = name
    const saved = await persist(conversation)
    if (!saved) conversation.title = previous
    historyMutating.value = false
    return saved
  }

  async function deleteConversation(id: string): Promise<boolean> {
    if (id === busyConversationId.value || executing.value || historyMutating.value) return false
    historyMutating.value = true
    try {
      if (activeId.value === id) expirePending()
      // 排在所有已提交的写入之后，避免删除完成后旧保存请求重新建回历史。
      const deletion = saveQueue.then(() => api.aiConversationDelete(id))
      saveQueue = deletion.catch(() => {})
      await deletion
      conversations.value = conversations.value.filter(c => c.id !== id)
      delete drafts.value[id]
      if (activeId.value === id) {
        activeId.value = history.value[0]?.id ?? ''
        if (!activeId.value) {
          historyMutating.value = false
          newConversation()
        }
        rememberActive()
      }
      return true
    } catch (e) { historyError.value = `删除历史对话失败：${String(e)}`; return false }
    finally { historyMutating.value = false }
  }

  function makeMessage(role: 'user' | 'assistant', content: string, kind: AiChatMessage['kind'] = 'message'): AiChatMessage {
    return { id: crypto.randomUUID(), role, content, createdAt: Date.now(), status: 'complete', model: role === 'assistant' ? modelLabel.value : '', kind }
  }

  /** 按当前窗格与上下文开关构造发送用的上下文（手动路径并入 selected）。 */
  function buildContext() {
    const pane = usePaneStore()
    const tab = pane.activeTab
    const selected = contextToggles.value.selection ? tab?.selected ?? [] : []
    const merged = [...selected, ...manualPaths.value]
    // 远程（FTP / SFTP）标签的路径不是本地路径，不能作为 AI 的本地操作上下文。
    const localTab = tab && tab.kind !== 'remote' ? tab : undefined
    return {
      activePath: contextToggles.value.activeDir ? localTab?.path ?? null : null,
      selected: Array.from(new Set(merged)),
    }
  }

  /** 找到当前等待中的 assistant 消息（仅当它属于活动会话）。 */
  function streamingMessage(): AiChatMessage | undefined {
    const conversation = conversations.value.find(c => c.id === busyConversationId.value)
    const last = conversation?.messages[conversation.messages.length - 1]
    return last?.role === 'assistant' ? last : undefined
  }

  /** 收到流式增量：只回写到发起请求的那条消息。 */
  function applyToken(requestId: string, delta: string) {
    if (!requestId || requestId !== activeRequestId.value) return
    const message = streamingMessage()
    if (message) message.content += delta
  }

  /** 收到工具步骤提示。 */
  function applyStep(requestId: string, step: string) {
    if (!requestId || requestId !== activeRequestId.value) return
    pendingStep.value = step
  }

  /** 请求停止当前生成。 */
  async function stop() {
    const id = activeRequestId.value
    if (!id || stopping.value) return
    stopping.value = true
    try { await api.aiChatCancel(id) }
    catch (e) { useUiStore().setStatusText(`停止生成失败：${String(e)}`) }
  }

  async function requestReply(conversation: AiConversation, assistant: AiChatMessage) {
    busyConversationId.value = conversation.id
    assistant.status = 'pending'
    assistant.content = ''
    assistant.operation = null
    assistant.model = modelLabel.value
    assistant.createdAt = Date.now()
    assistant.usage = null
    assistant.durationMs = null
    conversation.updatedAt = Date.now()
    const payload: AiMessage[] = conversation.messages.filter(m => m.id !== assistant.id && m.status === 'complete').map(m => ({
      role: m.role,
      content: m.operation === 'expired' ? `${m.content}\n（此操作未执行，原确认已失效。）` : m.content,
    }))
    const context = buildContext()
    const requestId = crypto.randomUUID()
    activeRequestId.value = requestId
    pendingStep.value = ''
    stopping.value = false
    const startedAt = Date.now()
    await persist(conversation)
    try {
      const reply = await api.aiChat(payload, context, requestId)
      assistant.content = reply.text || '模型没有返回文本，请检查模型配置后重试。'
      assistant.status = reply.interrupted ? 'interrupted' : (reply.text ? 'complete' : 'error')
      assistant.usage = reply.usage ?? null
      assistant.durationMs = Date.now() - startedAt
      if (reply.pendingConfirm) {
        if (activeId.value === conversation.id) {
          pendingConfirm.value = reply.pendingConfirm
          pendingCall.value = reply.toolCalls[0] ?? null
          pendingMessageId = assistant.id
          assistant.operation = 'pending'
        } else { assistant.operation = 'expired' }
      }
    } catch (e) {
      assistant.status = 'error'
      assistant.content = `请求失败：${String(e)}`
    } finally {
      conversation.updatedAt = Date.now()
      responding.value = false
      stopping.value = false
      activeRequestId.value = ''
      pendingStep.value = ''
      busyConversationId.value = ''
      await persist(conversation)
    }
  }

  async function send(text: string): Promise<boolean> {
    const content = text.trim()
    if (!content || busy.value || historyMutating.value) return false
    responding.value = true
    await Promise.all([loadHistory(), loadConfig()])
    if (!historyLoaded || !current.value) { responding.value = false; return false }
    expirePending()
    const conversation = current.value
    if (!conversation.messages.length) conversation.title = content.split('\n')[0]!.slice(0, 40)
    conversation.messages.push(makeMessage('user', content))
    conversation.messages.push(makeMessage('assistant', ''))
    const assistant = conversation.messages[conversation.messages.length - 1]!
    await requestReply(conversation, assistant)
    return true
  }

  async function retry(messageId: string) {
    const conversation = current.value
    const assistant = conversation ? conversation.messages[conversation.messages.length - 1] : undefined
    if (busy.value || historyMutating.value || !conversation || !assistant || assistant.id !== messageId || assistant.kind !== 'message' || !['error', 'interrupted'].includes(assistant.status)) return
    responding.value = true
    expirePending()
    await requestReply(conversation, assistant)
  }

  function describeOutcome(outcome: ToolOutcome): string {
    switch (outcome.kind) {
      case 'text': return outcome.payload
      case 'fs': return outcome.payload.ok ? '操作已完成。' : `操作失败：${outcome.payload.errors.join('; ')}`
      case 'search': return `找到 ${outcome.payload.length} 个结果。`
      case 'listing': return `目录 ${outcome.payload.path} 共 ${outcome.payload.entries.length} 项。`
    }
  }

  async function confirmPending(approved: boolean) {
    const conversation = current.value
    const message = messages.value.find(m => m.id === pendingMessageId)
    if (busy.value || !conversation || !message || !pendingConfirm.value) return
    const op = pendingConfirm.value
    const call = pendingCall.value
    pendingConfirm.value = null
    pendingCall.value = null
    pendingMessageId = ''
    executing.value = true
    busyConversationId.value = conversation.id
    try {
      let text = '已取消该操作，文件未作更改。'
      let failed = false
      if (approved) {
        if (call) {
          const outcome = await api.aiExecuteTool(call, true)
          text = describeOutcome(outcome)
          failed = outcome.kind === 'fs' && !outcome.payload.ok
        } else {
          const result = await api.fsOperation(op)
          text = result.ok ? '操作已完成。' : `操作失败：${result.errors.join('; ')}`
          failed = !result.ok
        }
      }
      message.operation = approved ? (failed ? 'error' : 'completed') : 'cancelled'
      const resultMessage = makeMessage('assistant', text, 'operation')
      if (failed) resultMessage.status = 'error'
      conversation.messages.push(resultMessage)
      if (approved) {
        const pane = usePaneStore()
        try { await pane.refresh(pane.activePaneId) }
        catch (e) { useUiStore().setStatusText(`文件操作已结束，刷新窗格失败：${String(e)}`) }
      }
    } catch (e) {
      message.operation = 'error'
      const result = makeMessage('assistant', `操作失败：${String(e)}`, 'operation')
      result.status = 'error'
      conversation.messages.push(result)
    } finally {
      conversation.updatedAt = Date.now()
      executing.value = false
      busyConversationId.value = ''
      await persist(conversation)
    }
  }

  function toggleContext(key: keyof ContextToggles) {
    contextToggles.value = { ...contextToggles.value, [key]: !contextToggles.value[key] }
    try { localStorage.setItem(CONTEXT_KEY, JSON.stringify(contextToggles.value)) } catch { /* 忽略 */ }
  }

  function addContextPath(path: string) {
    const value = path.trim()
    if (!value || manualPaths.value.includes(value)) return
    manualPaths.value = [...manualPaths.value, value]
  }

  function removeContextPath(path: string) {
    manualPaths.value = manualPaths.value.filter(p => p !== path)
  }

  return { conversations, activeId, drafts, current, messages, history, responding, executing, busy, busyConversationId, pendingConfirm, pendingCall, config, modelLabel, error, historyError, historyLoading, historyMutating, activeRequestId, pendingStep, stopping, contextToggles, manualPaths, loadConfig, saveConfig, loadHistory, retryHistory, newConversation, selectConversation, renameConversation, deleteConversation, send, retry, confirmPending, applyToken, applyStep, stop, toggleContext, addContextPath, removeContextPath, buildContext }
})

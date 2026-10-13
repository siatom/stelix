<script setup lang="ts">
import { computed, nextTick, onMounted, ref, watch } from 'vue'
import { useAiStore } from '../stores/ai'
import { useUiStore } from '../stores/ui'
import { usePaneStore } from '../stores/pane'
import ChatMessage from './ai/ChatMessage.vue'
import ChatHistory from './ai/ChatHistory.vue'
import Icon from './icons/Icon.vue'

const ai = useAiStore()
const ui = useUiStore()
const pane = usePaneStore()
const showHistory = ref(false)
const addingPath = ref(false)
const pathDraft = ref('')
const draft = computed({ get: () => ai.drafts[ai.activeId] ?? '', set: value => { if (ai.activeId) ai.drafts[ai.activeId] = value } })
const bodyRef = ref<HTMLElement | null>(null)
const inputRef = ref<HTMLTextAreaElement | null>(null)
const atBottom = ref(true)
const hasNewMessage = ref(false)
const streamingActive = computed(() => ai.responding && ai.busyConversationId === ai.activeId)
const activeDir = computed(() => pane.activeTab?.path ?? '')
const selectionCount = computed(() => pane.activeTab?.selected.length ?? 0)
const examples = ['找出这个目录下最大的 5 个文件', '把所有 png 图片列出来', '按扩展名统计当前目录']
const opLabels: Record<string, string> = { copy: '复制', move: '移动', delete: '删除', rename: '重命名', mkdir: '新建文件夹' }
const confirmSummary = computed(() => {
  const op = ai.pendingConfirm
  return op ? { op: opLabels[op.op] ?? op.op, sources: op.sources ?? [], target: op.target ?? '' } : null
})

onMounted(() => { void ai.loadConfig(); void ai.loadHistory() })

function resizeInput() {
  const input = inputRef.value
  if (!input) return
  input.style.height = 'auto'
  input.style.height = `${Math.min(input.scrollHeight, 140)}px`
}
function onScroll() {
  const body = bodyRef.value
  if (!body) return
  atBottom.value = body.scrollHeight - body.scrollTop - body.clientHeight < 60
  if (atBottom.value) hasNewMessage.value = false
}
function scrollToLatest() {
  const body = bodyRef.value
  if (body) body.scrollTop = body.scrollHeight
  atBottom.value = true
  hasNewMessage.value = false
}
watch(() => [ai.activeId, showHistory.value], async () => {
  atBottom.value = true
  hasNewMessage.value = false
  await nextTick()
  scrollToLatest()
  resizeInput()
})
watch(() => ai.messages.map(m => `${m.id}:${m.status}:${m.content.length}:${m.operation}`).join('|'), async () => {
  const shouldFollow = atBottom.value
  await nextTick()
  if (shouldFollow) scrollToLatest()
  else hasNewMessage.value = true
})
watch(draft, async () => { await nextTick(); resizeInput() })

async function send() {
  const text = draft.value
  const id = ai.activeId
  if (!text.trim() || ai.busy || ai.historyLoading || ai.historyMutating) return
  draft.value = ''
  atBottom.value = true
  if (!await ai.send(text) && !ai.drafts[id]) ai.drafts[id] = text
}
function onInputKeydown(event: KeyboardEvent) {
  if (event.key === 'Enter' && !event.shiftKey && !event.ctrlKey && !event.altKey && !event.metaKey && !event.isComposing) {
    event.preventDefault()
    void send()
  }
}
function newChat() {
  ai.newConversation()
  showHistory.value = false
  void nextTick(() => inputRef.value?.focus())
}
function dayLabel(time: number) {
  const date = new Date(time)
  const now = new Date()
  const yesterday = new Date(now.getFullYear(), now.getMonth(), now.getDate() - 1)
  if (date.toDateString() === now.toDateString()) return '今天'
  if (date.toDateString() === yesterday.toDateString()) return '昨天'
  return date.toLocaleDateString('zh-CN', { year: 'numeric', month: 'numeric', day: 'numeric' })
}
function startsDay(index: number) {
  return index === 0 || new Date(ai.messages[index]!.createdAt).toDateString() !== new Date(ai.messages[index - 1]!.createdAt).toDateString()
}
function baseName(path: string) {
  const trimmed = path.replace(/[\\/]+$/, '')
  const index = Math.max(trimmed.lastIndexOf('\\'), trimmed.lastIndexOf('/'))
  return index >= 0 ? trimmed.slice(index + 1) || trimmed : trimmed
}
function submitPath() {
  if (!pathDraft.value.trim()) return
  ai.addContextPath(pathDraft.value)
  pathDraft.value = ''
  addingPath.value = false
}
</script>

<template>
  <aside class="ai-panel" aria-label="AI 助手">
    <header class="ai-head">
      <span class="title"><Icon name="sparkles" :size="15" />AI 助手</span>
      <div class="head-actions">
        <button class="head-btn" title="新建对话" aria-label="新建对话" :disabled="ai.executing || ai.historyLoading || ai.historyMutating" @click="newChat"><Icon name="plus" :size="15" /></button>
        <button class="head-btn" :class="{ selected: showHistory }" title="历史对话" aria-label="历史对话" :aria-pressed="showHistory" @click="showHistory = !showHistory"><Icon name="clock" :size="14" /></button>
        <button class="head-btn" title="AI 大模型设置" aria-label="AI 大模型设置" @click="ui.openSettings('ai')"><Icon name="settings" :size="14" /></button>
        <button class="head-btn" title="收起侧边栏" aria-label="收起侧边栏" @click="ui.toggleSidebar()"><Icon name="panel-left" :size="14" /></button>
      </div>
    </header>
    <div class="conversation-bar"><span :title="showHistory ? '历史对话' : ai.current?.title">{{ showHistory ? '历史对话' : ai.current?.title ?? '新对话' }}</span><button v-if="showHistory" title="返回当前对话" aria-label="返回当前对话" @click="showHistory = false"><Icon name="chevron-left" :size="13" />返回</button></div>
    <div v-if="ai.historyError" class="notice error-notice" role="alert"><span>{{ ai.historyError }}</span><button :disabled="ai.historyLoading || ai.historyMutating" @click="ai.retryHistory">重试</button></div>
    <div v-if="ai.error" class="notice error-notice" role="alert"><span>模型配置读取失败：{{ ai.error }}</span><button @click="ui.openSettings('ai')">设置</button></div>

    <ChatHistory v-if="showHistory" @open="showHistory = false" />
    <template v-else>
      <button class="model-status" :title="ai.modelLabel" @click="ui.openSettings('ai')"><span class="model-dot" :class="{ enabled: ai.config.enabled }" /><span>{{ ai.modelLabel }}</span><Icon name="chevron-down" :size="11" /></button>
      <div class="chat-flow">
        <div ref="bodyRef" class="ai-body" :aria-busy="ai.responding && ai.busyConversationId === ai.activeId" @scroll="onScroll">
          <div v-if="ai.historyLoading" class="loading" role="status">正在读取对话…</div>
          <div v-else-if="!ai.messages.length" class="empty">
            <div class="welcome-icon"><Icon name="sparkles" :size="22" /></div>
            <h2>今天想整理什么？</h2><p>查找文件、分析目录，或描述你的整理需求。</p>
            <button v-for="example in examples" :key="example" class="example" @click="draft = example; inputRef?.focus()">{{ example }}<Icon name="arrow-up" :size="12" /></button>
          </div>
          <template v-for="(message, index) in ai.messages" :key="message.id">
            <div v-if="startsDay(index)" class="day-divider">{{ dayLabel(message.createdAt) }}</div>
            <ChatMessage :message="message" :busy="ai.busy" :step="streamingActive ? ai.pendingStep : ''" :can-retry="index === ai.messages.length - 1 && message.kind === 'message' && (message.status === 'error' || message.status === 'interrupted')" @retry="ai.retry" />
          </template>
          <div v-if="confirmSummary" class="confirm">
            <p class="confirm-title"><Icon name="alert-triangle" :size="13" />确认文件操作</p>
            <dl><dt>操作</dt><dd>{{ confirmSummary.op }}</dd><template v-if="confirmSummary.sources.length"><dt>文件</dt><dd>{{ confirmSummary.sources.slice(0, 4).join('、') }}<span v-if="confirmSummary.sources.length > 4">…共 {{ confirmSummary.sources.length }} 项</span></dd></template><template v-if="confirmSummary.target"><dt>目标</dt><dd>{{ confirmSummary.target }}</dd></template></dl>
            <div class="confirm-actions"><button :disabled="ai.busy" class="allow" @click="ai.confirmPending(true)">确认执行</button><button :disabled="ai.busy" @click="ai.confirmPending(false)">取消</button></div>
          </div>
          <div v-if="ai.executing" class="loading" role="status">正在执行文件操作…</div>
        </div>
        <button v-if="!atBottom && ai.messages.length" class="jump-latest" @click="scrollToLatest"><Icon name="arrow-down" :size="12" />{{ hasNewMessage ? '查看新回复' : '回到最新' }}</button>
      </div>
      <div v-if="ai.responding && ai.busyConversationId !== ai.activeId" class="background-reply" role="status">另一条对话正在回复<button @click="ai.selectConversation(ai.busyConversationId)">查看</button></div>
      <div class="context-area">
        <div class="context-bar" aria-label="对话上下文">
          <button class="ctx-chip" :class="{ on: ai.contextToggles.activeDir && activeDir }" :disabled="!activeDir" :title="activeDir || '当前没有打开的目录'" @click="ai.toggleContext('activeDir')"><Icon name="folder" :size="12" /><span>当前目录{{ activeDir ? `：${baseName(activeDir)}` : '' }}</span></button>
          <button class="ctx-chip" :class="{ on: ai.contextToggles.selection && selectionCount > 0 }" :disabled="!selectionCount" :title="`已选中 ${selectionCount} 项`" @click="ai.toggleContext('selection')"><Icon name="check-circle" :size="12" /><span>选中 {{ selectionCount }} 项</span></button>
          <span v-for="path in ai.manualPaths" :key="path" class="ctx-chip on manual" :title="path"><Icon name="file" :size="12" /><span>{{ baseName(path) }}</span><button class="ctx-remove" type="button" title="移除该上下文" aria-label="移除该上下文" @click="ai.removeContextPath(path)"><Icon name="x" :size="10" /></button></span>
          <button class="ctx-chip add" type="button" title="添加路径作为上下文" aria-label="添加路径作为上下文" :aria-expanded="addingPath" @click="addingPath = !addingPath"><Icon name="plus" :size="12" /></button>
        </div>
        <div v-if="addingPath" class="ctx-add">
          <input v-model="pathDraft" aria-label="上下文路径" placeholder="文件或文件夹的绝对路径" spellcheck="false" @keydown.enter.prevent="submitPath" @keydown.esc="addingPath = false" />
          <button type="button" :disabled="!pathDraft.trim()" @click="submitPath">添加</button>
        </div>
      </div>
      <form class="ai-input" @submit.prevent="send">
        <div class="composer"><textarea ref="inputRef" v-model="draft" rows="2" aria-label="消息内容" placeholder="描述你的需求…" spellcheck="false" @input="resizeInput" @keydown="onInputKeydown" /><div class="composer-bottom"><span>{{ streamingActive ? (ai.stopping ? '正在停止…' : ai.pendingStep || '正在思考…') : 'Enter 发送 · Shift+Enter 换行' }}</span><button v-if="streamingActive" class="send stop" type="button" title="停止生成" aria-label="停止生成" :disabled="ai.stopping" @click="ai.stop()"><Icon name="stop" :size="13" /></button><button v-else class="send" type="submit" title="发送消息" aria-label="发送消息" :disabled="ai.busy || ai.historyLoading || ai.historyMutating || !draft.trim()"><Icon name="arrow-up" :size="15" /></button></div></div>
        <p class="input-hint">文件写操作会先请你确认</p>
      </form>
    </template>
  </aside>
</template>

<style scoped>
.ai-panel { width: 100%; height: 100%; display: flex; flex-direction: column; overflow: hidden; background: var(--bg-panel); }
.ai-head { flex: none; min-height: var(--toolbar-h); display: flex; align-items: center; justify-content: space-between; gap: 8px; padding: 0 10px; }
.title { display: flex; align-items: center; gap: 6px; font-size: 1em; font-weight: 600; white-space: nowrap; }
.head-actions { display: flex; gap: 2px; }
.head-btn { display: flex; align-items: center; justify-content: center; width: 25px; height: 26px; border-radius: 6px; color: var(--fg-muted); }
.head-btn:hover:not(:disabled), .head-btn.selected { background: var(--bg-hover); color: var(--fg); }
.conversation-bar { flex: none; display: flex; align-items: center; justify-content: space-between; gap: 8px; min-height: 31px; padding: 0 12px 7px; border-bottom: 1px solid var(--border); }
.conversation-bar > span { min-width: 0; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; font-size: 0.9167em; color: var(--fg-muted); }
.conversation-bar button { flex: none; display: flex; align-items: center; gap: 2px; color: var(--fg-dim); font-size: 0.8333em; }
.model-status { flex: none; display: flex; align-items: center; gap: 6px; padding: 8px 12px; color: var(--fg-dim); text-align: left; font-size: 0.8333em; }
.model-status > span:nth-child(2) { flex: 1; min-width: 0; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.model-status:hover { background: var(--bg-hover); color: var(--fg); }
.model-dot { flex: none; width: 5px; height: 5px; border-radius: 50%; background: var(--fg-dim); }
.model-dot.enabled { background: var(--ok); }
.chat-flow { position: relative; flex: 1; min-height: 0; }
.ai-body { height: 100%; overflow: auto; padding: 0 12px 12px; overflow-anchor: none; }
.empty { display: flex; flex-direction: column; gap: 10px; padding: 26px 0 12px; }
.welcome-icon { display: flex; align-items: center; justify-content: center; width: 40px; height: 40px; border-radius: var(--radius); background: var(--bg-hover); }
.empty h2 { margin: 8px 0 0; font-size: 1.3333em; font-weight: 600; }
.empty p { margin: 0 0 12px; color: var(--fg-dim); font-size: 0.9167em; line-height: 1.6; }
.example { display: flex; align-items: center; justify-content: space-between; gap: 8px; padding: 10px; border: 1px solid var(--border); border-radius: var(--radius-sm); color: var(--fg-muted); text-align: left; line-height: 1.5; font-size: 0.9167em; }
.example:hover { background: var(--bg-hover); color: var(--fg); }
.example :deep(svg) { flex: none; }
.day-divider { padding: 12px 0 4px; text-align: center; color: var(--fg-dim); font-size: 0.75em; }
.jump-latest { position: absolute; bottom: 10px; left: 50%; transform: translateX(-50%); display: flex; align-items: center; gap: 5px; padding: 7px 10px; border: 1px solid var(--border-strong); border-radius: 20px; background: var(--bg-panel); box-shadow: var(--shadow-sm); font-size: 0.8333em; white-space: nowrap; }
.notice { flex: none; display: flex; align-items: flex-start; gap: 6px; padding: 8px 10px; font-size: 0.8333em; line-height: 1.5; }
.notice span { flex: 1; min-width: 0; overflow-wrap: anywhere; }
.notice button { flex: none; text-decoration: underline; }
.error-notice { color: var(--danger); background: var(--danger-soft); }
.loading { padding: 16px 0; color: var(--fg-dim); font-size: 0.9167em; }
.confirm { margin-top: 12px; padding: 12px; border: 1px solid var(--border-strong); border-radius: var(--radius); background: var(--bg-hover); }
.confirm-title { display: flex; align-items: center; gap: 6px; margin: 0 0 10px; color: var(--warn); font-size: 0.9167em; font-weight: 600; }
.confirm dl { display: grid; grid-template-columns: 28px minmax(0, 1fr); gap: 6px; margin: 0; font-size: 0.8333em; line-height: 1.6; }
.confirm dt { color: var(--fg-dim); }
.confirm dd { margin: 0; overflow-wrap: anywhere; color: var(--fg-muted); }
.confirm-actions { display: flex; gap: 8px; margin-top: 12px; }
.confirm-actions button { flex: 1; min-height: 29px; border: 1px solid var(--border-strong); border-radius: var(--radius-sm); font-size: 0.875em; }
.confirm-actions .allow { background: var(--accent); border-color: var(--accent); color: var(--accent-fg); }
.background-reply { display: flex; justify-content: space-between; padding: 7px 12px; font-size: 0.8333em; color: var(--fg-dim); }
.background-reply button { text-decoration: underline; }
.context-area { flex: none; padding: 0 10px; }
.context-bar { display: flex; align-items: center; gap: 6px; overflow-x: auto; padding-bottom: 6px; scrollbar-width: none; }
.context-bar::-webkit-scrollbar { display: none; }
.ctx-chip { display: flex; align-items: center; gap: 4px; flex: none; max-width: 170px; padding: 3px 8px; border: 1px solid var(--border); border-radius: 999px; color: var(--fg-dim); font-size: 0.75em; }
.ctx-chip > span { min-width: 0; overflow: hidden; white-space: nowrap; text-overflow: ellipsis; }
.ctx-chip:hover:not(:disabled) { background: var(--bg-hover); color: var(--fg); }
.ctx-chip.on { border-color: var(--accent); background: var(--accent-soft); color: var(--fg); }
.ctx-chip:disabled { opacity: 0.4; }
.ctx-chip.add { padding: 3px 6px; }
.ctx-remove { display: flex; align-items: center; color: var(--fg-dim); }
.ctx-remove:hover { color: var(--danger); }
.ctx-add { display: flex; gap: 6px; padding-bottom: 6px; }
.ctx-add input { flex: 1; min-width: 0; min-height: 28px; padding: 4px 8px; font-size: 0.8333em; }
.ctx-add button { flex: none; padding: 3px 10px; border: 1px solid var(--border-strong); border-radius: var(--radius-sm); font-size: 0.8333em; }
.ctx-add button:disabled { opacity: 0.45; }
.ai-input { flex: none; padding: 8px 10px 6px; }
.composer { padding: 9px; border: 1px solid var(--border-strong); border-radius: 14px; background: var(--bg-input); }
.composer:focus-within { border-color: var(--accent); box-shadow: 0 0 0 2px var(--accent-soft); }
.composer textarea { display: block; width: 100%; min-width: 0; min-height: 48px; max-height: 140px; padding: 0; resize: none; border: 0; border-radius: 0; outline: none; font-size: 1em; line-height: 1.6; box-shadow: none; background: transparent; }
.composer-bottom { display: flex; align-items: center; justify-content: space-between; gap: 4px; padding-top: 4px; }
.composer-bottom > span { font-size: 0.7em; color: var(--fg-dim); }
.send { display: flex; align-items: center; justify-content: center; flex: none; width: 27px; height: 27px; border-radius: 50%; background: var(--accent); color: var(--accent-fg); }
.send:disabled { opacity: 0.3; }
.send.stop { background: var(--bg-elevated); color: var(--fg); border: 1px solid var(--border-strong); }
.send.stop:hover:not(:disabled) { background: var(--bg-hover); }
.input-hint { margin: 6px 0 0; text-align: center; font-size: 0.75em; color: var(--fg-dim); }
</style>

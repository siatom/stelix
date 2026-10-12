<script setup lang="ts">
import { computed, ref } from 'vue'
import { openUrl } from '@tauri-apps/plugin-opener'
import { useUiStore } from '../../stores/ui'
import type { AiChatMessage } from '../../types'
import Icon from '../icons/Icon.vue'
import { renderMarkdown } from './markdown'

const props = defineProps<{ message: AiChatMessage; canRetry: boolean; busy: boolean; step?: string }>()
const emit = defineEmits<{ retry: [id: string] }>()
const copied = ref(false)
const markdown = computed(() => props.message.status === 'pending' ? '' : renderMarkdown(props.message.content))
const time = computed(() => new Date(props.message.createdAt).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit', hour12: false }))
const operationText = computed(() => ({ pending: '等待确认', completed: '操作已结束', cancelled: '已取消', expired: '原确认已失效，文件未执行更改', error: '操作失败' })[props.message.operation ?? 'expired'])
const stepText = computed(() => props.step?.trim() || '正在思考…')
const stats = computed(() => {
  const parts: string[] = []
  const usage = props.message.usage
  if (usage && (usage.inputTokens || usage.outputTokens)) parts.push(`↑${usage.inputTokens} ↓${usage.outputTokens} tokens`)
  if (props.message.durationMs != null) parts.push(`${(props.message.durationMs / 1000).toFixed(1)}s`)
  return parts.join(' · ')
})

async function copy() {
  try {
    await navigator.clipboard.writeText(props.message.content)
    copied.value = true
  } catch (e) { useUiStore().setStatusText(`复制失败：${String(e)}`) }
}
async function onLink(event: MouseEvent) {
  const link = (event.target as HTMLElement).closest('a')
  if (!link) return
  event.preventDefault()
  const href = link.getAttribute('href')
  if (!href || !/^https?:\/\//i.test(href)) return
  try { await openUrl(href) }
  catch (e) { useUiStore().setStatusText(`打开链接失败：${String(e)}`) }
}
</script>

<template>
  <article class="chat-message" :class="[message.role, { failed: message.status === 'error' || message.status === 'interrupted', operation: message.kind === 'operation' }]">
    <div class="message-meta"><span class="speaker"><Icon v-if="message.role === 'assistant'" :name="message.kind === 'operation' ? 'check-circle' : 'sparkles'" :size="12" />{{ message.role === 'user' ? '你' : message.kind === 'operation' ? '执行结果' : 'AI 助手' }}</span><time :datetime="new Date(message.createdAt).toISOString()">{{ time }}</time></div>
    <div v-if="message.status === 'pending'" class="pending" role="status"><span class="thinking-dots"><i /><i /><i /></span>{{ stepText }}</div>
    <div v-else-if="message.role === 'user'" class="message-content user-content">{{ message.content }}</div>
    <div v-else class="message-content markdown" data-native-context-menu @click="onLink" v-html="markdown" />
    <div v-if="message.model && message.role === 'assistant' && message.kind !== 'operation'" class="message-model" :title="message.model">{{ message.model }}<template v-if="stats"> · {{ stats }}</template></div>
    <div v-if="message.operation" class="operation-status" :class="message.operation">{{ operationText }}</div>
    <div v-if="message.status !== 'pending'" class="message-actions">
      <button :title="copied ? '已复制' : '复制消息'" :aria-label="copied ? '已复制消息' : '复制消息'" @click="copy"><Icon :name="copied ? 'check' : 'copy'" :size="12" />{{ copied ? '已复制' : '复制' }}</button>
      <button v-if="canRetry" :disabled="busy" @click="emit('retry', message.id)"><Icon name="refresh" :size="12" />重试</button>
      <span v-if="message.status === 'interrupted'" class="interrupted">回复已中断</span>
    </div>
  </article>
</template>

<style scoped>
.chat-message { flex: none; min-width: 0; padding: 12px 0; }
.chat-message + .chat-message { border-top: 1px solid var(--border); }
.message-meta { display: flex; align-items: center; justify-content: space-between; gap: 8px; margin-bottom: 8px; font-size: 0.8333rem; color: var(--fg-dim); }
.speaker { display: flex; align-items: center; gap: 5px; font-weight: 600; color: var(--fg-muted); }
.message-content { font-size: 1.0417rem; line-height: 1.7; overflow-wrap: anywhere; user-select: text; }
.user-content { padding: 9px 11px; border-radius: var(--radius); background: var(--bg-hover); white-space: pre-wrap; }
.message-model { margin-top: 7px; font-size: 0.75rem; color: var(--fg-dim); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.message-actions { display: flex; align-items: center; flex-wrap: wrap; gap: 8px; margin-top: 7px; }
.message-actions button { display: flex; align-items: center; gap: 4px; padding: 3px 5px; border-radius: 5px; font-size: 0.8333rem; color: var(--fg-dim); }
.message-actions button:hover { background: var(--bg-hover); color: var(--fg); }
.failed .message-content { color: var(--danger); }
.interrupted { font-size: 0.8333rem; color: var(--fg-dim); }
.operation .message-content { padding: 8px 10px; border: 1px solid var(--border); border-radius: var(--radius-sm); background: var(--bg-hover); }
.operation-status { margin-top: 7px; font-size: 0.8333rem; color: var(--fg-dim); }
.operation-status.pending { color: var(--warn); }
.operation-status.completed { color: var(--ok); }
.operation-status.error { color: var(--danger); }
.pending { display: flex; align-items: center; gap: 8px; min-height: 32px; font-size: 0.9167rem; color: var(--fg-muted); }
.thinking-dots { display: flex; gap: 3px; }
.thinking-dots i { width: 4px; height: 4px; border-radius: 50%; background: currentColor; animation: pulse 1.2s ease-in-out infinite; }
.thinking-dots i:nth-child(2) { animation-delay: 0.15s; }
.thinking-dots i:nth-child(3) { animation-delay: 0.3s; }
@keyframes pulse { 50% { opacity: 0.35; transform: translateY(-2px); } }
@media (prefers-reduced-motion: reduce) { .thinking-dots i { animation: none; } }
.markdown :deep(> :first-child) { margin-top: 0; }
.markdown :deep(> :last-child) { margin-bottom: 0; }
.markdown :deep(p) { margin: 0.6em 0; }
.markdown :deep(h1), .markdown :deep(h2), .markdown :deep(h3), .markdown :deep(h4) { margin: 1em 0 0.5em; font-size: 1.0833rem; line-height: 1.5; }
.markdown :deep(ul), .markdown :deep(ol) { margin: 0.6em 0; padding-left: 1.5em; }
.markdown :deep(li) { margin: 0.3em 0; }
.markdown :deep(code) { padding: 1px 4px; border-radius: 4px; background: var(--bg-hover); font-family: var(--mono); font-size: 0.9167rem; }
.markdown :deep(pre) { max-width: 100%; overflow-x: auto; margin: 10px 0; padding: 10px; border: 1px solid var(--border); border-radius: var(--radius-sm); background: var(--bg-hover); white-space: pre; }
.markdown :deep(pre code) { padding: 0; background: transparent; }
.markdown :deep(blockquote) { margin: 8px 0; padding-left: 10px; border-left: 2px solid var(--border-strong); color: var(--fg-muted); }
.markdown :deep(table) { display: block; max-width: 100%; overflow-x: auto; border-collapse: collapse; font-size: 0.9167rem; }
.markdown :deep(th), .markdown :deep(td) { padding: 6px 8px; border: 1px solid var(--border-strong); white-space: nowrap; }
.markdown :deep(a) { color: var(--fg); text-decoration: underline; text-underline-offset: 3px; }
.markdown :deep(hr) { border: 0; border-top: 1px solid var(--border); }
</style>

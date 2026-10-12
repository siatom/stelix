<script setup lang="ts">
import { computed, nextTick, ref } from 'vue'
import { useAiStore } from '../../stores/ai'
import type { AiConversation } from '../../types'
import Icon from '../icons/Icon.vue'

const ai = useAiStore()
const emit = defineEmits<{ open: [] }>()
const query = ref('')
const renaming = ref('')
const renameDraft = ref('')
const renameInput = ref<HTMLInputElement[]>([])
const deleting = ref('')
const groups = computed(() => {
  const now = new Date()
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime()
  const yesterday = new Date(now.getFullYear(), now.getMonth(), now.getDate() - 1).getTime()
  const term = query.value.trim().toLocaleLowerCase()
  const filtered = ai.history.filter(c => !term || c.title.toLocaleLowerCase().includes(term) || c.messages.some(m => m.content.toLocaleLowerCase().includes(term)))
  return [
    { title: '今天', items: filtered.filter(c => c.updatedAt >= today) },
    { title: '昨天', items: filtered.filter(c => c.updatedAt >= yesterday && c.updatedAt < today) },
    { title: '更早', items: filtered.filter(c => c.updatedAt < yesterday) },
  ].filter(g => g.items.length)
})

function snippet(conversation: AiConversation) {
  const last = conversation.messages[conversation.messages.length - 1]
  return last?.content.replace(/\s+/g, ' ').slice(0, 100) || '正在回复…'
}
function open(id: string) {
  ai.selectConversation(id)
  if (ai.activeId === id) emit('open')
}
async function startRename(conversation: AiConversation) {
  renaming.value = conversation.id
  renameDraft.value = conversation.title
  deleting.value = ''
  await nextTick()
  renameInput.value[0]?.focus()
  renameInput.value[0]?.select()
}
async function saveRename() {
  if (await ai.renameConversation(renaming.value, renameDraft.value)) renaming.value = ''
}
async function remove(id: string) {
  if (await ai.deleteConversation(id)) deleting.value = ''
}
</script>

<template>
  <section class="chat-history" aria-label="历史对话管理">
    <div class="history-search"><Icon name="search" :size="14" /><input v-model="query" aria-label="搜索历史对话" placeholder="搜索标题或对话内容" spellcheck="false" /></div>
    <div v-if="ai.historyLoading" class="history-empty" role="status">正在读取历史对话…</div>
    <div v-else-if="!groups.length" class="history-empty"><Icon name="message-square" :size="24" /><p>{{ query.trim() ? '没有找到相关对话' : '还没有历史对话' }}</p><span>{{ query.trim() ? '换个关键词试试' : '发送第一条消息后会自动保存到本机' }}</span></div>
    <div v-else class="history-list">
      <section v-for="group in groups" :key="group.title" class="history-group">
        <h3>{{ group.title }}</h3>
        <div v-for="conversation in group.items" :key="conversation.id" class="history-item" :class="{ active: conversation.id === ai.activeId }">
          <div v-if="renaming === conversation.id" class="rename-form">
            <input ref="renameInput" v-model="renameDraft" aria-label="对话新名称" maxlength="120" @keydown.enter.prevent="saveRename" @keydown.esc="renaming = ''" />
            <div class="rename-actions"><button :disabled="!renameDraft.trim() || ai.historyMutating" @click="saveRename">保存</button><button :disabled="ai.historyMutating" @click="renaming = ''">取消</button></div>
          </div>
          <template v-else>
            <button class="conversation-main" :disabled="ai.executing || ai.historyMutating" :title="conversation.title" @click="open(conversation.id)"><span class="conversation-name">{{ conversation.title }}</span><span class="conversation-snippet">{{ snippet(conversation) }}</span><span class="conversation-date">{{ new Date(conversation.updatedAt).toLocaleString('zh-CN', { month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit', hour12: false }) }}<span v-if="conversation.id === ai.busyConversationId"> · {{ ai.executing ? '执行中' : '回复中' }}</span></span></button>
            <div class="history-actions"><button title="重命名对话" :aria-label="`重命名对话 ${conversation.title}`" :disabled="ai.historyMutating" @click="startRename(conversation)"><Icon name="pencil" :size="12" /></button><button title="删除对话" :aria-label="`删除对话 ${conversation.title}`" :disabled="ai.historyMutating || ai.executing || conversation.id === ai.busyConversationId" @click="deleting = conversation.id"><Icon name="trash" :size="12" /></button></div>
          </template>
          <div v-if="deleting === conversation.id" class="delete-confirm"><p>删除这条对话及其全部消息？</p><div><button class="danger" :disabled="ai.historyMutating" @click="remove(conversation.id)">删除</button><button :disabled="ai.historyMutating" @click="deleting = ''">取消</button></div></div>
        </div>
      </section>
    </div>
    <p class="history-hint">历史仅保存在本机，继续对话时会使用当前模型。</p>
  </section>
</template>

<style scoped>
.chat-history { flex: 1; min-height: 0; display: flex; flex-direction: column; gap: 12px; padding: 12px; }
.history-search { display: flex; align-items: center; gap: 7px; padding: 0 9px; border: 1px solid var(--border-strong); border-radius: var(--radius-sm); color: var(--fg-dim); }
.history-search input { width: 100%; min-width: 0; height: 34px; padding: 0; border: 0; background: transparent; font-size: 0.9167rem; }
.history-search:focus-within { border-color: var(--accent); box-shadow: 0 0 0 2px var(--accent-soft); }
.history-search input:focus { outline: none; box-shadow: none; }
.history-list { flex: 1; min-height: 0; overflow: auto; }
.history-group + .history-group { margin-top: 18px; }
.history-group h3 { margin: 0 0 7px 4px; font-size: 0.8333rem; font-weight: 500; color: var(--fg-dim); }
.history-item { position: relative; margin-bottom: 6px; border: 1px solid transparent; border-radius: var(--radius-sm); }
.history-item:hover { background: var(--bg-hover); }
.history-item.active { background: var(--bg-active); }
.conversation-main { display: flex; flex-direction: column; gap: 5px; width: 100%; padding: 10px 12px; padding-right: 54px; text-align: left; }
.conversation-name { max-width: 100%; overflow: hidden; white-space: nowrap; text-overflow: ellipsis; color: var(--fg); font-size: 1rem; font-weight: 500; }
.conversation-snippet { max-width: 100%; overflow: hidden; white-space: nowrap; text-overflow: ellipsis; color: var(--fg-muted); font-size: 0.875rem; }
.conversation-date { color: var(--fg-dim); font-size: 0.75rem; }
.history-actions { position: absolute; top: 8px; right: 5px; display: flex; gap: 1px; }
.history-actions button { display: flex; align-items: center; justify-content: center; width: 22px; height: 24px; color: var(--fg-dim); border-radius: 4px; }
.history-actions button:hover:not(:disabled) { color: var(--fg); background: var(--bg-selected); }
.history-empty { flex: 1; display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 10px; color: var(--fg-dim); font-size: 0.9167rem; text-align: center; line-height: 1.6; }
.history-empty p { margin: 0; color: var(--fg-muted); }
.history-empty span { font-size: 0.8333rem; }
.history-hint { margin: 0; color: var(--fg-dim); font-size: 0.8333rem; line-height: 1.5; }
.rename-form { padding: 10px; }
.rename-form input { width: 100%; min-width: 0; border-radius: 6px; font-size: 0.9167rem; }
.rename-actions { display: flex; justify-content: flex-end; gap: 6px; margin-top: 8px; }
.rename-actions button, .delete-confirm button { padding: 4px 8px; border: 1px solid var(--border-strong); border-radius: 6px; font-size: 0.8333rem; }
.delete-confirm { padding: 0 10px 10px; font-size: 0.875rem; }
.delete-confirm p { margin: 0 0 8px; color: var(--fg-muted); }
.delete-confirm div { display: flex; justify-content: flex-end; gap: 6px; }
.danger { color: var(--danger); }
</style>

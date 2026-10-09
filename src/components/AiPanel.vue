<script setup lang="ts">
// TODO(WS-D / feat/ui-panes): AI 对话面板（流式输出 + 写操作二次确认 + 配置入口）
import { ref } from 'vue'
import { useAiStore } from '../stores/ai'

const ai = useAiStore()
const draft = ref('')

async function send() {
  const text = draft.value
  draft.value = ''
  await ai.send(text)
}
</script>

<template>
  <aside class="ai-panel">
    <header class="ai-head">AI 助手</header>
    <div class="ai-body">
      <p v-if="ai.messages.length === 0" class="hint">试试：「找出这个目录下最大的 5 个文件」</p>
      <div v-for="(m, i) in ai.messages" :key="i" class="msg" :class="m.role">
        {{ m.content }}
      </div>
      <div v-if="ai.pendingConfirm" class="confirm">
        <p>AI 请求执行写操作，是否允许？</p>
        <button class="ok" @click="ai.confirmPending(true)">允许</button>
        <button class="no" @click="ai.confirmPending(false)">取消</button>
      </div>
    </div>
    <footer class="ai-input">
      <input v-model="draft" placeholder="用自然语言描述你的需求…" @keyup.enter="send" />
      <button @click="send">发送</button>
    </footer>
  </aside>
</template>

<style scoped>
.ai-panel {
  width: var(--ai-w);
  height: 100%;
  background: var(--bg-panel);
  border-left: 1px solid var(--border);
  display: flex;
  flex-direction: column;
}
.ai-head {
  height: var(--toolbar-h);
  display: flex;
  align-items: center;
  padding: 0 10px;
  border-bottom: 1px solid var(--border);
  font-size: 12px;
  color: var(--fg-muted);
}
.ai-body {
  flex: 1;
  overflow: auto;
  padding: 10px;
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.msg {
  padding: 8px 10px;
  border-radius: var(--radius);
  background: var(--bg-elevated);
  white-space: pre-wrap;
  word-break: break-word;
}
.msg.user {
  background: var(--bg-selected);
}
.confirm {
  border: 1px solid var(--warn);
  border-radius: var(--radius);
  padding: 8px;
}
.confirm .ok {
  color: var(--ok);
  margin-right: 10px;
}
.confirm .no {
  color: var(--danger);
}
.ai-input {
  display: flex;
  gap: 6px;
  padding: 8px;
  border-top: 1px solid var(--border);
}
.ai-input input {
  flex: 1;
}
.hint {
  color: var(--fg-dim);
  font-size: 12px;
}
</style>

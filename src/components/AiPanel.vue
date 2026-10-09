<script setup lang="ts">
// AI 对话面板（流式输出 + 写操作二次确认 + 配置入口）（WS-D）
import { computed, nextTick, onMounted, ref, watch } from 'vue'
import type { AiConfig } from '../types'
import { useAiStore } from '../stores/ai'

const ai = useAiStore()
const draft = ref('')
const bodyRef = ref<HTMLElement | null>(null)
const showSettings = ref(false)

const cfgDraft = ref<AiConfig>({ baseUrl: '', apiKey: '', model: '', enabled: false })

const examples = [
  '找出这个目录下最大的 5 个文件',
  '把所有 png 图片列出来',
  '按扩展名统计当前目录',
  '删除所有 .tmp 临时文件',
]

onMounted(() => {
  void ai.loadConfig()
})

// 打开设置时同步当前配置
watch(showSettings, (open) => {
  if (open) cfgDraft.value = { ...ai.config }
})

// 新消息自动滚到底部
watch(
  () => ai.messages.map((m) => m.content.length).join(','),
  async () => {
    await nextTick()
    const el = bodyRef.value
    if (el) el.scrollTop = el.scrollHeight
  },
)

function escapeHtml(s: string): string {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
}

/** 极简 Markdown：代码块 + 换行（不引入依赖，先转义再拼装，避免 XSS） */
function renderMarkdown(text: string): string {
  const parts = escapeHtml(text).split('```')
  return parts
    .map((seg, i) => {
      if (i % 2 === 1) {
        const nl = seg.indexOf('\n')
        const body = nl >= 0 ? seg.slice(nl + 1) : seg
        return `<pre class="code-block"><code>${body.replace(/\n$/, '')}</code></pre>`
      }
      return seg.replace(/\n/g, '<br/>')
    })
    .join('')
}

function send() {
  const text = draft.value
  if (!text.trim() || ai.streaming) return
  draft.value = ''
  void ai.send(text)
}

function useExample(text: string) {
  draft.value = text
}

function saveConfig() {
  void ai.saveConfig({ ...cfgDraft.value })
  showSettings.value = false
}

const confirmSummary = computed(() => {
  const op = ai.pendingConfirm
  if (!op) return null
  return {
    op: op.op,
    sources: op.sources ?? [],
    target: op.target ?? '',
    call: ai.pendingCall?.name ?? '',
  }
})

const OP_LABEL: Record<string, string> = {
  copy: '复制',
  move: '移动',
  delete: '删除',
  rename: '重命名',
  mkdir: '新建文件夹',
}
</script>

<template>
  <aside class="ai-panel">
    <header class="ai-head">
      <span class="title">🤖 AI 助手</span>
      <div class="spacer" />
      <button class="head-btn" title="设置" @click="showSettings = !showSettings">⚙</button>
      <button class="head-btn" title="清空对话" @click="ai.clear">🗑</button>
    </header>

    <!-- 配置区 -->
    <div v-if="showSettings" class="settings">
      <label class="row">
        <span>启用</span>
        <input v-model="cfgDraft.enabled" type="checkbox" class="cb" />
      </label>
      <label class="row">
        <span>Base URL</span>
        <input v-model="cfgDraft.baseUrl" placeholder="https://api.openai.com/v1" spellcheck="false" />
      </label>
      <label class="row">
        <span>API Key</span>
        <input v-model="cfgDraft.apiKey" type="password" placeholder="sk-…" spellcheck="false" />
      </label>
      <label class="row">
        <span>模型</span>
        <input v-model="cfgDraft.model" placeholder="gpt-4o-mini" spellcheck="false" />
      </label>
      <div class="settings-actions">
        <button @click="showSettings = false">取消</button>
        <button class="primary" @click="saveConfig">保存</button>
      </div>
    </div>

    <div ref="bodyRef" class="ai-body">
      <div v-if="!ai.messages.length" class="empty">
        <p class="empty-title">你好，我可以帮你整理文件</p>
        <button v-for="ex in examples" :key="ex" class="example" @click="useExample(ex)">{{ ex }}</button>
      </div>

      <div v-for="(m, i) in ai.messages" :key="i" class="msg" :class="m.role">
        <div v-if="m.role === 'assistant'" class="bubble" v-html="renderMarkdown(m.content)" />
        <div v-else class="bubble">{{ m.content }}</div>
      </div>

      <div v-if="ai.streaming" class="typing">正在思考…</div>

      <!-- 写操作二次确认 -->
      <div v-if="confirmSummary" class="confirm">
        <p class="confirm-title">⚠️ AI 请求执行写操作</p>
        <ul class="confirm-detail">
          <li>操作：{{ OP_LABEL[confirmSummary.op] ?? confirmSummary.op }}</li>
          <li v-if="confirmSummary.call">工具：{{ confirmSummary.call }}</li>
          <li v-if="confirmSummary.sources.length">
            源：{{ confirmSummary.sources.slice(0, 4).join('、') }}
            <template v-if="confirmSummary.sources.length > 4">…（共 {{ confirmSummary.sources.length }} 项）</template>
          </li>
          <li v-if="confirmSummary.target">目标：{{ confirmSummary.target }}</li>
        </ul>
        <div class="confirm-actions">
          <button class="ok" @click="ai.confirmPending(true)">允许</button>
          <button class="no" @click="ai.confirmPending(false)">取消</button>
        </div>
      </div>

      <p v-if="ai.error && !ai.messages.length" class="err">{{ ai.error }}</p>
    </div>

    <footer class="ai-input">
      <textarea
        v-model="draft"
        rows="2"
        placeholder="用自然语言描述需求…（Enter 发送，Shift+Enter 换行）"
        spellcheck="false"
        @keydown.enter.exact.prevent="send"
      />
      <button class="send" :disabled="ai.streaming || !draft.trim()" @click="send">发送</button>
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
  overflow: hidden;
}
.ai-head {
  flex: none;
  height: var(--toolbar-h);
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 0 10px;
  border-bottom: 1px solid var(--border);
  font-size: 12px;
  color: var(--fg-muted);
}
.title {
  color: var(--fg);
}
.spacer {
  flex: 1;
}
.head-btn {
  width: 24px;
  height: 22px;
  border-radius: var(--radius);
  color: var(--fg-muted);
}
.head-btn:hover {
  background: var(--bg-hover);
  color: var(--fg);
}

/* 配置区 */
.settings {
  flex: none;
  padding: 8px 10px;
  border-bottom: 1px solid var(--border);
  background: var(--bg-elevated);
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.settings .row {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 12px;
  color: var(--fg-muted);
}
.settings .row span {
  width: 66px;
  flex: none;
}
.settings .row input {
  flex: 1;
  min-width: 0;
  height: 22px;
  font-size: 12px;
}
.settings .row input.cb {
  flex: none;
  width: 14px;
  height: 14px;
}
.settings-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}
.settings-actions button {
  padding: 3px 12px;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  color: var(--fg-muted);
}
.settings-actions button:hover {
  color: var(--fg);
  background: var(--bg-hover);
}
.settings-actions button.primary {
  background: var(--accent);
  border-color: var(--accent);
  color: var(--accent-fg);
}

/* 消息区 */
.ai-body {
  flex: 1;
  min-height: 0;
  overflow: auto;
  padding: 10px;
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.empty {
  display: flex;
  flex-direction: column;
  gap: 6px;
  margin-top: 8px;
}
.empty-title {
  margin: 0 0 4px;
  font-size: 12px;
  color: var(--fg-dim);
}
.example {
  text-align: left;
  padding: 6px 10px;
  border: 1px dashed var(--border-strong);
  border-radius: var(--radius);
  font-size: 12px;
  color: var(--fg-muted);
}
.example:hover {
  background: var(--bg-hover);
  color: var(--fg);
}
.msg {
  display: flex;
}
.msg.user {
  justify-content: flex-end;
}
.msg.assistant {
  justify-content: flex-start;
}
.bubble {
  max-width: 92%;
  padding: 7px 10px;
  border-radius: 8px;
  background: var(--bg-elevated);
  font-size: 12.5px;
  line-height: 1.5;
  white-space: pre-wrap;
  word-break: break-word;
  user-select: text;
}
.msg.user .bubble {
  background: var(--bg-selected);
}
.bubble :deep(.code-block) {
  margin: 6px 0;
  padding: 8px;
  background: var(--bg-input);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  font-family: var(--mono);
  font-size: 11.5px;
  white-space: pre;
  overflow-x: auto;
}
.typing {
  font-size: 12px;
  color: var(--fg-dim);
  padding-left: 4px;
}

/* 确认卡片 */
.confirm {
  border: 1px solid var(--warn);
  border-radius: var(--radius);
  padding: 8px 10px;
  background: rgba(210, 153, 34, 0.1);
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.confirm-title {
  margin: 0;
  font-size: 12.5px;
  color: var(--warn);
}
.confirm-detail {
  margin: 0;
  padding-left: 16px;
  font-size: 12px;
  color: var(--fg);
  word-break: break-all;
}
.confirm-actions {
  display: flex;
  gap: 8px;
}
.confirm-actions button {
  padding: 3px 14px;
  border-radius: var(--radius);
  border: 1px solid var(--border);
}
.confirm-actions .ok {
  color: var(--ok);
  border-color: var(--ok);
}
.confirm-actions .no {
  color: var(--danger);
  border-color: var(--danger);
}
.err {
  margin: 0;
  font-size: 12px;
  color: var(--danger);
}

/* 输入区 */
.ai-input {
  flex: none;
  display: flex;
  gap: 6px;
  padding: 8px;
  border-top: 1px solid var(--border);
  align-items: flex-end;
}
.ai-input textarea {
  flex: 1;
  min-width: 0;
  resize: none;
  font-family: inherit;
  font-size: 12.5px;
  color: var(--fg);
  background: var(--bg-input);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  padding: 6px 8px;
  outline: none;
}
.ai-input textarea:focus {
  border-color: var(--accent);
}
.send {
  flex: none;
  padding: 6px 14px;
  border-radius: var(--radius);
  background: var(--accent);
  color: var(--accent-fg);
  font-size: 12.5px;
}
.send:disabled {
  opacity: 0.45;
  cursor: default;
}
</style>

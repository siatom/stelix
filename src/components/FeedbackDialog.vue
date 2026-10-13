<script setup lang="ts">
// 意见反馈弹窗：在本机 gitee CLI 上直接把反馈创建为 Gitee issue。
// 入口在活动栏底部（ActivityBar），状态由 ui store 的 feedbackOpen 控制。
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { openUrl } from '@tauri-apps/plugin-opener'
import { api } from '../api'
import { useUiStore } from '../stores/ui'
import Icon from './icons/Icon.vue'
import type { FeedbackAuth, FeedbackCreated } from '../types'

type FeedbackKind = 'Bug' | '功能建议' | '其它'

const KINDS: FeedbackKind[] = ['Bug', '功能建议', '其它']

const ui = useUiStore()
const appVersion = __APP_VERSION__
const commitId = __COMMIT_ID__

const kind = ref<FeedbackKind>('Bug')
const title = ref('')
const description = ref('')
const submitting = ref(false)
const error = ref('')
const created = ref<FeedbackCreated | null>(null)
const auth = ref<FeedbackAuth | null>(null)
const copied = ref(false)

/** 已登录且 CLI 可用时才能提交。 */
const canSubmit = computed(
  () => auth.value?.cliFound === true && auth.value.loggedIn && !submitting.value && title.value.trim().length > 0 && description.value.trim().length > 0,
)

function onKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape') ui.closeFeedback()
}

onMounted(() => window.addEventListener('keydown', onKeydown))
onUnmounted(() => window.removeEventListener('keydown', onKeydown))

/** 每次打开都重置表单并重新检查 CLI 状态（用户可能刚装好 / 刚登录）。 */
watch(
  () => ui.feedbackOpen,
  (open) => {
    if (!open) return
    kind.value = 'Bug'
    title.value = ''
    description.value = ''
    error.value = ''
    created.value = null
    copied.value = false
    void refreshAuth()
  },
)

async function refreshAuth() {
  auth.value = null
  try {
    auth.value = await api.feedbackAuthStatus()
  } catch (e) {
    auth.value = {
      cliFound: false,
      loggedIn: false,
      message: `无法检查 gitee CLI 状态：${String(e)}`,
    }
  }
}

async function submit() {
  if (!canSubmit.value) return
  submitting.value = true
  error.value = ''
  try {
    created.value = await api.feedbackSubmit({
      kind: kind.value,
      title: title.value.trim(),
      description: description.value.trim(),
      appVersion,
      commit: commitId,
    })
    ui.setStatusText(`反馈已提交：${created.value.number}`)
  } catch (e) {
    error.value = String(e)
  } finally {
    submitting.value = false
  }
}

async function openCreated() {
  if (!created.value) return
  try {
    await openUrl(created.value.url)
  } catch (e) {
    ui.setStatusText(`打开链接失败：${String(e)}`)
  }
}

async function copyLink() {
  if (!created.value) return
  try {
    await navigator.clipboard.writeText(created.value.url)
    copied.value = true
    ui.setStatusText('已复制 issue 链接')
  } catch (e) {
    ui.setStatusText(`复制失败：${String(e)}`)
  }
}
</script>

<template>
  <Teleport to="body">
    <div v-if="ui.feedbackOpen" class="feedback-backdrop" @mousedown.self="ui.closeFeedback()">
      <section class="feedback-dialog" role="dialog" aria-modal="true" aria-labelledby="feedback-title">
        <header class="dialog-head">
          <div>
            <h2 id="feedback-title">意见反馈</h2>
            <p>经本机 gitee CLI 直接创建 issue，提交到 silicon-atom/stelix</p>
          </div>
          <button type="button" class="close-btn" title="关闭" aria-label="关闭反馈" @click="ui.closeFeedback()">
            <Icon name="x" :size="16" />
          </button>
        </header>

        <div class="dialog-body">
          <!-- 提交成功：展示 issue 编号与链接 -->
          <div v-if="created" class="fb-success">
            <Icon name="check-circle" :size="30" />
            <p class="fb-success-title">反馈已提交</p>
            <p class="fb-success-no">{{ created.number }}</p>
            <button type="button" class="fb-link" :title="created.url" @click="openCreated">
              <Icon name="external-link" :size="13" />
              <span>{{ created.url }}</span>
            </button>
            <div class="fb-actions">
              <button type="button" class="fb-btn ghost" @click="copyLink">
                <Icon name="copy" :size="13" />{{ copied ? '已复制' : '复制链接' }}
              </button>
              <button type="button" class="fb-btn primary" @click="openCreated">
                <Icon name="external-link" :size="13" />在浏览器打开
              </button>
            </div>
            <p class="fb-hint">感谢反馈，可以关闭本窗口继续使用。</p>
          </div>

          <template v-else>
            <!-- CLI 状态：未安装 / 未登录时给出可操作指引，并阻止提交 -->
            <div
              v-if="auth && (!auth.cliFound || !auth.loggedIn)"
              class="fb-banner"
              :class="auth.cliFound ? 'warn' : 'danger'"
            >
              <Icon :name="auth.cliFound ? 'alert-triangle' : 'alert-circle'" :size="14" />
              <p>{{ auth.message }}</p>
              <button type="button" class="fb-btn ghost sm" @click="refreshAuth">
                <Icon name="refresh" :size="12" />重新检查
              </button>
            </div>
            <div v-else-if="auth" class="fb-banner ok">
              <Icon name="check-circle" :size="14" />
              <p>{{ auth.message }}</p>
            </div>
            <div v-else class="fb-banner">
              <Icon name="loader" :size="14" class="spin" />
              <p>正在检查 gitee CLI…</p>
            </div>

            <label class="fb-field">
              <span>反馈类型</span>
              <div class="fb-kinds">
                <button
                  v-for="k in KINDS"
                  :key="k"
                  type="button"
                  class="fb-kind"
                  :class="{ active: kind === k }"
                  @click="kind = k"
                >
                  {{ k }}
                </button>
              </div>
            </label>

            <label class="fb-field">
              <span>标题</span>
              <input v-model="title" maxlength="120" placeholder="一句话描述问题或建议" spellcheck="false" />
            </label>

            <label class="fb-field">
              <span>详细描述</span>
              <textarea
                v-model="description"
                rows="8"
                placeholder="复现步骤、期望结果、或你的建议…"
                spellcheck="false"
              />
            </label>

            <p class="fb-meta">
              提交时自动附带：版本 {{ appVersion }} · Commit {{ commitId }} · 操作系统与架构
            </p>

            <p v-if="error" class="fb-error">{{ error }}</p>

            <div class="fb-actions">
              <button type="button" class="fb-btn ghost" @click="ui.closeFeedback()">取消</button>
              <button type="button" class="fb-btn primary" :disabled="!canSubmit" @click="submit">
                <Icon :name="submitting ? 'loader' : 'send'" :size="13" :class="{ spin: submitting }" />
                {{ submitting ? '提交中…' : '提交反馈' }}
              </button>
            </div>
          </template>
        </div>
      </section>
    </div>
  </Teleport>
</template>

<style scoped>
.feedback-backdrop {
  position: fixed;
  z-index: 120;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 28px;
  background: var(--mask);
}
.feedback-dialog {
  display: flex;
  flex-direction: column;
  width: min(620px, 100%);
  max-height: min(760px, 100%);
  background: var(--bg-panel);
  border: 1px solid var(--border-strong);
  border-radius: calc(var(--radius) * 1.5);
  box-shadow: var(--shadow-lg);
  overflow: hidden;
}
.dialog-head {
  display: flex;
  flex: none;
  align-items: center;
  justify-content: space-between;
  padding: 16px 20px;
  border-bottom: 1px solid var(--border);
}
.dialog-head h2 {
  margin: 0;
  font-size: 1.25rem;
}
.dialog-head p {
  margin: 4px 0 0;
  color: var(--fg-dim);
  font-size: 0.9167rem;
}
.close-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 30px;
  height: 30px;
  border-radius: var(--radius-sm);
  color: var(--fg-muted);
}
.close-btn:hover {
  background: var(--bg-hover);
  color: var(--fg);
}
.dialog-body {
  display: flex;
  flex: 1;
  flex-direction: column;
  gap: 14px;
  min-height: 0;
  padding: 16px 20px 18px;
  overflow: auto;
}
.fb-banner {
  display: flex;
  align-items: flex-start;
  gap: 8px;
  padding: 10px 12px;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--bg-elevated);
  color: var(--fg-muted);
  font-size: 0.9167rem;
}
.fb-banner p {
  flex: 1;
  margin: 0;
  white-space: pre-line;
  line-height: 1.5;
}
.fb-banner.ok {
  border-color: var(--border);
  background: var(--ok-soft);
  color: var(--fg);
}
.fb-banner.warn {
  border-color: var(--warn);
  background: var(--warn-soft);
  color: var(--fg);
}
.fb-banner.danger {
  border-color: var(--danger);
  background: var(--danger-soft);
  color: var(--fg);
}
.fb-field {
  display: flex;
  flex-direction: column;
  gap: 6px;
  color: var(--fg-muted);
  font-size: 0.9167rem;
}
.fb-field > span {
  color: var(--fg);
  font-weight: 600;
}
.fb-field input,
.fb-field textarea {
  width: 100%;
  padding: 8px 10px;
  border-radius: var(--radius);
  resize: vertical;
}
.fb-kinds {
  display: flex;
  gap: 8px;
}
.fb-kind {
  flex: 1;
  min-height: 32px;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--bg-input);
  color: var(--fg-muted);
  font-size: 0.9167rem;
}
.fb-kind:hover {
  background: var(--bg-hover);
  color: var(--fg);
}
.fb-kind.active {
  border-color: var(--border-strong);
  background: var(--bg-active);
  color: var(--fg);
  font-weight: 600;
}
.fb-meta {
  margin: 0;
  color: var(--fg-dim);
  font-size: 0.8333rem;
  line-height: 1.5;
}
.fb-error {
  margin: 0;
  padding: 10px 12px;
  border: 1px solid var(--danger);
  border-radius: var(--radius);
  background: var(--danger-soft);
  color: var(--fg);
  font-size: 0.9167rem;
  white-space: pre-line;
  line-height: 1.5;
}
.fb-actions {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 8px;
}
.fb-btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  min-height: 32px;
  padding: 0 14px;
  border: 1px solid transparent;
  border-radius: var(--radius);
  font-size: 0.9167rem;
}
.fb-btn.sm {
  min-height: 26px;
  padding: 0 10px;
  font-size: 0.8333rem;
}
.fb-btn.primary {
  border-color: var(--border-strong);
  background: var(--accent);
  color: var(--accent-fg);
  font-weight: 600;
}
.fb-btn.primary:hover:not(:disabled) {
  filter: brightness(1.08);
}
.fb-btn.primary:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}
.fb-btn.ghost {
  border-color: var(--border);
  background: transparent;
  color: var(--fg-muted);
}
.fb-btn.ghost:hover {
  background: var(--bg-hover);
  color: var(--fg);
}
.fb-success {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 8px;
  padding: 18px 0 6px;
  text-align: center;
  color: var(--ok);
}
.fb-success-title {
  margin: 4px 0 0;
  color: var(--fg);
  font-size: 1.0833rem;
  font-weight: 600;
}
.fb-success-no {
  margin: 0;
  color: var(--fg-muted);
  font-family: var(--mono);
  font-size: 0.9167rem;
}
.fb-link {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  max-width: 100%;
  padding: 6px 10px;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--bg-elevated);
  color: var(--fg-muted);
  font-size: 0.8333rem;
}
.fb-link:hover {
  color: var(--fg);
  background: var(--bg-hover);
}
.fb-link span {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.fb-success .fb-actions {
  margin-top: 6px;
}
.fb-hint {
  margin: 0;
  color: var(--fg-dim);
  font-size: 0.8333rem;
}
.spin {
  animation: fb-spin 1s linear infinite;
}
@keyframes fb-spin {
  to {
    transform: rotate(360deg);
  }
}
</style>

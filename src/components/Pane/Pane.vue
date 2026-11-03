<script setup lang="ts">
// 单个窗格 = 标签栏 + 内嵌工具栏 + 面包屑 + 文件列表（虚拟滚动）（WS-D）
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { usePaneStore } from '../../stores/pane'
import { isCapturingShortcut } from '../../stores/shortcuts'
import { basename, RECYCLE_BIN_SHELL_PATH } from './utils'
import TabBar from './TabBar.vue'
import PaneToolbar from './PaneToolbar.vue'
import PathBar from './PathBar.vue'
import FileList from './FileList.vue'
import TrashView from './TrashView.vue'
import TerminalView from '../Terminal/TerminalView.vue'
import StatusBar from '../StatusBar.vue'
import Icon from '../icons/Icon.vue'
import FileIcon from '../icons/FileIcon.vue'
import { useTerminalStore } from '../../stores/terminal'
import type { TabState } from '../../types'
import type { TerminalSession } from '../../stores/terminal'

const props = defineProps<{ paneId: string; dropActive?: boolean }>()

const paneStore = usePaneStore()
const terminalStore = useTerminalStore()
/** 回收站标签的内容与操作由子组件维护，标题栏按钮通过它读取选中态并触发操作。 */
const trashView = ref<InstanceType<typeof TrashView> | null>(null)
const pane = computed(() => paneStore.panes.find((p) => p.id === props.paneId))
const isActivePane = computed(() => paneStore.activePaneId === props.paneId)
const tab = computed(() => {
  const p = pane.value
  if (!p) return undefined
  return p.tabs.find((t) => t.id === p.activeTabId) ?? p.tabs[0]
})

/** 终端标签对应的 SSH 会话（会话在标签创建时同步登记）。 */
const terminalSession = computed(() => {
  const id = tab.value?.terminalId
  return id ? terminalStore.sessions.find((session) => session.id === id) : undefined
})

/**
 * 本窗格内的所有终端标签。
 *
 * xterm 实例按标签常驻（`v-show` 切换）：切到目录标签再切回来时滚动历史仍在，
 * 会话输出也不会因为组件卸载而丢缓冲。
 */
const terminalTabs = computed(() =>
  (pane.value?.tabs ?? [])
    .filter((item) => item.kind === 'terminal' && !!item.terminalId)
    .map((item) => ({
      tab: item,
      session: terminalStore.sessions.find((session) => session.id === item.terminalId),
    }))
    .filter(
      (item): item is { tab: TabState; session: TerminalSession } => !!item.session,
    ),
)

const terminalStatusText = computed(() => {
  switch (terminalSession.value?.status) {
    case 'connecting':
      return '连接中'
    case 'connected':
      return '已连接'
    case 'closed':
      return '已断开'
    case 'error':
      return '连接失败'
    default:
      return ''
  }
})

function activate() {
  paneStore.activePaneId = props.paneId
}

// ---- 顶部标题栏「新建 / 删除」→ 当前活动窗格 ----
// 标题栏（App.vue）把操作写进 store 的 pendingAction（目标是活动窗格），
// 对应窗格在这里消费：新建转成一次 createRequest 交给 FileList 行内编辑，删除直接开确认框。
// Pane 与 FileList 是父子组件，用一次性请求对象中转；FileList 处理完回发 create-handled。
const createRequest = ref<{ kind: 'mkdir' | 'createFile'; seq: number; tabId: string } | null>(null)
let createSeq = 0

watch(
  () => paneStore.pendingAction,
  (req) => {
    if (!req || req.paneId !== props.paneId) return
    paneStore.consumeAction(req.seq)
    if (paneStore.activePaneId !== props.paneId) return // 请求派发后焦点已转移，丢弃
    const t = tab.value
    if (!t) return
    if (req.kind === 'delete') {
      if (paneStore.canDeleteInActivePane) openDelete([...t.selected], req.permanent)
      return
    }
    createSeq += 1
    createRequest.value = { kind: req.kind, seq: createSeq, tabId: t.id }
  },
)

function onCreateHandled() {
  createRequest.value = null
}

// ---- 删除确认对话框 ----

const deleteDialog = ref<string[] | null>(null)
/** 当前确认框是否走「永久删除」；Shift+Delete / Shift 点击标题栏删除时置为 true。 */
const permanentDelete = ref(false)
const errorMsg = ref<string | null>(null)
const busy = ref(false)

function handleDeleteDialogKeydown(event: KeyboardEvent) {
  if (isCapturingShortcut()) return // 设置里正在捕获新键位
  if (!deleteDialog.value || event.key !== 'Enter' || event.isComposing) return
  event.preventDefault()
  event.stopPropagation()
  if (!busy.value) void confirmDelete()
}

onMounted(() => window.addEventListener('keydown', handleDeleteDialogKeydown, true))
onBeforeUnmount(() => window.removeEventListener('keydown', handleDeleteDialogKeydown, true))

function openDelete(paths: string[], permanent = false) {
  errorMsg.value = null
  permanentDelete.value = permanent
  deleteDialog.value = [...paths]
}

function closeDialogs() {
  deleteDialog.value = null
  permanentDelete.value = false
}

async function confirmDelete() {
  const paths = deleteDialog.value
  if (!paths || !paths.length) return
  busy.value = true
  try {
    const res = await paneStore.runFs(props.paneId, {
      op: 'delete',
      sources: paths,
      permanent: permanentDelete.value,
    })
    if (!res.ok) errorMsg.value = res.errors.join('; ')
    deleteDialog.value = null
    permanentDelete.value = false
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <section class="pane" :data-pane-id="paneId" @mousedown="activate">
    <template v-if="tab">
      <div class="pane-topbar" :class="{ 'is-active': isActivePane }">
        <div v-if="tab.kind === 'terminal'" class="terminal-heading">
          <Icon name="terminal" :size="15" />
          <span class="terminal-host">
            {{ terminalSession ? `${terminalSession.location.username}@${terminalSession.location.host}:${terminalSession.location.port}` : tab.title }}
          </span>
          <span class="terminal-state" :class="terminalSession?.status">{{ terminalStatusText }}</span>
          <span v-if="terminalSession?.fingerprint" class="terminal-fingerprint" :title="`主机密钥 ${terminalSession.fingerprint}`">
            {{ terminalSession.fingerprint }}
          </span>
        </div>
        <template v-else>
          <PaneToolbar :pane-id="paneId" />
          <div v-if="tab.kind === 'search'" class="search-heading" :title="tab.searchQuery">
            搜索结果：{{ tab.searchQuery }}
          </div>
          <div v-else-if="tab.kind === 'trash'" class="trash-heading">
            <FileIcon
              :path="RECYCLE_BIN_SHELL_PATH"
              :is-dir="false"
              :ext="null"
              :trash-state="true"
              fallback="trash"
              :size="15"
            />
            <span class="trash-title">回收站</span>
            <div class="trash-actions">
              <button
                :disabled="!trashView?.canRestore"
                title="还原所选到原位置"
                @click="trashView?.restoreSelected()"
              >
                <Icon name="restore" :size="15" /><span>还原</span>
              </button>
              <button
                :disabled="!trashView?.canOpenOriginal"
                title="打开所选条目的原位置"
                @click="trashView?.openOriginalSelected()"
              >
                <Icon name="folder-arrow" :size="15" /><span>原位置</span>
              </button>
              <button
                class="danger"
                :disabled="!trashView?.canPurge"
                title="跳过回收站永久删除所选"
                @click="trashView?.askPurgeSelected()"
              >
                <Icon name="trash" :size="15" /><span>永久删除</span>
              </button>
              <button
                class="danger"
                :disabled="!trashView?.canEmpty"
                title="永久删除回收站中的全部条目"
                @click="trashView?.askEmpty()"
              >
                <Icon name="trash" :size="15" /><span>清空</span>
              </button>
            </div>
          </div>
          <PathBar v-else :pane-id="paneId" :tab="tab" />
        </template>
      </div>
      <FileList
        v-if="tab.kind !== 'terminal' && tab.kind !== 'trash'"
        :pane-id="paneId"
        :tab="tab"
        :create-request="createRequest"
        @delete="openDelete"
        @create-handled="onCreateHandled"
      />
      <TrashView v-else-if="tab.kind === 'trash'" ref="trashView" :pane-id="paneId" :tab="tab" />
      <TerminalView
        v-for="item in terminalTabs"
        v-show="tab.kind === 'terminal' && tab.id === item.tab.id"
        :key="item.tab.id"
        class="pane-terminal"
        :session="item.session"
        :active="isActivePane && tab.id === item.tab.id"
      />
    </template>
    <div v-else class="state">没有可用标签</div>
    <div v-if="dropActive" class="drop-overlay"><span>复制到此文件夹</span></div>
    <TabBar :pane-id="paneId" />
    <StatusBar
      :pane-id="paneId"
      :trash-count="tab?.kind === 'trash' ? trashView?.count ?? 0 : undefined"
      :trash-selected-count="tab?.kind === 'trash' ? trashView?.selectedCount ?? 0 : undefined"
    />

    <div v-if="errorMsg" class="error-bar">
      <span>{{ errorMsg }}</span>
      <button @click="errorMsg = null"><Icon name="x" :size="13" /></button>
    </div>

    <!-- 删除确认 -->
    <div v-if="deleteDialog" class="mask" @mousedown.stop @click.self="closeDialogs">
      <div class="dialog">
        <h3>{{ permanentDelete ? '确认永久删除' : '确认删除' }}</h3>
        <p v-if="permanentDelete" class="warn-text danger">
          将永久删除以下 {{ deleteDialog.length }} 项，不进入回收站，此操作不可撤销：
        </p>
        <p v-else class="warn-text">将把以下 {{ deleteDialog.length }} 项移到系统回收站，之后可在回收站中还原：</p>
        <ul class="del-list">
          <li v-for="p in deleteDialog.slice(0, 8)" :key="p">{{ basename(p) }}</li>
        </ul>
        <p v-if="deleteDialog.length > 8" class="more">… 等共 {{ deleteDialog.length }} 项</p>
        <div class="actions">
          <button class="mode-toggle" :disabled="busy" @click="permanentDelete = !permanentDelete">
            {{ permanentDelete ? '改为移到回收站' : '永久删除…' }}
          </button>
          <button :disabled="busy" @click="closeDialogs">取消</button>
          <button class="danger" :disabled="busy" @click="confirmDelete">
            {{ permanentDelete ? '永久删除' : '移到回收站' }}
          </button>
        </div>
      </div>
    </div>
  </section>
</template>

<style scoped>
.pane {
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
  background: transparent;
  /* 不加描边：与侧栏靠背景深浅区分，与对侧窗格靠 DualPane 的 1px 分割线区分 */
  position: relative;
  overflow: hidden;
}
.drop-overlay {
  position: absolute;
  inset: 8px;
  z-index: 20;
  display: grid;
  place-items: center;
  border: 2px dashed var(--accent);
  border-radius: var(--radius-lg);
  background: color-mix(in srgb, var(--accent) 10%, var(--bg-app));
  color: var(--fg);
  font-size: 1.1rem;
  font-weight: 600;
  pointer-events: none;
}
.state {
  flex: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--fg-dim);
  font-size: 1rem;
}
.pane-topbar {
  display: flex;
  min-width: 0;
  background: var(--bg-panel);
  font-size: var(--file-font-size);
}
.pane-topbar :deep(.pane-toolbar) {
  flex: none;
  border-bottom: 0;
  background: transparent;
}
.pane-topbar :deep(.path-bar) {
  flex: 1;
  min-width: 0;
  border-bottom: 0;
  background: transparent;
}
.search-heading {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 0 12px;
  color: var(--fg-muted);
  font-size: inherit;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.trash-heading {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 0 12px;
  color: var(--fg);
  font-size: inherit;
  overflow: hidden;
  white-space: nowrap;
}
.trash-title {
  min-width: 0;
  overflow: hidden;
  font-weight: 550;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.trash-actions {
  display: flex;
  align-items: center;
  gap: 2px;
  flex: none;
  margin-left: auto;
}
.trash-actions button {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 24px;
  padding: 0 8px;
  border-radius: var(--radius);
  color: var(--fg-muted);
  font-size: 0.875em;
  white-space: nowrap;
}
.trash-actions button:hover:not(:disabled) {
  background: var(--bg-hover);
  color: var(--fg);
}
.trash-actions button.danger:hover:not(:disabled) {
  background: color-mix(in srgb, var(--danger) 14%, transparent);
  color: var(--danger);
}
.trash-actions button:disabled {
  opacity: 0.38;
  cursor: default;
}
.terminal-heading {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 0 12px;
  color: var(--fg-muted);
  font-size: inherit;
  overflow: hidden;
  white-space: nowrap;
}
.terminal-host {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  font-family: 'Cascadia Mono', 'JetBrains Mono', Consolas, monospace;
  color: var(--fg);
}
.terminal-state {
  flex: none;
  padding: 1px 7px;
  border-radius: 999px;
  background: var(--bg-hover);
  color: var(--fg-dim);
  font-size: 0.8333rem;
}
.terminal-state.connected {
  color: #4aa552;
}
.terminal-state.error {
  color: var(--danger);
}
.terminal-fingerprint {
  flex: none;
  margin-left: auto;
  overflow: hidden;
  text-overflow: ellipsis;
  color: var(--fg-dim);
  font-size: 0.8rem;
}
.pane-terminal {
  flex: 1;
  min-height: 0;
}
.pane-topbar.is-active {
  background: var(--pane-title-bg-active);
}
.pane-topbar.is-active :deep(.pane-toolbar),
.pane-topbar.is-active :deep(.path-bar) {
  background: transparent;
}

.error-bar {
  position: absolute;
  left: 8px;
  right: 8px;
  bottom: 8px;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 10px;
  background: var(--danger-soft);
  border: 1px solid var(--danger);
  border-radius: var(--radius);
  font-size: 1rem;
  color: var(--fg);
}
.error-bar span {
  flex: 1;
  word-break: break-word;
}
.error-bar button {
  color: var(--fg-muted);
}

.mask {
  position: fixed;
  inset: 0;
  z-index: 300;
  background: var(--mask);
  display: flex;
  align-items: center;
  justify-content: center;
}
.dialog {
  width: 340px;
  max-width: 90vw;
  background: var(--bg-elevated);
  border: 1px solid var(--border-strong);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-lg);
  padding: 14px;
  display: flex;
  flex-direction: column;
  gap: 10px;
}
.dialog h3 {
  margin: 0;
  font-size: 1.0833rem;
  font-weight: 600;
  color: var(--fg);
}
.warn-text {
  margin: 0;
  font-size: 1rem;
  color: var(--fg-muted);
}
.warn-text.danger {
  color: var(--danger);
}
.del-list {
  margin: 0;
  padding-left: 18px;
  max-height: 140px;
  overflow: auto;
  font-size: 1rem;
  color: var(--fg);
}
.more {
  margin: 0;
  font-size: 0.9167rem;
  color: var(--fg-dim);
}
.actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}
.actions button {
  padding: 4px 14px;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  color: var(--fg-muted);
}
.actions button:hover:not(:disabled) {
  background: var(--bg-hover);
  color: var(--fg);
}
.actions button.mode-toggle {
  margin-right: auto;
  border-color: transparent;
  color: var(--fg-dim);
}
.actions button.mode-toggle:hover:not(:disabled) {
  color: var(--danger);
}
.actions button.danger {
  background: var(--danger);
  border-color: var(--danger);
  color: var(--accent-fg);
}
.actions button:disabled {
  opacity: 0.5;
  cursor: default;
}
</style>

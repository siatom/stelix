<script setup lang="ts">
// 单个窗格 = 标签栏 + 内嵌工具栏 + 面包屑 + 文件列表（虚拟滚动）（WS-D）
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { usePaneStore } from '../../stores/pane'
import { isCapturingShortcut } from '../../stores/shortcuts'
import { basename } from './utils'
import TabBar from './TabBar.vue'
import PaneToolbar from './PaneToolbar.vue'
import PathBar from './PathBar.vue'
import FileList from './FileList.vue'
import StatusBar from '../StatusBar.vue'
import Icon from '../icons/Icon.vue'

const props = defineProps<{ paneId: string }>()

const paneStore = usePaneStore()
const pane = computed(() => paneStore.panes.find((p) => p.id === props.paneId))
const isActivePane = computed(() => paneStore.activePaneId === props.paneId)
const tab = computed(() => {
  const p = pane.value
  if (!p) return undefined
  return p.tabs.find((t) => t.id === p.activeTabId) ?? p.tabs[0]
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
      if (paneStore.canDeleteInActivePane) openDelete([...t.selected])
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

function openDelete(paths: string[]) {
  errorMsg.value = null
  deleteDialog.value = [...paths]
}

function closeDialogs() {
  deleteDialog.value = null
}

async function confirmDelete() {
  const paths = deleteDialog.value
  if (!paths || !paths.length) return
  busy.value = true
  try {
    const res = await paneStore.runFs(props.paneId, { op: 'delete', sources: paths })
    if (!res.ok) errorMsg.value = res.errors.join('; ')
    deleteDialog.value = null
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <section class="pane" @mousedown="activate">
    <template v-if="tab">
      <div class="pane-topbar" :class="{ 'is-active': isActivePane }">
        <PaneToolbar :pane-id="paneId" />
        <div v-if="tab.kind === 'search'" class="search-heading" :title="tab.searchQuery">
          搜索结果：{{ tab.searchQuery }}
        </div>
        <PathBar v-else :pane-id="paneId" :tab="tab" />
      </div>
      <FileList
        :pane-id="paneId"
        :tab="tab"
        :create-request="createRequest"
        @delete="openDelete"
        @create-handled="onCreateHandled"
      />
    </template>
    <div v-else class="state">没有可用标签</div>
    <TabBar :pane-id="paneId" />
    <StatusBar :pane-id="paneId" />

    <div v-if="errorMsg" class="error-bar">
      <span>{{ errorMsg }}</span>
      <button @click="errorMsg = null"><Icon name="x" :size="13" /></button>
    </div>

    <!-- 删除确认 -->
    <div v-if="deleteDialog" class="mask" @mousedown.stop @click.self="closeDialogs">
      <div class="dialog">
        <h3>确认删除</h3>
        <p class="warn-text">将永久删除以下 {{ deleteDialog.length }} 项，此操作不可撤销：</p>
        <ul class="del-list">
          <li v-for="p in deleteDialog.slice(0, 8)" :key="p">{{ basename(p) }}</li>
        </ul>
        <p v-if="deleteDialog.length > 8" class="more">… 等共 {{ deleteDialog.length }} 项</p>
        <div class="actions">
          <button :disabled="busy" @click="closeDialogs">取消</button>
          <button class="danger" :disabled="busy" @click="confirmDelete">删除</button>
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
  background: var(--bg-app);
  /* 不加描边：与侧栏靠背景深浅区分，与对侧窗格靠 DualPane 的 1px 分割线区分 */
  position: relative;
  overflow: hidden;
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
}
.pane-topbar :deep(.pane-toolbar) {
  flex: none;
  border-bottom: 0;
}
.pane-topbar :deep(.path-bar) {
  flex: 1;
  min-width: 0;
  border-bottom: 0;
}
.search-heading {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: center;
  padding: 0 12px;
  color: var(--fg-muted);
  font-size: 1rem;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
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

<script setup lang="ts">
// 设置面板：索引根目录管理 + 索引统计 + 索引进度事件
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { api, events } from '../../api'
import { usePaneStore } from '../../stores/pane'
import { useUiStore } from '../../stores/ui'
import Icon from '../icons/Icon.vue'
import type { IndexProgress, IndexStatus } from '../../types'

const pane = usePaneStore()
const ui = useUiStore()
const appVersion = __APP_VERSION__
const commitId = __COMMIT_ID__

const newRoot = ref('')
const busy = ref(false)
const error = ref<string | null>(null)
const status = ref<IndexStatus | null>(null)
const autostart = ref(false)
const autostartBusy = ref(false)
let unlisteners: (() => void)[] = []

const scanning = computed(() => ui.scanning || (status.value?.scanning ?? false))
const total = computed(() => status.value?.total ?? ui.indexTotal)
const scanned = computed(() => status.value?.scanned ?? ui.scanned)

async function refreshStatus() {
  try {
    const s = await api.indexStatus()
    status.value = s
    ui.setIndexTotal(s.total)
    ui.setScanning(s.scanning)
    ui.setScanned(s.scanned)
    if (!ui.indexRoots.length && s.roots.length) ui.setIndexRoots(s.roots)
  } catch (e) {
    error.value = String(e)
  }
}

// 开机自启动：真实状态以后端注册表为准，挂载时读取、切换时写回。
async function refreshAutostart() {
  try {
    autostart.value = await api.autostartGet()
  } catch (e) {
    error.value = String(e)
  }
}

async function toggleAutostart(value: boolean) {
  autostartBusy.value = true
  error.value = null
  try {
    await api.autostartSet(value)
    autostart.value = value
    ui.setStatusText(value ? '已开启开机自启动' : '已关闭开机自启动')
  } catch (e) {
    error.value = String(e)
    ui.setStatusText(`设置开机自启动失败：${String(e)}`)
  } finally {
    autostartBusy.value = false
  }
}

async function applyRoots(roots: string[]) {
  ui.setIndexRoots(roots)
  busy.value = true
  error.value = null
  try {
    await api.startIndex(roots)
    ui.setStatusText(`已开始索引 ${roots.length} 个根目录`)
    await refreshStatus()
  } catch (e) {
    error.value = String(e)
    ui.setStatusText(`索引启动失败：${String(e)}`)
  } finally {
    busy.value = false
  }
}

function addRoot() {
  const path = newRoot.value.trim()
  if (!path || ui.indexRoots.includes(path)) return
  newRoot.value = ''
  void applyRoots([...ui.indexRoots, path])
}

function addCurrent() {
  const path = pane.activeTab?.path
  if (!path || ui.indexRoots.includes(path)) return
  void applyRoots([...ui.indexRoots, path])
}

function removeRoot(path: string) {
  void applyRoots(ui.indexRoots.filter((r) => r !== path))
}

function onProgress(p: unknown) {
  const prog = p as IndexProgress
  ui.setScanned(prog.scanned)
  if (typeof prog.total === 'number' && prog.total > 0) ui.setIndexTotal(prog.total)
  ui.setScanning(!prog.done)
  const suffix = prog.total ? ` / ${prog.total.toLocaleString()}` : ''
  ui.setStatusText(`正在索引：${prog.scanned.toLocaleString()}${suffix}`)
}

function onDone() {
  ui.setScanning(false)
  ui.setStatusText('索引完成')
  void refreshStatus()
}

onMounted(async () => {
  await Promise.all([refreshStatus(), refreshAutostart()])
  unlisteners = [
    await events.onIndexProgress(onProgress),
    await events.onIndexDone(onDone),
  ]
})

onUnmounted(() => {
  for (const fn of unlisteners) fn()
  unlisteners = []
})
</script>

<template>
  <div class="settings-panel">
    <div class="settings-scroll">
      <section class="sec">
        <h4 class="sec-head">索引状态</h4>
        <div class="stat-grid">
          <div class="stat">
            <span class="k">条目数</span>
            <span class="v">{{ total.toLocaleString() }}</span>
          </div>
          <div class="stat">
            <span class="k">已扫描</span>
            <span class="v">{{ scanned.toLocaleString() }}</span>
          </div>
          <div class="stat">
            <span class="k">状态</span>
            <span class="v" :class="{ busy: scanning }">{{ scanning ? '扫描中…' : '空闲' }}</span>
          </div>
        </div>
      </section>

      <section class="sec">
        <h4 class="sec-head">索引根目录</h4>

        <div class="root-list">
          <div v-if="!ui.indexRoots.length" class="state">尚未配置索引根目录</div>
          <div v-for="root in ui.indexRoots" :key="root" class="root-item" :title="root">
            <span class="root-path">{{ root }}</span>
            <button class="del" title="移除该根目录" :disabled="busy" @click="removeRoot(root)">
              <Icon name="x" :size="13" />
            </button>
          </div>
        </div>

        <div class="add-row">
          <input
            v-model="newRoot"
            class="root-input"
            placeholder="输入要索引的目录，如 D:\\Projects"
            @keydown.enter="addRoot"
          />
          <button class="add" :disabled="busy" @click="addRoot">添加</button>
        </div>
        <button class="add-current" :disabled="busy" @click="addCurrent">
          添加当前目录：{{ pane.activeTab?.path || '—' }}
        </button>
      </section>

      <section class="sec">
        <h4 class="sec-head">视图</h4>
        <label class="toggle">
          <input
            type="checkbox"
            :checked="ui.hideProtected"
            @change="ui.setHideProtected(($event.target as HTMLInputElement).checked)"
          />
          <span class="toggle-text">隐藏受保护的操作系统文件</span>
        </label>
        <label class="toggle">
          <input
            type="checkbox"
            :checked="ui.hideFileExtensions"
            @change="ui.setHideFileExtensions(($event.target as HTMLInputElement).checked)"
          />
          <span class="toggle-text">隐藏文件扩展名</span>
        </label>
      </section>

      <section class="sec">
        <h4 class="sec-head">启动</h4>
        <label class="toggle">
          <input
            type="checkbox"
            :checked="autostart"
            :disabled="autostartBusy"
            @change="toggleAutostart(($event.target as HTMLInputElement).checked)"
          />
          <span class="toggle-text">开机自启动（启动后静默在托盘）</span>
        </label>
      </section>

      <div v-if="error" class="error">操作失败：{{ error }}</div>
    </div>

    <div class="build-info">版本 {{ appVersion }} · Commit {{ commitId }}</div>
  </div>
</template>

<style scoped>
.settings-panel {
  display: flex;
  flex-direction: column;
  height: 100%;
}
/* 内容区滚动，版本信息固定在面板底部 */
.settings-scroll {
  flex: 1;
  min-height: 0;
  overflow: auto;
  display: flex;
  flex-direction: column;
  gap: 12px;
}
.sec {
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.sec-head {
  margin: 0;
  font-size: 11px;
  font-weight: 600;
  color: var(--fg-muted);
  letter-spacing: 0.04em;
}
.stat-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 4px;
}
.stat {
  display: flex;
  justify-content: space-between;
  padding: 4px 6px;
  background: var(--bg-elevated);
  border-radius: var(--radius);
  font-size: 11px;
}
.stat .k {
  color: var(--fg-dim);
}
.stat .v {
  color: var(--fg);
  font-family: var(--mono);
}
.stat .v.busy {
  color: var(--warn);
}
.root-list {
  display: flex;
  flex-direction: column;
  gap: 3px;
}
.root-item {
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 3px 6px;
  background: var(--bg-elevated);
  border-radius: var(--radius);
}
.root-path {
  flex: 1;
  font-size: 11px;
  font-family: var(--mono);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.del {
  flex: none;
  width: 16px;
  height: 16px;
  border-radius: var(--radius);
  color: var(--fg-dim);
}
.del:hover:not(:disabled) {
  background: var(--danger);
  color: var(--accent-fg);
}
.add-row {
  display: flex;
  gap: 4px;
}
.root-input {
  flex: 1;
  min-width: 0;
  font-size: 11px;
}
.add {
  flex: none;
  padding: 0 10px;
  border-radius: var(--radius);
  border: 1px solid var(--border-strong);
  color: var(--fg);
  font-size: 12px;
}
.add:hover:not(:disabled) {
  background: var(--bg-hover);
  border-color: var(--accent);
}
.add-current {
  width: 100%;
  padding: 4px 6px;
  border-radius: var(--radius);
  border: 1px dashed var(--border-strong);
  color: var(--fg-muted);
  font-size: 11px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.add-current:hover:not(:disabled) {
  background: var(--bg-hover);
  color: var(--fg);
}
.add:disabled,
.add-current:disabled,
.del:disabled {
  opacity: 0.45;
  cursor: default;
}
.state {
  font-size: 11px;
  color: var(--fg-dim);
}
.toggle {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 4px 6px;
  background: var(--bg-elevated);
  border-radius: var(--radius);
  cursor: pointer;
}
.toggle:hover {
  background: var(--bg-hover);
}
.toggle input {
  flex: none;
  margin: 0;
  accent-color: var(--accent);
}
.toggle-text {
  font-size: 11px;
  color: var(--fg);
}
.error {
  padding: 4px 6px;
  border-radius: var(--radius);
  background: var(--danger-soft);
  color: var(--danger);
  font-size: 11px;
  word-break: break-all;
}
.build-info {
  flex: none;
  margin-top: 8px;
  padding-top: 8px;
  border-top: 1px solid var(--border);
  color: var(--fg-dim);
  font-family: var(--mono);
  font-size: 10px;
  text-align: center;
}
</style>

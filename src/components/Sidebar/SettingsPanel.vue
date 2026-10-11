<script setup lang="ts">
// 设置面板：索引根目录管理 + 视图偏好 + 启动行为
import { computed, onMounted, ref } from 'vue'
import { api } from '../../api'
import { usePaneStore } from '../../stores/pane'
import { useUiStore } from '../../stores/ui'
import Icon from '../icons/Icon.vue'

const pane = usePaneStore()
const ui = useUiStore()
const appVersion = __APP_VERSION__
const commitId = __COMMIT_ID__

const newRoot = ref('')
const busy = ref(false)
const error = ref<string | null>(null)
const autostart = ref(false)
const autostartBusy = ref(false)

// 「添加当前目录」按钮上只展示末级目录名，完整路径放到 title
const currentDirPath = computed(() => pane.activeTab?.path ?? '')
const currentDirName = computed(() => {
  const p = currentDirPath.value.replace(/[\\/]+$/, '')
  if (!p) return ''
  const parts = p.split(/[\\/]/)
  return parts[parts.length - 1] || p
})

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
    await ui.refreshIndexStatus()
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

function restoreDefaults() {
  ui.resetViewDefaults()
  ui.setStatusText('已恢复默认设置')
}

onMounted(() => {
  void ui.refreshIndexStatus()
  void refreshAutostart()
})
</script>

<template>
  <div class="settings-panel">
    <div class="settings-scroll">
      <section class="card">
        <header class="card-head">索引根目录</header>
        <div class="card-body">
          <div v-if="!ui.indexRoots.length" class="state">尚未配置索引根目录</div>
          <div v-else class="root-list">
            <div v-for="root in ui.indexRoots" :key="root" class="root-item" :title="root">
              <Icon name="folder" :size="13" class="root-ico" />
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
              placeholder="输入要索引的目录"
              @keydown.enter="addRoot"
            />
            <button class="add" :disabled="busy" @click="addRoot">添加</button>
          </div>

          <button
            class="add-current"
            :disabled="busy"
            :title="currentDirPath || '当前目录不可用'"
            @click="addCurrent"
          >
            <Icon name="plus" :size="13" />
            <span>添加当前目录</span>
            <span v-if="currentDirName" class="ac-path">{{ currentDirName }}</span>
          </button>
        </div>
      </section>

      <section class="card">
        <header class="card-head">视图</header>
        <div class="card-body tight">
          <label class="row">
            <span class="row-label">界面字体大小</span>
            <select
              class="setting-select"
              :value="ui.fontSize"
              @change="ui.setFontSize(Number(($event.target as HTMLSelectElement).value))"
            >
              <option v-for="size in [11, 12, 13, 14, 15, 16]" :key="size" :value="size">{{ size }}px</option>
            </select>
          </label>
          <label class="row">
            <span class="row-label">侧边栏行高</span>
            <select
              class="setting-select"
              :value="ui.sidebarRowHeight"
              @change="ui.setSidebarRowHeight(Number(($event.target as HTMLSelectElement).value))"
            >
              <option v-for="size in [20, 22, 24, 26, 28, 30, 32]" :key="size" :value="size">{{ size }}px</option>
            </select>
          </label>
          <label class="row">
            <span class="row-label">文件列表行高</span>
            <select
              class="setting-select"
              :value="ui.fileRowHeight"
              @change="ui.setFileRowHeight(Number(($event.target as HTMLSelectElement).value))"
            >
              <option v-for="size in [22, 24, 26, 28, 30, 32, 34, 36, 40]" :key="size" :value="size">{{ size }}px</option>
            </select>
          </label>
          <label class="row">
            <span class="row-label">隐藏受保护的操作系统文件</span>
            <input
              type="checkbox"
              :checked="ui.hideProtected"
              @change="ui.setHideProtected(($event.target as HTMLInputElement).checked)"
            />
          </label>
          <label class="row">
            <span class="row-label">隐藏文件扩展名</span>
            <input
              type="checkbox"
              :checked="ui.hideFileExtensions"
              @change="ui.setHideFileExtensions(($event.target as HTMLInputElement).checked)"
            />
          </label>
        </div>
      </section>

      <section class="card">
        <header class="card-head">启动</header>
        <div class="card-body tight">
          <label class="row">
            <span class="row-label">开机自启动</span>
            <input
              type="checkbox"
              :checked="autostart"
              :disabled="autostartBusy"
              @change="toggleAutostart(($event.target as HTMLInputElement).checked)"
            />
          </label>
          <p class="row-hint">启动后静默驻留系统托盘</p>
        </div>
      </section>

      <div v-if="error" class="error">操作失败：{{ error }}</div>
    </div>

    <div class="panel-footer">
      <button class="restore-btn" type="button" :disabled="busy" @click="restoreDefaults">
        <Icon name="refresh" :size="13" />
        <span>恢复默认</span>
      </button>
      <div class="build-info">版本 {{ appVersion }} · Commit {{ commitId }}</div>
    </div>
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
  gap: 10px;
}

/* ---- 分组卡片 ---- */
.card {
  background: var(--bg-elevated);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  overflow: hidden;
}
.card-head {
  padding: 7px 11px;
  font-size: 0.9167rem;
  font-weight: 600;
  color: var(--fg-muted);
  border-bottom: 1px solid var(--border);
}
.card-body {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 8px;
}
.card-body.tight {
  gap: 2px;
  padding: 6px;
}

/* ---- 索引根目录 ---- */
.root-list {
  display: flex;
  flex-direction: column;
  gap: 2px;
}
.root-item {
  display: flex;
  align-items: center;
  gap: 7px;
  padding: 5px 7px;
  border-radius: var(--radius-sm);
}
.root-item:hover {
  background: var(--bg-hover);
}
.root-ico {
  color: var(--ico-folder);
}
.root-path {
  flex: 1;
  min-width: 0;
  font-size: 0.9167rem;
  font-family: var(--mono);
  color: var(--fg);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.del {
  flex: none;
  display: flex;
  align-items: center;
  justify-content: center;
  width: 18px;
  height: 18px;
  border-radius: var(--radius-sm);
  color: var(--fg-dim);
}
.del:hover:not(:disabled) {
  background: var(--danger);
  color: var(--accent-fg);
}
.add-row {
  display: flex;
  gap: 6px;
}
.root-input {
  flex: 1;
  min-width: 0;
  font-size: 0.9167rem;
}
.add {
  flex: none;
  padding: 0 12px;
  border-radius: var(--radius-sm);
  border: 1px solid var(--border-strong);
  color: var(--fg);
  font-size: 1rem;
}
.add:hover:not(:disabled) {
  background: var(--bg-hover);
  border-color: var(--accent);
}
.add-current {
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  height: 28px;
  padding: 0 9px;
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  color: var(--fg-muted);
  font-size: 0.9167rem;
}
.add-current:hover:not(:disabled) {
  background: var(--bg-hover);
  color: var(--fg);
  border-color: var(--border-strong);
}
.add-current span:first-of-type {
  flex: none;
}
.ac-path {
  margin-left: auto;
  min-width: 0;
  max-width: 55%;
  font-family: var(--mono);
  color: var(--fg-dim);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.add:disabled,
.add-current:disabled,
.del:disabled {
  opacity: 0.45;
  cursor: default;
}
.state {
  padding: 2px;
  font-size: 0.9167rem;
  color: var(--fg-dim);
}

/* ---- 开关行 ---- */
.row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 7px 8px;
  border-radius: var(--radius-sm);
  cursor: pointer;
}
.row:hover {
  background: var(--bg-hover);
}
.row-label {
  font-size: 0.9167rem;
  color: var(--fg);
}
.row input {
  flex: none;
  margin: 0;
  accent-color: var(--accent);
}
.setting-select {
  flex: none;
  padding: 3px 6px;
  border: 1px solid var(--border-strong);
  border-radius: var(--radius-sm);
  background: var(--bg-input);
  color: var(--fg);
  font: inherit;
  cursor: pointer;
}
.row-hint {
  margin: 0;
  padding: 0 8px 4px;
  font-size: 0.8333rem;
  color: var(--fg-dim);
}

.error {
  padding: 6px 10px;
  border-radius: var(--radius);
  background: var(--danger-soft);
  color: var(--danger);
  font-size: 0.9167rem;
  word-break: break-all;
}
.panel-footer {
  flex: none;
  margin-top: 8px;
  padding-top: 8px;
  border-top: 1px solid var(--border);
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.restore-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  height: 30px;
  border: 1px solid var(--border-strong);
  border-radius: var(--radius-sm);
  color: var(--fg-muted);
  font-size: 0.9167rem;
}
.restore-btn:hover:not(:disabled) {
  background: var(--bg-hover);
  color: var(--fg);
  border-color: var(--accent);
}
.restore-btn:disabled {
  opacity: 0.45;
  cursor: default;
}
.build-info {
  color: var(--fg-dim);
  font-family: var(--mono);
  font-size: 0.8333rem;
  text-align: center;
}
</style>

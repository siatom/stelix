<script setup lang="ts">
// 左侧标签设置页：常规、文件索引、AI 大模型和关于。
import { computed, onMounted, ref } from 'vue'
import { api } from '../../api'
import { usePaneStore } from '../../stores/pane'
import { useUiStore } from '../../stores/ui'
import Icon from '../icons/Icon.vue'
import AiSettings from './AiSettings.vue'
import ShortcutsSettings from './ShortcutsSettings.vue'

const pane = usePaneStore()
const ui = useUiStore()
const appVersion = __APP_VERSION__
const commitId = __COMMIT_ID__
const tabs = [
  { id: 'general', label: '常规', icon: 'settings' },
  { id: 'index', label: '文件索引', icon: 'folder' },
  { id: 'shortcuts', label: '快捷键', icon: 'command' },
  { id: 'ai', label: 'AI 大模型', icon: 'sparkles' },
  { id: 'about', label: '关于', icon: 'info' },
] as const

function onTabKey(event: KeyboardEvent, index: number) {
  let next = index
  if (event.key === 'ArrowDown') next = (index + 1) % tabs.length
  else if (event.key === 'ArrowUp') next = (index + tabs.length - 1) % tabs.length
  else if (event.key === 'Home') next = 0
  else if (event.key === 'End') next = tabs.length - 1
  else return
  event.preventDefault()
  ui.settingsTab = tabs[next]!.id
  const list = (event.currentTarget as HTMLElement).parentElement
  ;(list?.querySelectorAll<HTMLButtonElement>('[role="tab"]')[next])?.focus()
}

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
    <nav class="settings-tabs" role="tablist" aria-label="设置分类" aria-orientation="vertical">
      <button v-for="(tab, index) in tabs" :id="`settings-tab-${tab.id}`" :key="tab.id" role="tab" :aria-selected="ui.settingsTab === tab.id" aria-controls="settings-content" :tabindex="ui.settingsTab === tab.id ? 0 : -1" :class="{ active: ui.settingsTab === tab.id }" @click="ui.settingsTab = tab.id" @keydown="onTabKey($event, index)"><Icon :name="tab.icon" :size="16" /><span>{{ tab.label }}</span></button>
    </nav>
    <div id="settings-content" class="settings-content" role="tabpanel" :aria-labelledby="`settings-tab-${ui.settingsTab}`">
    <div class="settings-scroll">
      <h2 class="page-title">{{ tabs.find(tab => tab.id === ui.settingsTab)?.label }}</h2>
      <AiSettings v-show="ui.settingsTab === 'ai'" />
      <ShortcutsSettings v-show="ui.settingsTab === 'shortcuts'" />
      <section v-show="ui.settingsTab === 'index'" class="card">
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

      <section v-show="ui.settingsTab === 'general'" class="card">
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

      <section v-show="ui.settingsTab === 'general'" class="card">
        <header class="card-head">视图</header>
        <div class="card-body tight">
          <label class="row">
            <span class="row-label">侧边栏字体大小</span>
            <select class="setting-select" :value="String(ui.sidebarFontSize || 12)" @change="ui.setSidebarFontSize(Number(($event.target as HTMLSelectElement).value))">
              <option v-for="size in [11, 12, 13, 14, 15, 16]" :key="size" :value="String(size)">{{ size }}px</option>
            </select>
          </label>
          <label class="row">
            <span class="row-label">文件列表字体大小</span>
            <select class="setting-select" :value="String(ui.fileFontSize || 12)" @change="ui.setFileFontSize(Number(($event.target as HTMLSelectElement).value))">
              <option v-for="size in [11, 12, 13, 14, 15, 16]" :key="size" :value="String(size)">{{ size }}px</option>
            </select>
          </label>
          <label class="row">
            <span class="row-label">侧边栏行高</span>
            <select class="setting-select" :value="ui.sidebarRowHeight" @change="ui.setSidebarRowHeight(Number(($event.target as HTMLSelectElement).value))">
              <option v-for="size in [20, 22, 24, 26, 28, 30, 32]" :key="size" :value="size">{{ size }}px</option>
            </select>
          </label>
          <label class="row">
            <span class="row-label">文件列表行高</span>
            <select class="setting-select" :value="ui.fileRowHeight" @change="ui.setFileRowHeight(Number(($event.target as HTMLSelectElement).value))">
              <option v-for="size in [22, 24, 26, 28, 30, 32, 34, 36, 40]" :key="size" :value="size">{{ size }}px</option>
            </select>
          </label>
          <label class="row">
            <span class="row-label">隐藏受保护的操作系统文件</span>
            <input type="checkbox" :checked="ui.hideProtected" @change="ui.setHideProtected(($event.target as HTMLInputElement).checked)" />
          </label>
          <label class="row">
            <span class="row-label">隐藏文件扩展名</span>
            <input type="checkbox" :checked="ui.hideFileExtensions" @change="ui.setHideFileExtensions(($event.target as HTMLInputElement).checked)" />
          </label>
        </div>
      </section>

      <section v-show="ui.settingsTab === 'about'" class="card">
        <header class="card-head">Stelix File Manager 星索</header>
        <div class="card-body"><p class="about-text">版本 {{ appVersion }}</p><p class="about-text">Commit {{ commitId }}</p></div>
      </section>
      <div v-if="error && (ui.settingsTab === 'general' || ui.settingsTab === 'index')" class="error">操作失败：{{ error }}</div>
    </div>

    <div v-if="ui.settingsTab === 'general'" class="panel-footer">
      <button class="restore-btn" type="button" :disabled="busy" @click="restoreDefaults">
        <Icon name="refresh" :size="13" />
        <span>恢复默认</span>
      </button>
    </div>
    </div>
  </div>
</template>

<style scoped>
.settings-panel {
  display: grid;
  grid-template-columns: 154px minmax(0, 1fr);
  height: 100%;
  min-height: 0;
  font-size: max(12px, var(--ui-font-size));
  background: var(--bg-app);
}
.settings-tabs {
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding: 18px 12px;
  border-right: 1px solid var(--border);
  background: var(--bg-panel);
}
.settings-tabs button {
  display: flex;
  align-items: center;
  gap: 9px;
  min-height: 38px;
  padding: 9px 10px;
  text-align: left;
  border-radius: var(--radius-sm);
  color: var(--fg-muted);
}
.settings-tabs button:hover { background: var(--bg-hover); color: var(--fg); }
.settings-tabs button.active { background: var(--bg-active); color: var(--fg); font-weight: 600; }
.settings-content { display: flex; flex-direction: column; min-width: 0; min-height: 0; padding: 20px; background: var(--bg-app); }
.page-title { margin: 0 0 6px; font-size: 1.3333rem; font-weight: 600; }
.about-text { margin: 4px; color: var(--fg-muted); }
@media (max-width: 580px) {
  .settings-panel { grid-template-columns: 110px minmax(0, 1fr); }
  .settings-tabs { padding: 12px 6px; }
  .settings-tabs button { gap: 6px; padding: 8px 6px; }
  .settings-content { padding: 12px; }
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
.settings-scroll > * { flex-shrink: 0; }

/* ---- 分组卡片 ---- */
.card {
  background: var(--bg-elevated);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  overflow: hidden;
  box-shadow: var(--shadow-sm);
}
.card-head {
  padding: 10px 12px;
  font-size: 1rem;
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
  font-size: 1rem;
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
  font-size: 1rem;
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
  height: 30px;
  padding: 0 9px;
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  color: var(--fg-muted);
  font-size: 1rem;
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
  font-size: 1rem;
  color: var(--fg-dim);
}

/* ---- 开关行 ---- */
.row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 7px 8px;
  height: 32px;
  box-sizing: border-box;
  border-radius: var(--radius-sm);
  cursor: pointer;
}
.row:hover {
  background: var(--bg-hover);
}
.row-label {
  font-size: 1rem;
  color: var(--fg);
}
.row input {
  flex: none;
  margin: 0;
  accent-color: var(--accent);
}
.setting-select {
  flex: none;
  min-width: 78px;
  height: 28px;
  padding: 2px 6px;
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
  font-size: 1rem;
  color: var(--fg-dim);
}

.error {
  padding: 6px 10px;
  border-radius: var(--radius);
  background: var(--danger-soft);
  color: var(--danger);
  font-size: 1rem;
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
  font-size: 1rem;
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
</style>

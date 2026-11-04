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
const accentColors = [
  { name: '星空蓝', value: '#6d8cff' },
  { name: '海盐青', value: '#20a99a' },
  { name: '薄荷绿', value: '#39a96b' },
  { name: '琥珀橙', value: '#db8b2b' },
  { name: '莓果红', value: '#d95770' },
  { name: '薰衣紫', value: '#9270d8' },
  { name: '玫瑰粉', value: '#d66caf' },
  { name: '石墨灰', value: '#737b89' },
]
const tabs = [
  { id: 'general', label: '常规', icon: 'settings' },
  { id: 'appearance', label: '外观', icon: 'image' },
  { id: 'index', label: '文件索引', icon: 'folder' },
  { id: 'shortcuts', label: '快捷键', icon: 'command' },
  { id: 'ai', label: 'AI 大模型', icon: 'sparkles' },
  { id: 'about', label: '关于', icon: 'info' },
] as const

async function onBackgroundImageChange(event: Event) {
  const input = event.target as HTMLInputElement
  const file = input.files?.[0]
  input.value = ''
  if (!file) return
  if (!['image/png', 'image/jpeg', 'image/webp', 'image/gif'].includes(file.type) || file.size > 8 * 1024 * 1024) {
    error.value = '请选择 PNG、JPG、WebP 或 GIF 图片，且文件不超过 8 MB。'
    return
  }
  const reader = new FileReader()
  try {
    const dataUrl = await new Promise<string>((resolve, reject) => {
      reader.onload = () => typeof reader.result === 'string' ? resolve(reader.result) : reject(new Error('无法读取图片数据'))
      reader.onerror = () => reject(reader.error ?? new Error('读取背景图片失败'))
      reader.readAsDataURL(file)
    })
    if (!await ui.setSettingsBackgroundImage(dataUrl)) throw new Error('保存图片失败，请重试')
    error.value = null
  } catch (cause) {
    error.value = String(cause)
  }
}

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
    await api.setIndexRoots(roots)
    ui.setStatusText(`已保存 ${roots.length} 个备用索引根目录`)
  } catch (e) {
    error.value = String(e)
    ui.setStatusText(`保存索引根目录失败：${String(e)}`)
  } finally {
    busy.value = false
  }
}

async function retryEverything() {
  busy.value = true
  error.value = null
  try {
    await api.retryEverythingIndex()
    ui.setStatusText('正在重新连接 Everything…')
    await ui.refreshIndexStatus()
  } catch (e) {
    error.value = String(e)
    ui.setStatusText(`连接 Everything 失败：${String(e)}`)
  } finally {
    busy.value = false
  }
}

async function enableLocalIndex() {
  busy.value = true
  error.value = null
  try {
    await api.startIndex(ui.indexRoots)
    ui.setStatusText(`已开始索引 ${ui.indexRoots.length} 个根目录`)
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
        <header class="card-head">备用内存索引根目录</header>
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

          <p class="row-hint">
            <template v-if="ui.usingEverything">正在使用本机 Everything 的实时索引。</template>
            <template v-else-if="ui.waitingForEverything">正在等待 Everything 启动；Stelix 不会扫描本地目录。</template>
            <template v-else>Everything 未运行。可重试连接，或手动启用备用内存索引。</template>
          </p>
          <div v-if="!ui.usingEverything && !ui.scanning" class="index-actions">
            <button class="add-current" :disabled="busy" @click="retryEverything">
              <Icon name="refresh" :size="13" />
              <span>重试连接 Everything</span>
            </button>
            <button class="add-current" :disabled="busy" @click="enableLocalIndex">
              <Icon name="folder" :size="13" />
              <span>启用本地内存索引</span>
            </button>
          </div>
        </div>
      </section>

      <div v-show="ui.settingsTab === 'appearance'" class="appearance-layout">
        <section class="appearance-section">
          <div class="section-heading"><div><h3>主题色</h3><p>用于按钮、选中状态和交互提示</p></div><span class="appearance-current"><i :style="{ background: ui.accentColor }"></i>{{ accentColors.find(color => color.value === ui.accentColor)?.name || '自定义' }}</span></div>
          <div class="color-swatches" role="group" aria-label="预设主题色">
            <button v-for="color in accentColors" :key="color.value" type="button" class="color-swatch" :class="{ selected: ui.accentColor === color.value }" :style="{ '--swatch': color.value }" :aria-label="color.name" :aria-pressed="ui.accentColor === color.value" :title="color.name" @click="ui.setAccentColor(color.value)"><span>{{ color.name }}</span></button>
            <label class="color-swatch custom-swatch" :style="{ '--swatch': ui.accentColor }" title="自定义颜色" :class="{ selected: !accentColors.some(color => color.value === ui.accentColor) }"><Icon name="plus" :size="16" /><input type="color" :value="ui.accentColor" aria-label="自定义主题色" @input="ui.setAccentColor(($event.target as HTMLInputElement).value)" /></label>
          </div>
        </section>

        <section class="appearance-section">
          <div class="section-heading"><div><h3>字体与行高</h3><p>调整列表的阅读舒适度和信息密度</p></div></div>
          <div class="font-grid">
            <label class="font-row">
              <span><strong>侧边栏字体</strong><small>目录树与导航面板</small></span>
              <select class="setting-select" :value="String(ui.sidebarFontSize || 12)" @change="ui.setSidebarFontSize(Number(($event.target as HTMLSelectElement).value))">
                <option v-for="size in [11, 12, 13, 14, 15, 16]" :key="size" :value="String(size)">{{ size }}px</option>
              </select>
            </label>
            <label class="font-row">
              <span><strong>文件列表字体</strong><small>文件与文件夹名称</small></span>
              <select class="setting-select" :value="String(ui.fileFontSize || 12)" @change="ui.setFileFontSize(Number(($event.target as HTMLSelectElement).value))">
                <option v-for="size in [11, 12, 13, 14, 15, 16]" :key="size" :value="String(size)">{{ size }}px</option>
              </select>
            </label>
            <label class="font-row">
              <span><strong>侧边栏行高</strong><small>目录树与导航列表</small></span>
              <select class="setting-select" :value="ui.sidebarRowHeight" @change="ui.setSidebarRowHeight(Number(($event.target as HTMLSelectElement).value))">
                <option v-for="size in [20, 22, 24, 26, 28, 30, 32]" :key="size" :value="size">{{ size }}px</option>
              </select>
            </label>
            <label class="font-row">
              <span><strong>文件列表行高</strong><small>文件与文件夹条目</small></span>
              <select class="setting-select" :value="ui.fileRowHeight" @change="ui.setFileRowHeight(Number(($event.target as HTMLSelectElement).value))">
                <option v-for="size in [22, 24, 26, 28, 30, 32, 34, 36, 40]" :key="size" :value="size">{{ size }}px</option>
              </select>
            </label>
          </div>
        </section>

        <section class="appearance-section opacity-section">
          <div class="section-heading"><div><h3>界面透明度</h3><p>调整设置窗口表面的通透程度</p></div><strong class="opacity-value">{{ ui.uiOpacity }}%</strong></div>
          <div class="opacity-control"><span>通透</span><input class="opacity-range" type="range" min="60" max="100" step="5" :value="ui.uiOpacity" aria-label="界面透明度" @input="ui.setUiOpacity(Number(($event.target as HTMLInputElement).value))" /><span>实色</span></div>
        </section>

        <section class="appearance-section background-section">
          <div class="section-heading"><div><h3>背景图片</h3><p>铺满整个界面；调低界面透明度即可透出图片</p></div></div>
          <div class="background-body">
            <div class="background-preview" :class="{ empty: !ui.settingsBackgroundImage }" :style="ui.settingsBackgroundImage ? { backgroundImage: `url(${ui.settingsBackgroundImage})` } : undefined"><Icon v-if="!ui.settingsBackgroundImage" name="image" :size="22" /><span v-if="!ui.settingsBackgroundImage">未设置</span></div>
            <div class="background-info">
              <span class="background-format">PNG、JPG、WebP 或 GIF · 最大 8 MB</span>
              <div class="background-actions">
                <label class="background-button">{{ ui.settingsBackgroundImage ? '更换图片' : '选择图片' }}<input type="file" accept="image/png,image/jpeg,image/webp,image/gif" @change="onBackgroundImageChange" /></label>
                <button v-if="ui.settingsBackgroundImage" class="background-button secondary" type="button" @click="ui.setSettingsBackgroundImage('')">移除</button>
              </div>
            </div>
          </div>
        </section>
      </div>

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
            <span class="row-label">显示隐藏文件</span>
            <input type="checkbox" :checked="ui.showHidden" @change="ui.setShowHidden(($event.target as HTMLInputElement).checked)" />
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
      <div v-if="error && (ui.settingsTab === 'general' || ui.settingsTab === 'appearance' || ui.settingsTab === 'index')" class="error">操作失败：{{ error }}</div>
    </div>

    <div v-if="ui.settingsTab === 'general' || ui.settingsTab === 'appearance'" class="panel-footer">
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
  position: relative;
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
.settings-content { position: relative; display: flex; flex-direction: column; min-width: 0; min-height: 0; padding: 20px; overflow: hidden; background: var(--bg-app); }
.settings-scroll { position: relative; z-index: 1; }
.page-title { margin: 0 0 6px; font-size: 1.3333rem; font-weight: 600; }
.about-text { margin: 4px; color: var(--fg-muted); }
.appearance-layout { display: flex; flex-direction: column; gap: 0; }
.appearance-section { padding: 15px 2px; border-bottom: 1px solid var(--border); }
.appearance-section:first-child { padding-top: 6px; }
.appearance-section:last-child { border-bottom: 0; }
.section-heading { display: flex; align-items: center; justify-content: space-between; gap: 16px; }
.section-heading h3 { margin: 0; font-size: 1rem; font-weight: 600; }
.section-heading p { margin: 3px 0 0; color: var(--fg-dim); font-size: 0.9167rem; }
.appearance-current { display: inline-flex; align-items: center; gap: 7px; color: var(--fg); font-size: 0.9167rem; font-weight: 500; }
.appearance-current i { width: 12px; height: 12px; border-radius: 50%; box-shadow: 0 0 0 2px color-mix(in srgb, var(--bg-elevated) 80%, transparent), 0 0 0 3px var(--accent); }
.color-swatches { display: flex; flex-wrap: wrap; gap: 10px; padding: 12px 2px 2px; }
.color-swatch { position: relative; display: flex; align-items: center; justify-content: center; flex: none; width: 34px; height: 34px; padding: 0; border: 2px solid transparent; border-radius: 50%; background: var(--swatch); color: white; cursor: pointer; transition: transform var(--t-fast), box-shadow var(--t-fast); }
.color-swatch:hover { transform: translateY(-1px) scale(1.06); }
.color-swatch.selected { border-color: var(--bg-elevated); box-shadow: 0 0 0 2px var(--swatch); }
.color-swatch span { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); white-space: nowrap; }
.custom-swatch { border: 1px dashed color-mix(in srgb, var(--fg-muted) 60%, transparent); background: var(--bg-hover); color: var(--fg-muted); overflow: hidden; }
.custom-swatch input { position: absolute; inset: 0; width: 100%; height: 100%; padding: 0; opacity: 0; cursor: pointer; }
.font-grid { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); column-gap: 24px; padding-top: 8px; }
.font-row { display: flex; align-items: center; justify-content: space-between; gap: 10px; min-height: 48px; padding: 5px 2px; border-bottom: 1px solid var(--border); }
.font-row span { display: flex; flex-direction: column; gap: 3px; min-width: 0; }
.font-row strong { font-weight: 550; }
.font-row small { color: var(--fg-dim); font-size: 0.8333rem; }
.opacity-value { color: var(--accent); font-size: 1.15rem; font-weight: 650; }
.opacity-control { display: flex; align-items: center; gap: 12px; padding-top: 10px; color: var(--fg-dim); font-size: 0.8333rem; }
.opacity-range { flex: 1; height: 18px; margin: 0; padding: 0; border: 0; border-radius: 0; background: transparent; accent-color: var(--accent); box-shadow: none; }
.opacity-range:focus { border: 0; box-shadow: none; }
.background-body { display: flex; align-items: center; gap: 14px; padding-top: 10px; }
.background-preview { display: flex; align-items: center; justify-content: center; flex-direction: column; flex: none; gap: 5px; width: 112px; height: 68px; border: 1px solid var(--border); border-radius: var(--radius-sm); background-position: center; background-size: cover; color: var(--fg-dim); font-size: 0.8333rem; overflow: hidden; }
.background-preview.empty { background: linear-gradient(135deg, var(--bg-hover), var(--bg-panel)); border-style: dashed; }
.background-info { display: flex; min-width: 0; flex-direction: column; align-items: flex-start; gap: 8px; }
.background-format { color: var(--fg-dim); font-size: 0.9167rem; }
.background-actions { display: flex; align-items: center; gap: 8px; }
.background-button { display: inline-flex; align-items: center; padding: 7px 11px; border: 1px solid var(--accent); border-radius: var(--radius-sm); background: var(--accent); color: var(--accent-fg); font: inherit; font-weight: 600; cursor: pointer; }
.background-button:hover { filter: brightness(0.94); }
.background-button.secondary { border-color: var(--border); background: transparent; color: var(--fg-muted); font-weight: 500; }
.background-button.secondary:hover { border-color: var(--danger); color: var(--danger); }
.background-button input { display: none; }
@media (max-width: 580px) {
  .settings-panel { grid-template-columns: 110px minmax(0, 1fr); }
  .settings-tabs { padding: 12px 6px; }
  .settings-tabs button { gap: 6px; padding: 8px 6px; }
  .settings-content { padding: 12px; }
}
@media (max-width: 760px) {
  .font-grid { grid-template-columns: 1fr; }
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
.index-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
}
.index-actions .add-current {
  flex: 1 1 180px;
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

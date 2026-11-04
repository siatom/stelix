<script setup lang="ts">
// 面包屑地址栏：点击各级跳转，右侧切换到可编辑输入框（WS-D）
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import type { TabState } from '../../types'
import { usePaneStore } from '../../stores/pane'
import { dirname, joinPath } from './utils'
import Icon from '../icons/Icon.vue'
import { addFavorite, FAVORITES_CHANGED_EVENT, loadFavorites } from '../Sidebar/shared'
import { useUiStore } from '../../stores/ui'
import { api } from '../../api'
import { normalizeRemotePath } from '../../shell/remote'

const props = defineProps<{ paneId: string; tab: TabState }>()
const paneStore = usePaneStore()
const ui = useUiStore()
const isFavorite = ref(loadFavorites().some((item) => item.path === props.tab.path))

/** 远程（FTP / SFTP）标签：路径只读展示，不参与本地收藏与路径补全。 */
const isRemote = computed(() => props.tab.kind === 'remote' && !!props.tab.remote)

const editing = ref(false)
const draft = ref('')
const inputRef = ref<HTMLInputElement | null>(null)

const crumbsEl = ref<HTMLElement | null>(null)
const mirrorEl = ref<HTMLElement | null>(null)
/** 折叠进省略号的前导层级数量：路径过长时优先保留深层目录，叶子目录始终可见 */
const hiddenCount = ref(0)
/** 面包屑窄到连省略号都放不下时，只留叶子目录 */
const ellipsisShown = ref(true)
const hiddenOpen = ref(false)
const ellipsisEl = ref<HTMLElement | null>(null)
const hiddenMenuEl = ref<HTMLElement | null>(null)

const segments = computed(() => {
  const location = props.tab.remote
  if (props.tab.kind === 'remote' && location) {
    const parts: { label: string; path: string }[] = [
      { label: `${location.protocol}://${location.host}:${location.port}`, path: '/' },
    ]
    let acc = ''
    for (const seg of normalizeRemotePath(props.tab.path).split('/').filter(Boolean)) {
      acc = `${acc}/${seg}`
      parts.push({ label: seg, path: acc })
    }
    return parts
  }
  const norm = props.tab.path.replace(/\//g, '\\')
  const share = norm.match(/^(\\\\[^\\]+\\[^\\]+)/)
  if (share) {
    const parts: { label: string; path: string }[] = [{ label: share[1].slice(2), path: share[1] }]
    let acc = share[1]
    for (const seg of norm.slice(share[1].length).split('\\').filter(Boolean)) {
      acc = joinPath(acc, seg)
      parts.push({ label: seg, path: acc })
    }
    return parts
  }
  const drive = norm.match(/^([A-Za-z]:)\\?/)
  const parts: { label: string; path: string }[] = []
  let acc = ''
  if (drive) {
    acc = `${drive[1]}\\`
    parts.push({ label: drive[1], path: acc })
  }
  const rest = norm.slice(drive ? drive[0].length : 0)
  for (const seg of rest.split('\\').filter(Boolean)) {
    acc = acc ? joinPath(acc, seg) : seg
    parts.push({ label: seg, path: acc })
  }
  return parts
})

const visibleFrom = computed(() => Math.min(hiddenCount.value, Math.max(0, segments.value.length - 1)))
const visibleSegments = computed(() => segments.value.slice(visibleFrom.value))
const hiddenSegments = computed(() => segments.value.slice(0, visibleFrom.value))

/** 依据可用宽度决定保留尾部几级：放不下时从头部折叠，绝不牺牲叶子目录。 */
function relayout() {
  const box = crumbsEl.value
  const mirror = mirrorEl.value
  if (!box || !mirror) return
  const total = segments.value.length
  if (!total) {
    hiddenCount.value = 0
    return
  }
  const avail = box.clientWidth - 1
  if (avail <= 0) return
  const mirrorCrumbs = Array.from(mirror.querySelectorAll<HTMLElement>('.crumb'))
  if (mirrorCrumbs.length !== total + 1) return // 首项是省略号占位
  const widths = mirrorCrumbs.map((el) => el.getBoundingClientRect().width)
  const ellipsisWidth = widths[0]
  const crumbWidths = widths.slice(1)
  const sepNode = mirror.querySelector<HTMLElement>('.sep')
  const sepWidth = sepNode ? sepNode.getBoundingClientRect().width : 0
  const gap = Number.parseFloat(getComputedStyle(box).columnGap) || 0
  const tail = (from: number) => crumbWidths.slice(from).reduce((sum, w) => sum + w, 0)

  if (tail(0) + sepWidth * (total - 1) + gap * (2 * total - 2) <= avail) {
    hiddenCount.value = 0
    ellipsisShown.value = true
    return
  }
  for (let drop = 1; drop < total; drop += 1) {
    const items = 1 + 2 * (total - drop)
    if (ellipsisWidth + tail(drop) + sepWidth * (total - drop) + gap * (items - 1) <= avail) {
      hiddenCount.value = drop
      ellipsisShown.value = true
      return
    }
  }
  // 极端窄：省略号也放不下，只保留叶子目录
  hiddenCount.value = total - 1
  ellipsisShown.value = false
}

let resizeObserver: ResizeObserver | null = null

function observeCrumbs() {
  resizeObserver?.disconnect()
  if (crumbsEl.value) resizeObserver?.observe(crumbsEl.value)
}

function toggleHidden() {
  if (!hiddenSegments.value.length) return
  hiddenOpen.value = !hiddenOpen.value
}

function pickHidden(path: string) {
  hiddenOpen.value = false
  go(path)
}

function onDocumentPointerDown(event: PointerEvent) {
  if (!hiddenOpen.value) return
  const target = event.target as Node | null
  if (!target) return
  if (hiddenMenuEl.value?.contains(target) || ellipsisEl.value?.contains(target)) return
  hiddenOpen.value = false
}

function onDocumentKeyDown(event: KeyboardEvent) {
  if (event.key !== 'Escape' || !hiddenOpen.value) return
  hiddenOpen.value = false
  event.stopPropagation()
}

function go(path: string) {
  const location = props.tab.remote
  if (props.tab.kind === 'remote' && location) {
    void paneStore.navigateRemote(props.paneId, location, path)
    return
  }
  void paneStore.navigateTo(props.paneId, path)
}

function syncFavorite() {
  isFavorite.value = loadFavorites().some((item) => item.path === props.tab.path)
}

function addCurrentFavorite() {
  if (isFavorite.value) return
  const added = addFavorite(props.tab.path)
  isFavorite.value = true
  ui.setStatusText(added ? '已加入收藏' : '该目录已在收藏中')
}

onMounted(async () => {
  window.addEventListener(FAVORITES_CHANGED_EVENT, syncFavorite)
  document.addEventListener('pointerdown', onDocumentPointerDown, true)
  document.addEventListener('keydown', onDocumentKeyDown, true)
  if (typeof ResizeObserver !== 'undefined') {
    resizeObserver = new ResizeObserver(() => relayout())
  }
  await nextTick()
  relayout()
  observeCrumbs()
})

onBeforeUnmount(() => {
  window.removeEventListener(FAVORITES_CHANGED_EVENT, syncFavorite)
  document.removeEventListener('pointerdown', onDocumentPointerDown, true)
  document.removeEventListener('keydown', onDocumentKeyDown, true)
  resizeObserver?.disconnect()
  resizeObserver = null
})

watch(segments, async () => {
  hiddenOpen.value = false
  await nextTick()
  relayout()
})

watch(visibleFrom, (count) => {
  if (!count) hiddenOpen.value = false
})

// 字号设置会改变每一级的实际宽度，需要重新量一次
watch(
  () => ui.fileFontSize,
  async () => {
    await nextTick()
    relayout()
  },
)

watch(editing, async (active) => {
  if (active) {
    hiddenOpen.value = false
    return
  }
  await nextTick()
  relayout()
  observeCrumbs()
})

async function startEdit() {
  if (isRemote.value) return // 远程路径只读
  draft.value = props.tab.path
  editing.value = true
  await nextTick()
  inputRef.value?.focus()
  inputRef.value?.select()
}

async function commit() {
  if (!editing.value) return
  if (isRemote.value) {
    editing.value = false
    return
  }
  const value = draft.value.trim()
  editing.value = false
  if (!value) return

  try {
    // 支持 `%APPDATA%` / `~\Downloads` 这类输入；展开后再判断是文件还是目录。
    const target = await api.expandPath(value)
    if (!target || target === props.tab.path) return
    if (await api.pathIsFile(target)) {
      await paneStore.navigateTo(props.paneId, dirname(target), props.tab.id)
      const normalized = target.replace(/\//g, '\\').replace(/\\+$/, '').toLowerCase()
      const file = props.tab.entries.find(
        (entry) => entry.isDir === false && entry.path.replace(/\//g, '\\').replace(/\\+$/, '').toLowerCase() === normalized,
      )
      if (file) {
        props.tab.selected = [file.path]
        paneStore.revealTarget = { tabId: props.tab.id, path: file.path }
      }
      return
    }
    go(target)
  } catch (error) {
    ui.setStatusText(`无法检查路径：${String(error)}`)
    return
  }
}

function cancel() {
  editing.value = false
}

watch(
  () => props.tab.path,
  () => {
    editing.value = false
    syncFavorite()
  },
)
</script>

<template>
  <div class="path-bar">
    <div
      v-if="!editing"
      ref="crumbsEl"
      class="crumbs"
      :title="isRemote ? '远程路径（只读）' : '双击切换为可编辑路径'"
      @dblclick.stop.prevent="startEdit"
    >
      <button
        v-if="visibleFrom > 0 && ellipsisShown"
        ref="ellipsisEl"
        class="crumb ellipsis"
        :class="{ open: hiddenOpen }"
        :title="`隐藏的上级目录：${hiddenSegments.map((seg) => seg.label).join(' / ')}`"
        @click.stop="toggleHidden"
      >
        …
      </button>
      <template v-for="(seg, i) in visibleSegments" :key="seg.path">
        <span v-if="(visibleFrom > 0 && ellipsisShown) || i > 0" class="sep">
          <Icon name="chevron-right" :size="13" />
        </span>
        <button
          class="crumb"
          :class="{ last: i === visibleSegments.length - 1 }"
          :title="seg.label"
          @click="go(seg.path)"
        >
          {{ seg.label }}
        </button>
      </template>
    </div>
    <!-- 隐形量具：始终按完整路径渲染一遍，用于计算每一级的自然宽度 -->
    <div v-if="!editing" ref="mirrorEl" class="crumb-mirror" aria-hidden="true">
      <span class="crumb ellipsis">…</span>
      <template v-for="(seg, i) in segments" :key="seg.path">
        <span v-if="i > 0" class="sep"><Icon name="chevron-right" :size="13" /></span>
        <span class="crumb">{{ seg.label }}</span>
      </template>
    </div>
    <div v-if="!editing && hiddenOpen && hiddenSegments.length" ref="hiddenMenuEl" class="crumb-menu">
      <button
        v-for="seg in hiddenSegments"
        :key="seg.path"
        class="crumb-menu-item"
        :title="seg.path"
        @click="pickHidden(seg.path)"
      >
        <Icon name="folder" :size="14" />
        <span>{{ seg.label }}</span>
      </button>
    </div>
    <input
      v-if="editing"
      ref="inputRef"
      v-model="draft"
      class="path-input"
      spellcheck="false"
      title="支持 Windows 环境变量（如 %APPDATA%）与 ~ 主目录"
      @keyup.enter="commit"
      @keyup.esc="cancel"
      @blur="commit"
    />
    <button
      v-if="!isRemote"
      class="favorite-button"
      :class="{ 'is-favorite': isFavorite }"
      :title="isFavorite ? '当前目录已在收藏中' : '将当前目录加入收藏'"
      aria-label="将当前目录加入收藏"
      :disabled="isFavorite"
      @click="addCurrentFavorite"
    >
      <Icon name="star" :size="15" />
    </button>
  </div>
</template>

<style scoped>
.path-bar {
  position: relative;
  display: flex;
  align-items: center;
  gap: 6px;
  /* 面包屑之间的间距（与隐形量具共用，避免两处漂移） */
  --crumb-gap: 2px;
  height: var(--pane-bar-h);
  padding: 0 6px;
  background: var(--bg-panel);
  border-bottom: 1px solid var(--border);
  font-size: var(--file-font-size);
}
.favorite-button {
  flex: none;
  width: 28px;
  height: 28px;
  display: flex;
  align-items: center;
  justify-content: center;
  border-radius: 50%;
  color: var(--fg-muted);
}
.favorite-button:hover:not(:disabled) {
  background: var(--bg-hover);
  color: var(--fg);
}
.favorite-button.is-favorite {
  color: #eab308;
  opacity: 1;
}
.favorite-button.is-favorite :deep(svg) {
  fill: currentColor;
}
.crumbs {
  flex: 1;
  display: flex;
  align-items: center;
  gap: var(--crumb-gap);
  min-width: 0;
  height: 22px;
  overflow: hidden;
  white-space: nowrap;
}
.crumb {
  flex: none;
  display: inline-block;
  max-width: 220px;
  height: 18px;
  padding: 0 5px;
  overflow: hidden;
  border-radius: var(--radius);
  color: var(--fg-muted);
  font-size: inherit;
  line-height: 18px;
  text-align: left;
  text-overflow: ellipsis;
  white-space: nowrap;
  transition:
    background var(--t-fast),
    color var(--t-fast);
}
.crumb:hover {
  background: var(--bg-active);
  color: var(--fg);
}
.crumb.last {
  /* 极端窄的窗格里允许叶子目录自身省略，而不是被裁掉 */
  flex: 0 1 auto;
  min-width: 0;
  color: var(--fg);
}
.crumb.ellipsis {
  color: var(--fg-dim);
  letter-spacing: 0.5px;
}
.crumb.ellipsis.open {
  background: var(--bg-active);
  color: var(--fg);
}
.sep {
  flex: none;
  display: flex;
  align-items: center;
  color: var(--fg-dim);
}
/* 隐形量具：按完整路径渲染，只为取每一级的自然宽度 */
.crumb-mirror {
  position: absolute;
  top: 0;
  left: 0;
  display: flex;
  align-items: center;
  gap: var(--crumb-gap);
  width: max-content;
  visibility: hidden;
  pointer-events: none;
  white-space: nowrap;
}
.crumb-menu {
  position: absolute;
  top: calc(100% - 2px);
  left: 4px;
  z-index: 30;
  display: flex;
  flex-direction: column;
  gap: 1px;
  min-width: 150px;
  max-width: 320px;
  max-height: 264px;
  overflow-y: auto;
  padding: 4px;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--bg-elevated);
  box-shadow: var(--shadow-md);
}
.crumb-menu-item {
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  height: 26px;
  padding: 0 8px;
  border-radius: var(--radius-sm);
  color: var(--fg-muted);
  font-size: inherit;
  text-align: left;
}
.crumb-menu-item:hover {
  background: var(--bg-hover);
  color: var(--fg);
}
.crumb-menu-item :deep(svg) {
  flex: none;
  color: var(--ico-folder);
}
.crumb-menu-item span {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.path-input {
  flex: 1;
  height: 22px;
  min-width: 0;
  font-family: "Microsoft YaHei", "微软雅黑", sans-serif;
  font-size: inherit;
}
</style>

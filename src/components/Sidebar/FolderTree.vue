<script setup lang="ts">
// 文件导航面板：此电脑（驱动器 + 文件夹懒加载树） + 常用位置 + 收藏
import { onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { api } from '../../api'
import { usePaneStore } from '../../stores/pane'
import { useUiStore } from '../../stores/ui'
import TreeNode from './TreeNode.vue'
import NetworkPanel from './NetworkPanel.vue'
import {
  FAVORITES_CHANGED_EVENT,
  RECENT_DIRECTORIES_KEY,
  RECENT_DIRECTORIES_CHANGED_EVENT,
  loadFavorites,
  loadRecentDirectories,
  resolveSystemLocations,
  saveFavorites,
} from './shared'
import type { QuickLocation, RecentDirectory, TreeNodeData } from './shared'
import Icon from '../icons/Icon.vue'

const pane = usePaneStore()
const ui = useUiStore()

const SECTION_KEY = 'file-povit.navSections'
type SectionKey = 'system' | 'favorites' | 'recent' | 'computer' | 'network'
function readSections(): Record<SectionKey, boolean> {
  const defaults = { system: true, favorites: true, recent: true, computer: true, network: true }
  try {
    const saved: unknown = JSON.parse(localStorage.getItem(SECTION_KEY) || '{}')
    if (!saved || typeof saved !== 'object' || Array.isArray(saved)) return defaults
    for (const key of Object.keys(defaults) as SectionKey[]) {
      if (typeof (saved as Record<string, unknown>)[key] === 'boolean') {
        defaults[key] = (saved as Record<string, boolean>)[key]
      }
    }
  } catch { /* 保持默认展开 */ }
  return defaults
}
const sections = ref(readSections())
watch(sections, (value) => {
  try { localStorage.setItem(SECTION_KEY, JSON.stringify(value)) } catch { /* 忽略持久化失败 */ }
}, { deep: true })

const roots = ref<TreeNodeData[]>([])
const loading = ref(false)
const error = ref<string | null>(null)
const wslDistros = ref<string[]>([])
const wslExpanded = ref(false)
const wslLoading = ref(false)
const wslError = ref<string | null>(null)

const system = ref<QuickLocation[]>([])
const favorites = ref<QuickLocation[]>([])
const recentDirectories = ref<RecentDirectory[]>([])
const locLoading = ref(false)
const locError = ref<string | null>(null)
const draggingFavorite = ref<string | null>(null)
const dragOverFavorite = ref<string | null>(null)
let favoritePointerId: number | null = null
let favoritePointerStart: { path: string; x: number; y: number } | null = null
let favoriteDragMoved = false
let recentRefreshTimer: number | null = null

async function loadRoots() {
  loading.value = true
  error.value = null
  try {
    const drives = await api.getRoots()
    roots.value = drives.map((d) => ({
      name: d.name || d.mount,
      path: d.mount,
    }))
  } catch (e) {
    error.value = String(e)
    roots.value = []
  } finally {
    loading.value = false
  }
}

async function loadWslDistros() {
  wslLoading.value = true
  wslError.value = null
  try {
    wslDistros.value = await api.listWslDistros()
  } catch (cause) {
    wslError.value = String(cause)
    wslDistros.value = []
  } finally {
    wslLoading.value = false
  }
}

function refreshComputer() {
  void loadRoots()
  void loadWslDistros()
}

async function loadLocations() {
  locLoading.value = true
  locError.value = null
  try {
    system.value = await resolveSystemLocations()
  } catch (e) {
    locError.value = String(e)
    system.value = []
  } finally {
    locLoading.value = false
  }
  favorites.value = loadFavorites()
}

function go(path: string) {
  ui.setStatusText(`导航到 ${path}`)
  void pane.navigate(path)
}

function removeFavorite(path: string) {
  favorites.value = favorites.value.filter((f) => f.path !== path)
  saveFavorites(favorites.value)
}

function onFavoriteClick(path: string) {
  if (favoriteDragMoved) {
    favoriteDragMoved = false
    return
  }
  go(path)
}

function startFavoriteDrag(ev: PointerEvent, path: string) {
  if (ev.button !== 0 || (ev.target as HTMLElement).closest('button')) return
  favoritePointerId = ev.pointerId
  favoritePointerStart = { path, x: ev.clientX, y: ev.clientY }
  ;(ev.currentTarget as HTMLElement).setPointerCapture(ev.pointerId)
}

function moveFavorite(ev: PointerEvent) {
  if (favoritePointerId !== ev.pointerId || !favoritePointerStart) return
  const sourcePath = favoritePointerStart.path
  if (!draggingFavorite.value) {
    const dx = ev.clientX - favoritePointerStart.x
    const dy = ev.clientY - favoritePointerStart.y
    if (Math.hypot(dx, dy) < 5) return
    draggingFavorite.value = sourcePath
  }

  const target = document.elementFromPoint(ev.clientX, ev.clientY)?.closest<HTMLElement>('[data-favorite-path]')
  const targetPath = target?.dataset.favoritePath
  if (!target || !targetPath || targetPath === sourcePath) {
    dragOverFavorite.value = null
    return
  }

  dragOverFavorite.value = targetPath
  const sourceIndex = favorites.value.findIndex((fav) => fav.path === sourcePath)
  const targetIndex = favorites.value.findIndex((fav) => fav.path === targetPath)
  if (sourceIndex < 0 || targetIndex < 0) return
  const insertAfter = ev.clientY >= target.getBoundingClientRect().top + target.offsetHeight / 2
  const reordered = [...favorites.value]
  const [moved] = reordered.splice(sourceIndex, 1)
  const adjustedTarget = targetIndex > sourceIndex ? targetIndex - 1 : targetIndex
  const insertIndex = adjustedTarget + (insertAfter ? 1 : 0)
  if (insertIndex === sourceIndex) return
  reordered.splice(insertIndex, 0, moved)
  favorites.value = reordered
  favoriteDragMoved = true
}

function finishFavoriteDrag(ev: PointerEvent) {
  if (favoritePointerId !== ev.pointerId) return
  favoritePointerId = null
  favoritePointerStart = null
  if (favoriteDragMoved) saveFavorites(favorites.value)
  window.setTimeout(() => { favoriteDragMoved = false }, 0)
  draggingFavorite.value = null
  dragOverFavorite.value = null
}

onMounted(() => {
  window.addEventListener(FAVORITES_CHANGED_EVENT, syncFavorites)
  window.addEventListener(RECENT_DIRECTORIES_CHANGED_EVENT, syncRecentDirectories)
  refreshComputer()
  void loadLocations()
  syncRecentDirectories()
})

function syncFavorites() {
  favorites.value = loadFavorites()
}

function syncRecentDirectories() {
  recentDirectories.value = loadRecentDirectories()
  scheduleRecentRefresh()
}

function scheduleRecentRefresh() {
  if (recentRefreshTimer !== null) window.clearTimeout(recentRefreshTimer)
  recentRefreshTimer = null
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(RECENT_DIRECTORIES_KEY) || '[]')
    if (!Array.isArray(parsed)) return
    const cutoff = Date.now() - 24 * 60 * 60 * 1000
    const latestRecent = Math.max(
      ...parsed
        .filter(
          (event): event is { openedAt: number } =>
            !!event && typeof event === 'object' && typeof (event as { openedAt?: unknown }).openedAt === 'number',
        )
        .map((event) => event.openedAt)
        .filter((openedAt) => openedAt > cutoff),
    )
    if (Number.isFinite(latestRecent)) {
      const delay = Math.max(0, latestRecent + 24 * 60 * 60 * 1000 - Date.now() + 20)
      recentRefreshTimer = window.setTimeout(syncRecentDirectories, delay)
    }
  } catch {
    // 忽略本地历史读取失败。
  }
}

onBeforeUnmount(() => {
  window.removeEventListener(FAVORITES_CHANGED_EVENT, syncFavorites)
  window.removeEventListener(RECENT_DIRECTORIES_CHANGED_EVENT, syncRecentDirectories)
  if (recentRefreshTimer !== null) window.clearTimeout(recentRefreshTimer)
})
</script>

<template>
  <div class="files-nav">
    <!-- 常用位置 -->
    <section class="nav-section">
      <div class="nav-head">
        <button class="nav-toggle" :aria-expanded="sections.system" @click="sections.system = !sections.system">
          <Icon name="chevron-right" :size="11" :class="{ open: sections.system }" /><span class="nav-title">常用位置</span>
        </button>
      </div>
      <div v-if="sections.system && locLoading" class="state">加载中…</div>
      <div v-else-if="sections.system && locError" class="state err">系统路径不可用：{{ locError }}</div>
      <ul v-else-if="sections.system" class="nav-list">
        <li v-for="loc in system" :key="loc.path" :title="loc.path" @click="go(loc.path)">
          <span class="ico"><Icon :name="loc.icon" :size="14" /></span>
          <span class="label">{{ loc.label }}</span>
        </li>
      </ul>
    </section>

    <!-- 收藏 -->
    <section class="nav-section">
      <div class="nav-head">
        <button class="nav-toggle" :aria-expanded="sections.favorites" @click="sections.favorites = !sections.favorites">
          <Icon name="chevron-right" :size="11" :class="{ open: sections.favorites }" /><span class="nav-title">收藏</span>
        </button>
      </div>
      <div v-if="sections.favorites && !favorites.length" class="state">暂无收藏，可从窗格工具栏添加当前目录</div>
      <TransitionGroup v-else-if="sections.favorites" tag="ul" name="favorite-sort" class="nav-list">
        <li
          v-for="fav in favorites"
          :key="fav.path"
          :data-favorite-path="fav.path"
          :title="fav.path"
          :class="{ 'drag-over': dragOverFavorite === fav.path, dragging: draggingFavorite === fav.path }"
          @click="onFavoriteClick(fav.path)"
          @pointerdown="startFavoriteDrag($event, fav.path)"
          @pointermove="moveFavorite"
          @pointerup="finishFavoriteDrag"
          @pointercancel="finishFavoriteDrag"
        >
          <span class="ico"><Icon :name="fav.icon" :size="14" /></span>
          <span class="label">{{ fav.label }}</span>
          <button class="del" title="移除收藏" @click.stop="removeFavorite(fav.path)">
            <Icon name="x" :size="11" />
          </button>
        </li>
      </TransitionGroup>
    </section>

    <!-- 最近高频打开文件的目录 -->
    <section class="nav-section">
      <div class="nav-head">
        <button class="nav-toggle" :aria-expanded="sections.recent" @click="sections.recent = !sections.recent">
          <Icon name="chevron-right" :size="11" :class="{ open: sections.recent }" /><span class="nav-title">最近常用</span>
        </button>
      </div>
      <div v-if="sections.recent && !recentDirectories.length" class="state">双击打开文件后，这里会显示常用目录</div>
      <ul v-else-if="sections.recent" class="nav-list">
        <li
          v-for="directory in recentDirectories"
          :key="directory.path"
          :title="`${directory.path}（所选 24 小时窗口打开 ${directory.count} 次）`"
          @click="go(directory.path)"
        >
          <span class="ico"><Icon :name="directory.icon" :size="14" /></span>
          <span class="label">{{ directory.label }}</span>
          <span class="count">{{ directory.count }} 次</span>
        </li>
      </ul>
    </section>

    <!-- 此电脑 -->
    <section class="nav-section">
      <div class="nav-head">
        <button class="nav-toggle" :aria-expanded="sections.computer" @click="sections.computer = !sections.computer">
          <Icon name="chevron-right" :size="11" :class="{ open: sections.computer }" /><span class="nav-title">此电脑</span>
        </button>
        <button class="mini" title="刷新驱动器和 WSL 实例" :disabled="loading || wslLoading" @click="refreshComputer">
          <Icon name="refresh" :size="13" />
        </button>
      </div>
      <div v-if="sections.computer && loading && !roots.length" class="state">正在读取驱动器…</div>
      <div v-if="sections.computer && error" class="state err">驱动器读取失败：{{ error }}</div>
      <div v-show="sections.computer" class="tree-body">
        <TreeNode
          v-for="root in roots"
          :key="root.path"
          :node="root"
          :depth="0"
          @navigate="go"
        />
        <div v-if="wslDistros.length" class="wsl-tree">
          <button class="wsl-row" :title="`Linux（${wslDistros.length} 个 WSL 实例）`" @click="wslExpanded = !wslExpanded">
            <span class="wsl-twist" :class="{ open: wslExpanded }"><Icon name="chevron-right" :size="11" /></span>
            <span class="wsl-icon"><Icon name="linux" :size="14" :stroke-width="1.4" /></span>
            <span class="wsl-name">Linux</span>
          </button>
          <TreeNode
            v-for="distro in wslExpanded ? wslDistros : []"
            :key="distro"
            :node="{ name: distro, path: `\\\\wsl.localhost\\${distro}` }"
            :depth="1"
            @navigate="go"
          />
        </div>
      </div>
      <div v-if="sections.computer && !loading && !wslLoading && !roots.length && !wslDistros.length && !error" class="state">未检测到驱动器或 WSL 实例</div>
      <div v-if="sections.computer && wslError" class="state err">WSL 实例读取失败：{{ wslError }}</div>
    </section>

    <!-- 网络 -->
    <section class="nav-section">
      <div class="nav-head">
        <button class="nav-toggle" :aria-expanded="sections.network" @click="sections.network = !sections.network">
          <Icon name="chevron-right" :size="11" :class="{ open: sections.network }" /><span class="nav-title">网络</span>
        </button>
      </div>
      <NetworkPanel v-show="sections.network" />
    </section>
  </div>
</template>

<style scoped>
.files-nav {
  display: flex;
  flex-direction: column;
  gap: 10px;
}
.nav-section {
  display: flex;
  flex-direction: column;
}
.nav-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 2px 4px 6px;
  border-bottom: 1px solid var(--border);
  margin-bottom: 4px;
}
.nav-title {
  font-size: 0.9167rem;
  color: var(--fg-muted);
  letter-spacing: 0.04em;
}
.nav-toggle {
  display: flex;
  align-items: center;
  gap: 4px;
  flex: 1;
  min-width: 0;
  text-align: left;
  color: var(--fg-dim);
}
.nav-toggle:hover .nav-title { color: var(--fg); }
.nav-toggle :deep(svg) { flex: none; transition: transform 0.12s ease; }
.nav-toggle :deep(svg.open) { transform: rotate(90deg); }
.mini {
  width: 20px;
  height: 20px;
  display: flex;
  align-items: center;
  justify-content: center;
  border-radius: var(--radius);
  color: var(--fg-muted);
}
.mini:hover:not(:disabled) {
  background: var(--bg-hover);
  color: var(--fg);
}
.mini:disabled {
  opacity: 0.4;
  cursor: default;
}
.tree-body {
  display: flex;
  flex-direction: column;
}
.wsl-row {
  display: flex;
  align-items: center;
  gap: 4px;
  width: 100%;
  height: 22px;
  padding: 0 6px;
  border-radius: var(--radius);
  text-align: left;
}
.wsl-row:hover { background: var(--bg-hover); }
.wsl-twist {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 12px;
  flex: none;
  color: var(--fg-dim);
  transition: transform 0.12s ease;
}
.wsl-twist.open { transform: rotate(90deg); }
.wsl-icon { display: flex; flex: none; color: var(--accent); }
.wsl-name { color: var(--fg-folder); font-size: 1rem; }
.nav-list {
  list-style: none;
  margin: 0;
  padding: 0;
}
.nav-list li {
  display: flex;
  align-items: center;
  gap: 6px;
  height: 24px;
  padding: 0 6px;
  border-radius: var(--radius);
  cursor: pointer;
  overflow: hidden;
}
.nav-list li:hover {
  background: var(--bg-hover);
}
.nav-list li[data-favorite-path] {
  cursor: grab;
}
.nav-list li.dragging {
  cursor: grabbing;
  position: relative;
  z-index: 1;
  background: var(--bg-active);
  box-shadow: var(--shadow-sm);
}
.nav-list li.drag-over {
  outline: 1px solid color-mix(in srgb, var(--accent) 60%, transparent);
  outline-offset: -1px;
}
.favorite-sort-move {
  transition: transform 140ms cubic-bezier(0.2, 0.75, 0.25, 1);
}
.ico {
  flex: none;
  display: flex;
  align-items: center;
  color: var(--fg-muted);
}
.label {
  flex: 1;
  font-size: 1rem;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.count {
  flex: none;
  color: var(--fg-dim);
  font-size: 0.8333rem;
}
.del {
  flex: none;
  width: 16px;
  height: 16px;
  display: flex;
  align-items: center;
  justify-content: center;
  border-radius: var(--radius);
  color: var(--fg-dim);
}
.del:hover {
  background: var(--danger);
  color: var(--accent-fg);
}
.state {
  padding: 4px 6px;
  font-size: 0.9167rem;
  color: var(--fg-dim);
}
.state.err {
  color: var(--danger);
}
</style>

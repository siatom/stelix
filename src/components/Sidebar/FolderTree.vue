<script setup lang="ts">
// 文件导航面板：本机（驱动器 + 文件夹懒加载树） + 常用位置 + 收藏
import { onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { api } from '../../api'
import { listen } from '@tauri-apps/api/event'
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
import FileIcon from '../icons/FileIcon.vue'
import { RECYCLE_BIN_SHELL_PATH } from '../Pane/utils'

const pane = usePaneStore()
const ui = useUiStore()

const SECTION_KEY = 'stelix.navSections'
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
const fileDropOverFavorite = ref<string | null>(null)
const openFavoriteMenu = ref<string | null>(null)
let favoritePointerId: number | null = null
let favoritePointerStart: { path: string; x: number; y: number } | null = null
let favoriteDragMoved = false
let recentRefreshTimer: number | null = null
let unlistenDriveChanges: (() => void) | null = null
let folderTreeMounted = false

async function loadRoots() {
  loading.value = true
  error.value = null
  try {
    const drives = await api.getRoots()
    roots.value = drives.map((d) => ({
      name: d.name || d.mount,
      path: d.mount,
      ejectable: d.ejectable,
    }))
  } catch (e) {
    error.value = String(e)
    roots.value = []
  } finally {
    loading.value = false
  }
}

async function refreshRootsIfChanged() {
  if (loading.value) return
  try {
    const drives = await api.getRoots()
    const nextRoots = drives.map((d) => ({ name: d.name || d.mount, path: d.mount, ejectable: d.ejectable }))
    if (nextRoots.length !== roots.value.length || nextRoots.some((root, i) => root.path !== roots.value[i]?.path)) {
      roots.value = nextRoots
      error.value = null
    }
  } catch {
    // 临时枚举失败时保留当前列表，避免打断导航。
  }
}

async function ejectDrive(path: string) {
  try {
    await api.ejectDrive(path)
    ui.setStatusText(`已弹出 ${path}`)
    window.setTimeout(() => void refreshRootsIfChanged(), 800)
  } catch (cause) {
    ui.setStatusText(`无法弹出 ${path}：${String(cause)}`)
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

async function onLocationContextMenu(event: MouseEvent, path: string) {
  event.preventDefault()
  event.stopPropagation()
  try {
    const shown = await api.showNativeShellMenu(path, {
      x: event.clientX,
      y: event.clientY,
      scale: window.devicePixelRatio || 1,
    })
    if (!shown) ui.setStatusText('没有可用的系统菜单项')
  } catch (cause) {
    ui.setStatusText(`无法打开系统菜单：${String(cause)}`)
  }
}

function toggleFavoriteMenu(event: MouseEvent, path: string) {
  event.stopPropagation()
  openFavoriteMenu.value = openFavoriteMenu.value === path ? null : path
}

function closeFavoriteMenu() {
  openFavoriteMenu.value = null
}

function onDocumentKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape' && openFavoriteMenu.value) closeFavoriteMenu()
}

function removeFavorite(path: string) {
  if (openFavoriteMenu.value === path) openFavoriteMenu.value = null
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

interface FileDragPayload {
  paneId: string
  paths: string[]
}

const FILE_DRAG_TYPE = 'application/x-stelix-items'
const FILE_DRAG_FALLBACK_TYPE = 'text/plain'
const FILE_DRAG_PREFIX = 'stelix:'

function readFileDragPayload(ev: DragEvent): FileDragPayload | null {
  const transfer = ev.dataTransfer
  const custom = transfer?.getData(FILE_DRAG_TYPE)
  const plain = transfer?.getData(FILE_DRAG_FALLBACK_TYPE) ?? ''
  const raw = custom || (plain.startsWith(FILE_DRAG_PREFIX) ? plain.slice(FILE_DRAG_PREFIX.length) : '')
  if (!raw) return null
  try {
    const payload = JSON.parse(raw) as Partial<FileDragPayload>
    if (typeof payload.paneId !== 'string' || !Array.isArray(payload.paths)) return null
    const paths = payload.paths.filter((path): path is string => typeof path === 'string' && !!path)
    return paths.length ? { paneId: payload.paneId, paths } : null
  } catch {
    return null
  }
}

function onFavoriteDragOver(ev: DragEvent, path: string) {
  if (!readFileDragPayload(ev)) return
  ev.preventDefault()
  ev.stopPropagation()
  if (ev.dataTransfer) ev.dataTransfer.dropEffect = ev.shiftKey ? 'copy' : 'move'
  fileDropOverFavorite.value = path
}

function onFavoriteDragLeave(ev: DragEvent, path: string) {
  const related = ev.relatedTarget as Node | null
  if (related && (ev.currentTarget as HTMLElement).contains(related)) return
  if (fileDropOverFavorite.value === path) fileDropOverFavorite.value = null
}

async function onFavoriteDrop(ev: DragEvent, targetPath: string) {
  ev.preventDefault()
  ev.stopPropagation()
  fileDropOverFavorite.value = null
  const payload = readFileDragPayload(ev)
  if (!payload) return
  const op = ev.shiftKey ? 'copy' : 'move'
  const result = await pane.runFs(payload.paneId, { op, sources: payload.paths, target: targetPath })
  if (result.ok) await pane.reloadByPath(targetPath)
}

onMounted(() => {
  folderTreeMounted = true
  window.addEventListener(FAVORITES_CHANGED_EVENT, syncFavorites)
  window.addEventListener(RECENT_DIRECTORIES_CHANGED_EVENT, syncRecentDirectories)
  document.addEventListener('click', closeFavoriteMenu)
  document.addEventListener('keydown', onDocumentKeydown)
  refreshComputer()
  void listen('computer:drives-changed', () => void refreshRootsIfChanged()).then((unlisten) => {
    if (folderTreeMounted) unlistenDriveChanges = unlisten
    else unlisten()
  })
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
  folderTreeMounted = false
  window.removeEventListener(FAVORITES_CHANGED_EVENT, syncFavorites)
  window.removeEventListener(RECENT_DIRECTORIES_CHANGED_EVENT, syncRecentDirectories)
  document.removeEventListener('click', closeFavoriteMenu)
  document.removeEventListener('keydown', onDocumentKeydown)
  if (recentRefreshTimer !== null) window.clearTimeout(recentRefreshTimer)
  unlistenDriveChanges?.()
})
</script>

<template>
  <div class="files-nav">
    <!-- 常用位置 -->
    <section class="nav-section system-section">
      <div class="nav-head">
        <button class="nav-toggle" :aria-expanded="sections.system" @click="sections.system = !sections.system">
          <Icon name="chevron-right" :size="11" :class="{ open: sections.system }" /><Icon name="home" class="section-icon system" :size="18" /><span class="nav-title">常用位置</span>
        </button>
      </div>
      <div v-if="sections.system && locLoading" class="state">加载中…</div>
      <div v-else-if="sections.system && locError" class="state err">系统路径不可用：{{ locError }}</div>
      <ul v-else-if="sections.system" class="nav-list">
        <li v-for="loc in system" :key="loc.path" :title="loc.path" @click="go(loc.path)" @contextmenu="onLocationContextMenu($event, loc.path)">
          <span class="ico system-ico" :class="`system-${loc.icon}`">
            <FileIcon :path="loc.path" :is-dir="true" :ext="null" :use-path-icon="true" :fallback="loc.icon" :size="18" />
          </span>
          <span class="label">{{ loc.label }}</span>
        </li>
      </ul>
    </section>

    <!-- 收藏 -->
    <section class="nav-section favorites-section">
      <div class="nav-head">
        <button class="nav-toggle" :aria-expanded="sections.favorites" @click="sections.favorites = !sections.favorites">
          <Icon name="chevron-right" :size="11" :class="{ open: sections.favorites }" /><Icon name="star" class="section-icon favorites" :size="18" /><span class="nav-title">收藏</span>
        </button>
      </div>
      <div v-if="sections.favorites && !favorites.length" class="state">暂无收藏，可从窗格工具栏添加当前目录</div>
      <TransitionGroup v-else-if="sections.favorites" tag="ul" name="favorite-sort" class="nav-list">
        <li
          v-for="(fav, index) in favorites"
          :key="fav.path"
          :data-favorite-path="fav.path"
          :title="fav.path"
          :class="{ 'drag-over': dragOverFavorite === fav.path, 'file-drop-over': fileDropOverFavorite === fav.path, dragging: draggingFavorite === fav.path }"
          @click="onFavoriteClick(fav.path)"
          @contextmenu="onLocationContextMenu($event, fav.path)"
          @pointerdown="startFavoriteDrag($event, fav.path)"
          @pointermove="moveFavorite"
          @pointerup="finishFavoriteDrag"
          @pointercancel="finishFavoriteDrag"
          @dragover="onFavoriteDragOver($event, fav.path)"
          @dragleave="onFavoriteDragLeave($event, fav.path)"
          @drop="onFavoriteDrop($event, fav.path)"
        >
          <span class="ico favorite-ico" :class="`favorite-color-${index % 5}`"><Icon name="folder" :size="18" /></span>
          <span class="label">{{ fav.label }}</span>
          <div
            class="fav-more-wrap"
            :class="{ 'menu-open': openFavoriteMenu === fav.path }"
            @keydown.esc.stop="closeFavoriteMenu"
          >
            <button
              class="fav-more"
              title="更多操作"
              :aria-label="`${fav.label}更多操作`"
              aria-haspopup="menu"
              :aria-expanded="openFavoriteMenu === fav.path"
              @click="toggleFavoriteMenu($event, fav.path)"
            >
              <Icon name="more" :size="14" />
            </button>
            <div v-if="openFavoriteMenu === fav.path" class="fav-menu" role="menu" @click.stop>
              <button role="menuitem" @click="removeFavorite(fav.path)"><Icon name="x" :size="13" />取消收藏</button>
            </div>
          </div>
        </li>
      </TransitionGroup>
    </section>

    <!-- 最近高频打开文件的目录 -->
    <section class="nav-section recent-section">
      <div class="nav-head">
        <button class="nav-toggle" :aria-expanded="sections.recent" @click="sections.recent = !sections.recent">
          <Icon name="chevron-right" :size="11" :class="{ open: sections.recent }" /><Icon name="clock" class="section-icon recent" :size="18" /><span class="nav-title">最近常用</span>
        </button>
      </div>
      <div v-if="sections.recent && !recentDirectories.length" class="state">双击打开文件后，这里会显示常用目录</div>
      <ul v-else-if="sections.recent" class="nav-list">
        <li
          v-for="directory in recentDirectories"
          :key="directory.path"
          :title="`${directory.path}（所选 24 小时窗口打开 ${directory.count} 次）`"
          @click="go(directory.path)"
          @contextmenu="onLocationContextMenu($event, directory.path)"
        >
          <span class="ico">
            <FileIcon
              :path="directory.path"
              :is-dir="true"
              :ext="null"
              :use-path-icon="true"
              :fallback="directory.icon"
              :size="18"
            />
          </span>
          <span class="label">{{ directory.label }}</span>
          <span class="count">{{ directory.count }} 次</span>
        </li>
      </ul>
    </section>

    <!-- 本机 -->
    <section class="nav-section">
      <div class="nav-head">
        <button class="nav-toggle" :aria-expanded="sections.computer" @click="sections.computer = !sections.computer">
          <Icon name="chevron-right" :size="11" :class="{ open: sections.computer }" /><Icon name="laptop-minimal" class="section-icon computer" :size="18" /><span class="nav-title">本机</span>
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
          :ejectable="Boolean(root.ejectable)"
          @navigate="go"
          @contextmenu="onLocationContextMenu"
          @eject="ejectDrive"
        />
        <div v-if="wslDistros.length" class="wsl-tree">
          <button class="wsl-row" :title="`Linux（${wslDistros.length} 个 WSL 实例）`" @click="wslExpanded = !wslExpanded">
            <span class="wsl-twist" :class="{ open: wslExpanded }"><Icon name="chevron-right" :size="11" /></span>
            <span class="wsl-icon" aria-hidden="true">
              <img class="wsl-logo" src="/wsl-tux.svg" alt="" aria-hidden="true" />
            </span>
            <span class="wsl-name">Linux（WSL）</span>
          </button>
          <TreeNode
            v-for="distro in wslExpanded ? wslDistros : []"
            :key="distro"
            :node="{ name: distro, path: `\\\\wsl.localhost\\${distro}` }"
            :depth="1"
            @navigate="go"
            @contextmenu="onLocationContextMenu"
          />
        </div>
        <div class="tree-sep" aria-hidden="true" />
        <button class="trash-row" title="回收站" @click="pane.openTrash()">
          <span class="trash-twist" aria-hidden="true" />
          <span class="trash-icon">
            <FileIcon
              :path="RECYCLE_BIN_SHELL_PATH"
              :is-dir="false"
              :ext="null"
              :trash-state="true"
              fallback="trash"
              :size="18"
            />
          </span>
          <span class="label">回收站</span>
        </button>
      </div>
      <div v-if="sections.computer && !loading && !wslLoading && !roots.length && !wslDistros.length && !error" class="state">未检测到驱动器或 WSL 实例</div>
      <div v-if="sections.computer && wslError" class="state err">WSL 实例读取失败：{{ wslError }}</div>
    </section>

    <!-- 网络 -->
    <section class="nav-section">
      <div class="nav-head">
        <button class="nav-toggle" :aria-expanded="sections.network" @click="sections.network = !sections.network">
          <Icon name="chevron-right" :size="11" :class="{ open: sections.network }" /><Icon name="network" class="section-icon network" :size="18" /><span class="nav-title">网络</span>
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
  font-size: var(--sidebar-font-size);
}
.nav-section {
  display: flex;
  flex-direction: column;
}
.nav-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-left: -6px;
  padding: 2px 4px 6px 0;
  border-bottom: 1px solid var(--border);
  margin-bottom: 4px;
}
.nav-title {
  font-size: 1em;
  color: var(--fg-muted);
  letter-spacing: 0.04em;
}
.section-icon {
  flex: none;
  margin-left: 2px;
}
.section-icon.system { color: #e5a22b; }
.section-icon.favorites { color: #f2bd63; }
.section-icon.computer { color: #4f83e8; }
.section-icon.network { color: #16a085; }
.section-icon.recent { color: var(--fg-muted); }
.section-icon.favorites :deep(path),
.favorite-ico :deep(path) { fill: currentColor; }
.nav-toggle .section-icon,
.nav-toggle .nav-title { position: relative; left: -6px; }
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
.nav-toggle :deep(svg:first-child) { position: relative; left: -6px; }
.nav-toggle :deep(svg.open) { transform: rotate(90deg); }
.mini {
  width: 20px;
  height: 20px;
  display: flex;
  align-items: center;
  justify-content: center;
  border-radius: var(--radius);
  color: var(--fg-muted);
  /* 默认隐藏，鼠标悬停分区标题或键盘聚焦时才出现（占位不变，避免标题跳动） */
  opacity: 0;
  transition: opacity 120ms ease;
}
.nav-head:hover .mini,
.nav-head:focus-within .mini {
  opacity: 1;
}
.mini:hover:not(:disabled) {
  background: var(--bg-hover);
  color: var(--fg);
}
.mini:disabled {
  cursor: default;
}
.mini:disabled :deep(svg) {
  opacity: 0.4;
}
.tree-body {
  display: flex;
  flex-direction: column;
}
.trash-row {
  display: flex;
  align-items: center;
  gap: 4px;
  width: 100%;
  height: var(--sidebar-row-h);
  padding: 0 6px;
  border-radius: var(--radius);
  color: var(--fg-folder);
  text-align: left;
}
.trash-row:hover { background: var(--bg-hover); }
.trash-twist { width: 12px; flex: none; }
.trash-icon {
  flex: none;
  display: flex;
  color: var(--fg-muted);
}
.trash-row:hover .trash-icon { color: var(--fg); }
.tree-sep {
  height: 1px;
  margin: 3px 6px 4px;
  background: var(--border);
}
.wsl-row {
  display: flex;
  align-items: center;
  gap: 4px;
  width: 100%;
  height: var(--sidebar-row-h);
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
.wsl-icon { display: flex; flex: none; }
.wsl-logo { display: block; width: 18px; height: 18px; object-fit: contain; }
.wsl-name { color: var(--fg-folder); font-size: 1em; }
.nav-list {
  list-style: none;
  margin: 0;
  padding: 0;
}
.system-section .nav-list,
.favorites-section .nav-list,
.recent-section .nav-list { padding-left: 16px; }
.nav-list li {
  display: flex;
  align-items: center;
  gap: 6px;
  height: var(--sidebar-row-h);
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
.nav-list li.file-drop-over {
  background: color-mix(in srgb, var(--accent) 16%, transparent);
  outline: 2px solid color-mix(in srgb, var(--accent) 72%, transparent);
  outline-offset: -2px;
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
.favorite-ico.favorite-color-0 { color: #f2bd63; }
.favorite-ico.favorite-color-1 { color: #56c7b0; }
.favorite-ico.favorite-color-2 { color: #b890eb; }
.favorite-ico.favorite-color-3 { color: #e7849b; }
.favorite-ico.favorite-color-4 { color: #67c7e8; }
.system-ico { color: var(--ico-folder); }
.system-ico.system-home { color: #e5a22b; }
.system-ico.system-monitor { color: #4f83e8; }
.system-ico.system-download { color: #56c7b0; }
.system-ico.system-file-text { color: #f2bd63; }
.system-ico.system-image { color: #e7849b; }
.system-ico.system-user { color: #b890eb; }
.system-ico.system-windows { color: #67c7e8; }
.label {
  flex: 1;
  font-size: 1em;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.count {
  flex: none;
  display: inline-flex;
  align-items: center;
  padding: 1px 7px;
  border-radius: 999px;
  font-size: 0.75em;
  font-weight: 500;
  line-height: 1.6;
  font-variant-numeric: tabular-nums;
  color: var(--fg-folder);
  background: color-mix(in srgb, var(--fg-folder) 16%, transparent);
}
.favorites-section .nav-list li {
  overflow: visible;
}
.fav-more-wrap {
  position: relative;
  flex: none;
  opacity: 0;
  transition: opacity 120ms ease;
}
/* 仅鼠标悬停在收藏项、键盘聚焦或菜单展开时显示“更多操作” */
.nav-list li:hover .fav-more-wrap,
.fav-more-wrap:focus-within,
.fav-more-wrap.menu-open {
  opacity: 1;
}
.fav-more {
  width: 18px;
  height: 18px;
  display: flex;
  align-items: center;
  justify-content: center;
  border-radius: var(--radius);
  color: var(--fg-dim);
}
.fav-more:hover,
.fav-more[aria-expanded="true"] {
  background: var(--bg-hover);
  color: var(--fg);
}
.fav-menu {
  position: absolute;
  z-index: 20;
  top: calc(100% + 3px);
  right: 0;
  min-width: 112px;
  padding: 4px;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--bg-panel);
  box-shadow: var(--shadow-md, var(--shadow-lg));
}
.fav-menu button {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  padding: 6px 8px;
  border-radius: var(--radius);
  color: var(--fg);
  text-align: left;
  font-size: 0.9167em;
  white-space: nowrap;
}
.fav-menu button:hover {
  background: var(--bg-hover);
}
.state {
  padding: 4px 6px;
  font-size: 0.9167em;
  color: var(--fg-dim);
}
.state.err {
  color: var(--danger);
}
</style>

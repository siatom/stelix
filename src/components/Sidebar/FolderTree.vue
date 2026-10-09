<script setup lang="ts">
// 文件导航面板：此电脑（驱动器 + 文件夹懒加载树） + 常用位置 + 收藏
import { onMounted, ref } from 'vue'
import { api } from '../../api'
import { usePaneStore } from '../../stores/pane'
import { useUiStore } from '../../stores/ui'
import TreeNode from './TreeNode.vue'
import { basename, loadFavorites, resolveSystemLocations, saveFavorites } from './shared'
import type { QuickLocation, TreeNodeData } from './shared'
import Icon from '../icons/Icon.vue'

const pane = usePaneStore()
const ui = useUiStore()

const roots = ref<TreeNodeData[]>([])
const loading = ref(false)
const error = ref<string | null>(null)

const system = ref<QuickLocation[]>([])
const favorites = ref<QuickLocation[]>([])
const locLoading = ref(false)
const locError = ref<string | null>(null)

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

function addCurrent() {
  const path = pane.activeTab?.path
  if (!path) return
  if (favorites.value.some((f) => f.path === path)) return
  favorites.value = [
    ...favorites.value,
    { label: basename(path) || path, icon: 'star', path },
  ]
  saveFavorites(favorites.value)
}

function removeFavorite(path: string) {
  favorites.value = favorites.value.filter((f) => f.path !== path)
  saveFavorites(favorites.value)
}

onMounted(() => {
  void loadRoots()
  void loadLocations()
})
</script>

<template>
  <div class="files-nav">
    <!-- 常用位置 -->
    <section class="nav-section">
      <div class="nav-head">
        <span class="nav-title">常用位置</span>
      </div>
      <div v-if="locLoading" class="state">加载中…</div>
      <div v-else-if="locError" class="state err">系统路径不可用：{{ locError }}</div>
      <ul v-else class="nav-list">
        <li v-for="loc in system" :key="loc.path" :title="loc.path" @click="go(loc.path)">
          <span class="ico"><Icon :name="loc.icon" :size="14" /></span>
          <span class="label">{{ loc.label }}</span>
        </li>
      </ul>
    </section>

    <!-- 此电脑 -->
    <section class="nav-section">
      <div class="nav-head">
        <span class="nav-title">此电脑</span>
        <button class="mini" title="刷新驱动器" :disabled="loading" @click="loadRoots">
          <Icon name="refresh" :size="13" />
        </button>
      </div>
      <div v-if="loading && !roots.length" class="state">正在读取驱动器…</div>
      <div v-else-if="error" class="state err">驱动器读取失败：{{ error }}</div>
      <div v-else-if="!roots.length" class="state">未检测到驱动器</div>
      <div v-else class="tree-body">
        <TreeNode
          v-for="root in roots"
          :key="root.path"
          :node="root"
          :depth="0"
          @navigate="go"
        />
      </div>
    </section>

    <!-- 收藏 -->
    <section class="nav-section">
      <div class="nav-head">
        <span class="nav-title">收藏</span>
        <button class="mini" title="将当前目录加入收藏" @click="addCurrent">
          <Icon name="star" :size="13" />
        </button>
      </div>
      <div v-if="!favorites.length" class="state">暂无收藏，点击上方星标添加当前目录</div>
      <ul v-else class="nav-list">
        <li
          v-for="fav in favorites"
          :key="fav.path"
          :title="fav.path"
          @click="go(fav.path)"
        >
          <span class="ico"><Icon :name="fav.icon" :size="14" /></span>
          <span class="label">{{ fav.label }}</span>
          <button class="del" title="移除收藏" @click.stop="removeFavorite(fav.path)">
            <Icon name="x" :size="11" />
          </button>
        </li>
      </ul>
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
  font-size: 11px;
  color: var(--fg-muted);
  letter-spacing: 0.04em;
}
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
.ico {
  flex: none;
  display: flex;
  align-items: center;
  color: var(--fg-muted);
}
.label {
  flex: 1;
  font-size: 12px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
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
  font-size: 11px;
  color: var(--fg-dim);
}
.state.err {
  color: var(--danger);
}
</style>

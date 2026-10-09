<script setup lang="ts">
// 快速访问：系统常用位置 + 用户收藏（localStorage 持久化）
import { onMounted, ref } from 'vue'
import { usePaneStore } from '../../stores/pane'
import { useUiStore } from '../../stores/ui'
import { basename, loadFavorites, resolveSystemLocations, saveFavorites } from './shared'
import type { QuickLocation } from './shared'

const pane = usePaneStore()
const ui = useUiStore()

const system = ref<QuickLocation[]>([])
const favorites = ref<QuickLocation[]>([])
const loading = ref(false)
const error = ref<string | null>(null)

async function init() {
  loading.value = true
  error.value = null
  try {
    system.value = await resolveSystemLocations()
  } catch (e) {
    error.value = String(e)
    system.value = []
  } finally {
    loading.value = false
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
    { label: basename(path) || path, icon: '⭐', path },
  ]
  saveFavorites(favorites.value)
}

function removeFavorite(path: string) {
  favorites.value = favorites.value.filter((f) => f.path !== path)
  saveFavorites(favorites.value)
}

onMounted(init)
</script>

<template>
  <div class="quick-access">
    <div class="qa-toolbar">
      <button class="add-btn" title="将当前目录加入收藏" @click="addCurrent">
        ＋ 收藏当前目录
      </button>
    </div>

    <section class="qa-section">
      <h4 class="qa-head">常用位置</h4>
      <div v-if="loading" class="state">加载中…</div>
      <div v-else-if="error" class="state err">系统路径不可用：{{ error }}</div>
      <ul v-else class="qa-list">
        <li v-for="loc in system" :key="loc.path" :title="loc.path" @click="go(loc.path)">
          <span class="ico">{{ loc.icon }}</span>
          <span class="label">{{ loc.label }}</span>
        </li>
      </ul>
    </section>

    <section class="qa-section">
      <h4 class="qa-head">收藏</h4>
      <div v-if="!favorites.length" class="state">暂无收藏，点击上方按钮添加当前目录</div>
      <ul v-else class="qa-list">
        <li
          v-for="fav in favorites"
          :key="fav.path"
          :title="fav.path"
          @click="go(fav.path)"
        >
          <span class="ico">{{ fav.icon }}</span>
          <span class="label">{{ fav.label }}</span>
          <button class="del" title="移除收藏" @click.stop="removeFavorite(fav.path)">×</button>
        </li>
      </ul>
    </section>
  </div>
</template>

<style scoped>
.quick-access {
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.qa-toolbar {
  display: flex;
}
.add-btn {
  width: 100%;
  height: 26px;
  border-radius: var(--radius);
  border: 1px dashed var(--border-strong);
  color: var(--fg-muted);
  font-size: 12px;
}
.add-btn:hover {
  background: var(--bg-hover);
  color: var(--fg);
  border-color: var(--accent);
}
.qa-section {
  display: flex;
  flex-direction: column;
}
.qa-head {
  margin: 4px 0;
  font-size: 11px;
  font-weight: 600;
  color: var(--fg-muted);
  letter-spacing: 0.04em;
}
.qa-list {
  list-style: none;
  margin: 0;
  padding: 0;
}
.qa-list li {
  display: flex;
  align-items: center;
  gap: 6px;
  height: 24px;
  padding: 0 6px;
  border-radius: var(--radius);
  cursor: pointer;
  overflow: hidden;
}
.qa-list li:hover {
  background: var(--bg-hover);
}
.ico {
  flex: none;
  font-size: 13px;
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
  line-height: 1;
  border-radius: var(--radius);
  color: var(--fg-dim);
  font-size: 13px;
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

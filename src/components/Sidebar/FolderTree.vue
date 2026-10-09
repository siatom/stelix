<script setup lang="ts">
// 文件导航面板：驱动器 + 文件夹懒加载树
import { onMounted, ref } from 'vue'
import { api } from '../../api'
import { usePaneStore } from '../../stores/pane'
import { useUiStore } from '../../stores/ui'
import TreeNode from './TreeNode.vue'
import type { TreeNodeData } from './shared'

const pane = usePaneStore()
const ui = useUiStore()

const roots = ref<TreeNodeData[]>([])
const loading = ref(false)
const error = ref<string | null>(null)

async function loadRoots() {
  loading.value = true
  error.value = null
  try {
    const drives = await api.getRoots()
    roots.value = drives.map((d) => ({
      name: d.name ? `${d.name} (${d.mount})` : d.mount,
      path: d.mount,
    }))
  } catch (e) {
    error.value = String(e)
    roots.value = []
  } finally {
    loading.value = false
  }
}

function onNavigate(path: string) {
  ui.setStatusText(`导航到 ${path}`)
  void pane.navigate(path)
}

onMounted(loadRoots)
</script>

<template>
  <div class="folder-tree">
    <div class="tree-toolbar">
      <span class="tree-title">此电脑</span>
      <button class="mini" title="刷新驱动器" :disabled="loading" @click="loadRoots">⟳</button>
    </div>

    <div v-if="loading && !roots.length" class="state">正在读取驱动器…</div>
    <div v-else-if="error" class="state err">驱动器读取失败：{{ error }}</div>
    <div v-else-if="!roots.length" class="state">未检测到驱动器</div>

    <div class="tree-body">
      <TreeNode
        v-for="root in roots"
        :key="root.path"
        :node="root"
        :depth="0"
        @navigate="onNavigate"
      />
    </div>
  </div>
</template>

<style scoped>
.folder-tree {
  display: flex;
  flex-direction: column;
  height: 100%;
}
.tree-toolbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 2px 4px 6px;
  border-bottom: 1px solid var(--border);
  margin-bottom: 4px;
}
.tree-title {
  font-size: 11px;
  color: var(--fg-muted);
  letter-spacing: 0.04em;
}
.mini {
  width: 20px;
  height: 20px;
  border-radius: var(--radius);
  color: var(--fg-muted);
  font-size: 12px;
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
  flex: 1;
  overflow: auto;
}
.state {
  padding: 6px;
  font-size: 12px;
  color: var(--fg-dim);
}
.state.err {
  color: var(--danger);
}
</style>

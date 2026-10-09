<script setup lang="ts">
// 目录树节点（自引用递归组件）—— 懒加载子目录
import { ref } from 'vue'
import { api } from '../../api'
import { useUiStore } from '../../stores/ui'
import type { TreeNodeData } from './shared'
import Icon from '../icons/Icon.vue'
import { folderIcon } from '../Pane/utils'

const props = defineProps<{ node: TreeNodeData; depth: number }>()
const emit = defineEmits<{ (e: 'navigate', path: string): void }>()

const ui = useUiStore()

const expanded = ref(false)
const loaded = ref(false)
const loading = ref(false)
const error = ref<string | null>(null)
const children = ref<TreeNodeData[]>([])

async function toggle() {
  if (expanded.value) {
    expanded.value = false
    return
  }
  expanded.value = true
  if (loaded.value) return
  loading.value = true
  error.value = null
  try {
    const listing = await api.listDir(props.node.path)
    children.value = listing.entries
      .filter((e) => e.isDir)
      // 「隐藏受保护的操作系统文件」（hidden && system）时同样过滤子树目录
      .filter((e) => !(ui.hideProtected && e.hidden && e.system))
      .sort((a, b) => a.name.localeCompare(b.name))
      .map((e) => ({ name: e.name, path: e.path }))
    loaded.value = true
  } catch (e) {
    error.value = String(e)
    children.value = []
  } finally {
    loading.value = false
  }
}

const pad = (d: number) => `${d * 12 + 6}px`
const childPad = (d: number) => `${d * 12 + 22}px`
</script>

<template>
  <div class="tree-node">
    <div
      class="row"
      :style="{ paddingLeft: pad(depth) }"
      :title="node.path"
      @click="emit('navigate', node.path)"
    >
      <span class="twist" :class="{ open: expanded }" @click.stop="toggle">
        <Icon name="chevron-right" :size="11" />
      </span>
      <span class="ico"><Icon :name="folderIcon(expanded)" :size="14" /></span>
      <span class="label">{{ node.name }}</span>
    </div>

    <div v-if="expanded">
      <div v-if="loading" class="state" :style="{ paddingLeft: childPad(depth + 1) }">
        加载中…
      </div>
      <div v-else-if="error" class="state err" :style="{ paddingLeft: childPad(depth + 1) }">
        无法读取：{{ error }}
      </div>
      <TreeNode
        v-for="child in children"
        :key="child.path"
        :node="child"
        :depth="depth + 1"
        @navigate="emit('navigate', $event)"
      />
    </div>
  </div>
</template>

<style scoped>
.row {
  display: flex;
  align-items: center;
  gap: 4px;
  height: 22px;
  padding-right: 6px;
  border-radius: var(--radius);
  cursor: pointer;
  white-space: nowrap;
  overflow: hidden;
}
.row:hover {
  background: var(--bg-hover);
}
.twist {
  width: 12px;
  flex: none;
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--fg-dim);
  transition: transform 0.12s ease;
}
.twist:hover {
  color: var(--fg);
}
.twist.open {
  transform: rotate(90deg);
}
.ico {
  flex: none;
  display: flex;
  align-items: center;
  color: var(--accent);
}
.label {
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  font-size: 12px;
  color: var(--fg-folder);
}
.state {
  height: 20px;
  display: flex;
  align-items: center;
  font-size: 11px;
  color: var(--fg-dim);
}
.state.err {
  color: var(--danger);
}
</style>

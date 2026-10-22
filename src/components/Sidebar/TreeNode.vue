<script setup lang="ts">
// 目录树节点（自引用递归组件）—— 懒加载子目录
import { onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { api } from '../../api'
import { useUiStore } from '../../stores/ui'
import type { FileEntry } from '../../types'
import type { TreeNodeData } from './shared'
import { clearWatchSource, FS_CHANGED_EVENT, normalizePath, setWatchSource } from '../../shell/dirWatch'
import { compareNames } from '../../naturalSort'
import Icon from '../icons/Icon.vue'
import FileIcon from '../icons/FileIcon.vue'
import { folderIcon } from '../Pane/utils'

const props = defineProps<{ node: TreeNodeData; depth: number }>()
const emit = defineEmits<{ (e: 'navigate', path: string): void }>()

const ui = useUiStore()

const isDrive = props.depth === 0 && /^[A-Za-z]:[\\/]?$/.test(props.node.path)

const expanded = ref(false)
const loaded = ref(false)
const loading = ref(false)
const error = ref<string | null>(null)
const children = ref<TreeNodeData[]>([])

/** 本节点在监听注册表里的来源 id（展开时上报自己的路径）。 */
const WATCH_SOURCE = `tree:${props.node.path}`

function toNodes(entries: FileEntry[]): TreeNodeData[] {
  return (
    entries
      .filter((e) => e.isDir)
      // 「隐藏受保护的操作系统文件」（hidden && system）时同样过滤子树目录
      .filter((e) => !(ui.hideProtected && e.hidden && e.system))
      .sort((a, b) => compareNames(a.name, b.name))
      .map((e) => ({ name: e.name, path: e.path }))
  )
}

async function loadChildren(showLoading: boolean) {
  if (showLoading) loading.value = true
  error.value = null
  try {
    const listing = await api.listDir(props.node.path)
    children.value = toNodes(listing.entries)
    loaded.value = true
  } catch (e) {
    error.value = String(e)
    children.value = []
  } finally {
    loading.value = false
  }
}

async function toggle() {
  if (expanded.value) {
    expanded.value = false
    return
  }
  expanded.value = true
  if (loaded.value) return
  await loadChildren(true)
}

// 只有展开的节点才需要后端监听其目录。
watch(expanded, (open) => {
  if (open) setWatchSource(WATCH_SOURCE, [props.node.path])
  else clearWatchSource(WATCH_SOURCE)
})

/** 该目录内容变化时静默刷新子项（保持展开态；子节点按 path 作 key 会保留各自展开状态）。 */
function onFsChanged(event: Event) {
  if (!expanded.value) return
  const dirs = (event as CustomEvent<string[]>).detail ?? []
  const self = normalizePath(props.node.path)
  if (!dirs.some((dir) => normalizePath(dir) === self)) return
  void loadChildren(false)
}

onMounted(() => window.addEventListener(FS_CHANGED_EVENT, onFsChanged))
onBeforeUnmount(() => {
  window.removeEventListener(FS_CHANGED_EVENT, onFsChanged)
  clearWatchSource(WATCH_SOURCE)
})

const pad = (d: number) => `${d * 12 + 6}px`
const childPad = (d: number) => `${d * 12 + 22}px`
</script>

<template>
  <div class="tree-node">
    <div
      class="row"
      :class="`depth-${Math.min(depth, 4)}`"
      :style="{ paddingLeft: pad(depth) }"
      :title="node.path"
      @click="emit('navigate', node.path)"
    >
      <span class="twist" :class="{ open: expanded }" @click.stop="toggle">
        <Icon name="chevron-right" :size="11" />
      </span>
      <span class="ico">
        <FileIcon
          v-if="isDrive"
          :path="node.path"
          :is-dir="false"
          :is-drive="true"
          :ext="null"
          fallback="drive"
          :size="18"
        />
        <FileIcon
          v-else
          :path="node.path"
          :is-dir="true"
          :ext="null"
          :use-path-icon="true"
          :fallback="folderIcon(expanded)"
          :size="18"
        />
      </span>
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
  height: var(--sidebar-row-h);
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
  color: var(--tree-icon-color, var(--ico-folder));
}
.row.depth-0 { --tree-icon-color: var(--ico-folder); }
.row.depth-1 { --tree-icon-color: #f2bd63; }
.row.depth-2 { --tree-icon-color: #56c7b0; }
.row.depth-3 { --tree-icon-color: #b890eb; }
.row.depth-4 { --tree-icon-color: #e7849b; }
.label {
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  font-size: 1em;
  color: var(--fg-folder);
}
.state {
  height: 20px;
  display: flex;
  align-items: center;
  font-size: 0.9167em;
  color: var(--fg-dim);
}
.state.err {
  color: var(--danger);
}
</style>

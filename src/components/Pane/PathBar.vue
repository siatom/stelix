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

onMounted(() => window.addEventListener(FAVORITES_CHANGED_EVENT, syncFavorite))
onBeforeUnmount(() => window.removeEventListener(FAVORITES_CHANGED_EVENT, syncFavorite))

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
      class="crumbs"
      :title="isRemote ? '远程路径（只读）' : '双击切换为可编辑路径'"
      @dblclick.stop.prevent="startEdit"
    >
      <template v-for="(seg, i) in segments" :key="seg.path">
        <span v-if="i > 0" class="sep"><Icon name="chevron-right" :size="15" /></span>
        <button class="crumb" :class="{ last: i === segments.length - 1 }" @click="go(seg.path)">
          {{ seg.label }}
        </button>
      </template>
    </div>
    <input
      v-else
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
  display: flex;
  align-items: center;
  gap: 6px;
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
  gap: 6px;
  min-width: 0;
  overflow-x: auto;
  white-space: nowrap;
  height: 22px;
}
.crumbs::-webkit-scrollbar {
  height: 0;
}
.crumb {
  padding: 2px 6px;
  border-radius: var(--radius);
  color: var(--fg-muted);
  font-size: inherit;
}
.crumb:hover {
  background: var(--bg-hover);
  color: var(--fg);
}
.crumb.last {
  color: var(--fg);
}
.sep {
  display: flex;
  align-items: center;
  color: var(--fg-dim);
}
.path-input {
  flex: 1;
  height: 22px;
  min-width: 0;
  font-family: "Microsoft YaHei", "微软雅黑", sans-serif;
  font-size: inherit;
}
</style>

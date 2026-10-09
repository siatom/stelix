<script setup lang="ts">
// 面包屑地址栏：点击各级跳转，右侧切换到可编辑输入框（WS-D）
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import type { TabState } from '../../types'
import { usePaneStore } from '../../stores/pane'
import { joinPath } from './utils'
import Icon from '../icons/Icon.vue'
import { addFavorite, FAVORITES_CHANGED_EVENT, loadFavorites } from '../Sidebar/shared'
import { useUiStore } from '../../stores/ui'

const props = defineProps<{ paneId: string; tab: TabState }>()
const paneStore = usePaneStore()
const ui = useUiStore()
const isFavorite = ref(loadFavorites().some((item) => item.path === props.tab.path))

const editing = ref(false)
const draft = ref('')
const inputRef = ref<HTMLInputElement | null>(null)

const segments = computed(() => {
  const norm = props.tab.path.replace(/\//g, '\\')
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
  draft.value = props.tab.path
  editing.value = true
  await nextTick()
  inputRef.value?.focus()
  inputRef.value?.select()
}

function commit() {
  if (!editing.value) return
  const value = draft.value.trim()
  editing.value = false
  if (value && value !== props.tab.path) go(value)
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
    <div v-if="!editing" class="crumbs" title="双击切换为可编辑路径" @dblclick.stop.prevent="startEdit">
      <template v-for="(seg, i) in segments" :key="seg.path">
        <span v-if="i > 0" class="sep"><Icon name="chevron-right" :size="11" /></span>
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
      @keyup.enter="commit"
      @keyup.esc="cancel"
      @blur="commit"
    />
    <button
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
  gap: 4px;
  height: var(--toolbar-h);
  padding: 0 6px;
  background: var(--bg-panel);
  border-bottom: 1px solid var(--border);
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
  gap: 1px;
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
  font-size: 12px;
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
  font-size: 12px;
}
</style>

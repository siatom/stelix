<script setup lang="ts">
// 面包屑地址栏：点击各级跳转，右侧切换到可编辑输入框（WS-D）
import { computed, nextTick, ref, watch } from 'vue'
import type { TabState } from '../../types'
import { usePaneStore } from '../../stores/pane'
import { joinPath } from './utils'
import Icon from '../icons/Icon.vue'

const props = defineProps<{ paneId: string; tab: TabState }>()
const paneStore = usePaneStore()

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
  },
)
</script>

<template>
  <div class="path-bar">
    <button class="up" title="上级目录" @click="paneStore.goUp(paneId)">
      <Icon name="arrow-up" :size="14" />
    </button>
    <div v-if="!editing" class="crumbs" title="双击切换为可编辑路径">
      <template v-for="(seg, i) in segments" :key="seg.path">
        <span v-if="i > 0" class="sep"><Icon name="chevron-right" :size="11" /></span>
        <button class="crumb" :class="{ last: i === segments.length - 1 }" @click="go(seg.path)">
          {{ seg.label }}
        </button>
      </template>
      <button class="edit-toggle" title="编辑路径" @click="startEdit">
        <Icon name="pencil" :size="13" />
      </button>
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
.up {
  flex: none;
  width: 24px;
  height: 22px;
  display: flex;
  align-items: center;
  justify-content: center;
  border-radius: var(--radius);
  color: var(--fg-muted);
}
.up:hover {
  background: var(--bg-hover);
  color: var(--fg);
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
.edit-toggle {
  flex: none;
  margin-left: auto;
  display: flex;
  align-items: center;
  padding: 2px 6px;
  color: var(--fg-dim);
}
.edit-toggle:hover {
  color: var(--fg);
}
.path-input {
  flex: 1;
  height: 22px;
  min-width: 0;
  font-family: var(--mono);
  font-size: 12px;
}
</style>

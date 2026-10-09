<script setup lang="ts">
// Ctrl+K 命令面板 —— 承载原顶部菜单的全部能力
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { api } from '../api'
import { usePaneStore } from '../stores/pane'
import { useUiStore } from '../stores/ui'
import { resolveSystemLocations } from './Sidebar/shared'
import type { QuickLocation } from './Sidebar/shared'

interface Command {
  id: string
  label: string
  hint?: string
  run: () => void | Promise<void>
}

const pane = usePaneStore()
const ui = useUiStore()

const open = ref(false)
const query = ref('')
const activeIndex = ref(0)
const commands = ref<Command[]>([])
const inputEl = ref<HTMLInputElement | null>(null)
const listEl = ref<HTMLElement | null>(null)

const filtered = computed(() => {
  const q = query.value.trim().toLowerCase()
  if (!q) return commands.value
  return commands.value.filter((c) => `${c.label} ${c.hint ?? ''}`.toLowerCase().includes(q))
})

function close() {
  open.value = false
}

async function openPalette() {
  query.value = ''
  activeIndex.value = 0
  open.value = true
  await nextTick()
  inputEl.value?.focus()
}

function toggle() {
  if (open.value) close()
  else void openPalette()
}

async function focusSearch() {
  close()
  await nextTick()
  const el = document.querySelector<HTMLInputElement>('.search-bar input')
  el?.focus()
  el?.select()
}

async function reindex() {
  try {
    await api.startIndex(ui.indexRoots)
    ui.setStatusText(`已重新索引 ${ui.indexRoots.length} 个根目录`)
  } catch (e) {
    ui.setStatusText(`重新索引失败：${String(e)}`)
  }
}

function buildCommands(locations: QuickLocation[]) {
  const list: Command[] = [
    {
      id: 'new-tab',
      label: '新建标签页',
      hint: 'Ctrl+T',
      run: () => pane.addTab(pane.activePaneId),
    },
    {
      id: 'close-tab',
      label: '关闭当前标签页',
      hint: 'Ctrl+W',
      run: () => {
        const tab = pane.activeTab
        if (tab) pane.closeTab(pane.activePaneId, tab.id)
      },
    },
    { id: 'toggle-ai', label: '切换 AI 助手面板', run: () => ui.toggleAi() },
    { id: 'toggle-sidebar', label: '切换侧边栏', run: () => ui.toggleSidebar() },
    { id: 'focus-search', label: '聚焦全局搜索', hint: '/', run: focusSearch },
    { id: 'reindex', label: '重新建立索引', run: reindex },
  ]
  for (const loc of locations) {
    list.push({
      id: `nav-${loc.path}`,
      label: `导航到${loc.label}`,
      hint: loc.path,
      run: () => {
        ui.setStatusText(`导航到 ${loc.path}`)
        void pane.navigate(loc.path)
      },
    })
  }
  commands.value = list
}

function move(delta: number) {
  const n = filtered.value.length
  if (!n) return
  activeIndex.value = (activeIndex.value + delta + n) % n
}

async function choose() {
  const cmd = filtered.value[activeIndex.value]
  if (!cmd) return
  close()
  try {
    await cmd.run()
  } catch (e) {
    ui.setStatusText(`命令执行失败：${String(e)}`)
  }
}

async function runAt(index: number) {
  activeIndex.value = index
  await choose()
}

function onKeydown(e: KeyboardEvent) {
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'k') {
    e.preventDefault()
    toggle()
    return
  }
  if (e.key === 'Escape' && open.value) {
    e.preventDefault()
    close()
  }
}

watch(query, () => {
  activeIndex.value = 0
})

watch(activeIndex, async () => {
  await nextTick()
  listEl.value?.querySelector<HTMLElement>('.palette-item.active')?.scrollIntoView({ block: 'nearest' })
})

onMounted(async () => {
  window.addEventListener('keydown', onKeydown)
  buildCommands(await resolveSystemLocations())
})

onUnmounted(() => {
  window.removeEventListener('keydown', onKeydown)
})
</script>

<template>
  <div v-if="open" class="palette-mask" @click.self="close">
    <div class="palette" role="dialog" aria-label="命令面板">
      <input
        ref="inputEl"
        v-model="query"
        class="palette-input"
        placeholder="输入命令…（↑↓ 选择，Enter 执行，Esc 关闭）"
        @keydown.down.prevent="move(1)"
        @keydown.up.prevent="move(-1)"
        @keydown.enter.prevent="choose"
      />
      <ul v-if="filtered.length" ref="listEl" class="palette-list">
        <li
          v-for="(cmd, i) in filtered"
          :key="cmd.id"
          class="palette-item"
          :class="{ active: i === activeIndex }"
          @mouseenter="activeIndex = i"
          @click="runAt(i)"
        >
          <span class="cmd-label">{{ cmd.label }}</span>
          <span v-if="cmd.hint" class="cmd-hint">{{ cmd.hint }}</span>
        </li>
      </ul>
      <div v-else class="palette-empty">无匹配命令</div>
    </div>
  </div>
</template>

<style scoped>
.palette-mask {
  position: fixed;
  inset: 0;
  background: var(--mask);
  display: flex;
  justify-content: center;
  padding-top: 12vh;
  z-index: 100;
}
.palette {
  width: 560px;
  max-width: 90vw;
  max-height: 60vh;
  display: flex;
  flex-direction: column;
  background: var(--bg-elevated);
  border: 1px solid var(--border-strong);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-lg);
  overflow: hidden;
}
.palette-input {
  width: 100%;
  border: none;
  border-bottom: 1px solid var(--border);
  border-radius: 0;
  padding: 10px 12px;
  font-size: 14px;
  background: var(--bg-input);
}
.palette-input:focus {
  border-color: var(--border);
}
.palette-list {
  list-style: none;
  margin: 0;
  padding: 6px;
  overflow: auto;
}
.palette-item {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 7px 10px;
  border-radius: var(--radius);
  cursor: pointer;
}
.palette-item.active {
  background: var(--bg-selected);
}
.cmd-label {
  font-size: 13px;
}
.cmd-hint {
  font-size: 11px;
  color: var(--fg-dim);
  font-family: var(--mono);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  max-width: 55%;
}
.palette-empty {
  padding: 14px;
  text-align: center;
  font-size: 12px;
  color: var(--fg-dim);
}
</style>

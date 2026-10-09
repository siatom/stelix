<script setup lang="ts">
// 标签页组（工作区）：把两个窗格的标签路径组合保存，可一键恢复
import { onMounted, ref } from 'vue'
import { usePaneStore } from '../../stores/pane'
import { useUiStore } from '../../stores/ui'
import { loadTabGroups, saveTabGroups } from './shared'
import type { TabGroup } from './shared'
import Icon from '../icons/Icon.vue'

const pane = usePaneStore()
const ui = useUiStore()

const groups = ref<TabGroup[]>([])

function snapshot(): TabGroup {
  const panes = pane.panes.map((p) => ({
    tabs: p.tabs.map((t) => t.path),
    active: Math.max(
      0,
      p.tabs.findIndex((t) => t.id === p.activeTabId),
    ),
  }))
  return {
    id: `grp-${Date.now()}`,
    name: `工作区 ${groups.value.length + 1}`,
    panes,
    activePaneId: pane.activePaneId,
    createdAt: Date.now(),
  }
}

function saveCurrent() {
  const g = snapshot()
  groups.value = [g, ...groups.value].slice(0, 20)
  saveTabGroups(groups.value)
  ui.setStatusText(`已保存工作区「${g.name}」`)
}

async function restore(g: TabGroup) {
  const paneIds = pane.panes.map((p) => p.id)
  for (let i = 0; i < paneIds.length; i++) {
    const paneId = paneIds[i]
    const gp = g.panes[i]
    if (!gp || !gp.tabs.length) continue
    pane.activePaneId = paneId
    const target = pane.panes.find((p) => p.id === paneId)
    if (!target || !target.tabs.length) continue

    // 仅保留一个标签，关闭其余，保证后续 addTab 结果可控
    const keepId = target.tabs[0].id
    for (const t of [...target.tabs]) {
      if (t.id !== keepId) pane.closeTab(paneId, t.id)
    }

    const activeIdx = Math.min(Math.max(gp.active, 0), gp.tabs.length - 1)
    const activePath = gp.tabs[activeIdx]
    const others = gp.tabs.filter((_, idx) => idx !== activeIdx)
    const firstPath = others.length ? others[0] : activePath

    await pane.navigate(firstPath)
    for (let k = 1; k < others.length; k++) pane.addTab(paneId, others[k])
    // 目标标签最后添加 → 成为活动标签
    if (others.length) pane.addTab(paneId, activePath)
  }

  if (g.activePaneId && paneIds.includes(g.activePaneId)) {
    pane.activePaneId = g.activePaneId
  }
  ui.setStatusText(`已恢复工作区「${g.name}」`)
}

function remove(id: string) {
  groups.value = groups.value.filter((g) => g.id !== id)
  saveTabGroups(groups.value)
}

function formatTime(ts: number): string {
  return new Date(ts).toLocaleString('zh-CN', { hour12: false })
}

onMounted(() => {
  groups.value = loadTabGroups()
})
</script>

<template>
  <div class="tab-groups">
    <div class="tg-toolbar">
      <button class="save-btn" title="保存当前双窗格标签布局" @click="saveCurrent">
        <Icon name="plus" :size="13" />
        <span>保存当前布局为工作区</span>
      </button>
    </div>

    <div v-if="!groups.length" class="state">暂无工作区，点击上方按钮保存当前布局</div>

    <ul v-else class="tg-list">
      <li v-for="g in groups" :key="g.id" class="tg-item">
        <div class="tg-main">
          <div class="tg-name">{{ g.name }}</div>
          <div class="tg-meta">
            <div v-for="(p, i) in g.panes" :key="i" class="tg-pane" :title="p.tabs.join('\n')">
              <span class="pane-tag">{{ pane.panes[i]?.id ?? i }}</span>
              <span class="pane-paths">{{ p.tabs.length }} 个标签 · {{ p.tabs[p.active] }}</span>
            </div>
          </div>
          <div class="tg-time">{{ formatTime(g.createdAt) }}</div>
        </div>
        <div class="tg-actions">
          <button class="act" title="恢复此工作区" @click="restore(g)">恢复</button>
          <button class="act del" title="删除" @click="remove(g.id)">删除</button>
        </div>
      </li>
    </ul>
  </div>
</template>

<style scoped>
.tab-groups {
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.save-btn {
  width: 100%;
  height: 26px;
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 5px;
  border-radius: var(--radius);
  border: 1px dashed var(--border-strong);
  color: var(--fg-muted);
  font-size: 12px;
}
.save-btn:hover {
  background: var(--bg-hover);
  color: var(--fg);
  border-color: var(--accent);
}
.tg-list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.tg-item {
  display: flex;
  gap: 6px;
  padding: 6px;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--bg-elevated);
}
.tg-main {
  flex: 1;
  min-width: 0;
}
.tg-name {
  font-size: 12px;
  font-weight: 600;
  color: var(--fg);
}
.tg-meta {
  margin-top: 3px;
  display: flex;
  flex-direction: column;
  gap: 2px;
}
.tg-pane {
  display: flex;
  gap: 4px;
  font-size: 11px;
  color: var(--fg-muted);
  white-space: nowrap;
  overflow: hidden;
}
.pane-tag {
  flex: none;
  color: var(--accent);
}
.pane-paths {
  overflow: hidden;
  text-overflow: ellipsis;
}
.tg-time {
  margin-top: 4px;
  font-size: 10px;
  color: var(--fg-dim);
}
.tg-actions {
  display: flex;
  flex-direction: column;
  gap: 4px;
  flex: none;
}
.act {
  padding: 2px 8px;
  border-radius: var(--radius);
  font-size: 11px;
  border: 1px solid var(--border);
  color: var(--fg-muted);
}
.act:hover {
  background: var(--bg-hover);
  color: var(--fg);
}
.act.del:hover {
  border-color: var(--danger);
  color: var(--danger);
}
.state {
  padding: 4px 6px;
  font-size: 11px;
  color: var(--fg-dim);
}
</style>

<script setup lang="ts">
// SSH 终端面板：底部可拖拽调整高度，多会话标签并存（支持同时连接多台服务器）。
import { computed, onBeforeUnmount } from 'vue'
import { useTerminalStore } from '../../stores/terminal'
import type { TerminalSession } from '../../stores/terminal'
import TerminalView from './TerminalView.vue'
import Icon from '../icons/Icon.vue'

const store = useTerminalStore()

const sessions = computed(() => store.sessions)
const showDock = computed(() => store.visible && sessions.value.length > 0)

const statusText: Record<TerminalSession['status'], string> = {
  connecting: '连接中',
  connected: '已连接',
  closed: '已断开',
  error: '连接失败',
}

let resizing = false

function onResizeMove(event: MouseEvent) {
  if (!resizing) return
  store.setDockHeight(window.innerHeight - event.clientY)
}

function stopResize() {
  resizing = false
  window.removeEventListener('mousemove', onResizeMove)
  window.removeEventListener('mouseup', stopResize)
}

function startResize(event: MouseEvent) {
  event.preventDefault()
  resizing = true
  window.addEventListener('mousemove', onResizeMove)
  window.addEventListener('mouseup', stopResize)
}

function resetHeight() {
  store.setDockHeight(280)
}

onBeforeUnmount(stopResize)
</script>

<template>
  <section
    v-if="showDock"
    class="terminal-dock"
    :style="{ height: `${store.dockHeight}px` }"
    aria-label="SSH 终端"
  >
    <div class="terminal-resize" title="拖动调整终端高度，双击恢复默认" @mousedown="startResize" @dblclick="resetHeight" />
    <header class="terminal-tabs">
      <button
        v-for="session in sessions"
        :key="session.id"
        type="button"
        class="terminal-tab"
        :class="{ active: store.activeId === session.id }"
        :title="`${session.location.username}@${session.location.host}:${session.location.port}${
          session.fingerprint ? `\n主机密钥 ${session.fingerprint}` : ''
        }`"
        @click="store.setActive(session.id)"
      >
        <span class="status-dot" :class="session.status" />
        <Icon name="terminal" :size="13" />
        <span class="tab-title">{{ session.title }}</span>
        <span class="tab-status">{{ statusText[session.status] }}</span>
        <span
          class="tab-close"
          role="button"
          :aria-label="`关闭 ${session.title}`"
          title="关闭终端"
          @click.stop="store.close(session.id, session.status !== 'closed')"
        >
          <Icon name="x" :size="11" />
        </span>
      </button>
      <div class="tabs-spacer" />
      <button class="dock-action" title="收起终端面板" aria-label="收起终端面板" @click="store.setVisible(false)">
        <Icon name="chevron-down" :size="14" />
      </button>
    </header>
    <div class="terminal-body">
      <TerminalView
        v-for="session in sessions"
        v-show="store.activeId === session.id"
        :key="session.id"
        :session="session"
        :active="store.activeId === session.id"
      />
    </div>
  </section>

  <button
    v-if="sessions.length && !store.visible"
    type="button"
    class="terminal-reopen"
    :title="`展开终端（${sessions.length} 个会话）`"
    aria-label="展开终端面板"
    @click="store.setVisible(true)"
  >
    <Icon name="terminal" :size="15" />
    <span>{{ sessions.length }}</span>
  </button>
</template>

<style scoped>
.terminal-dock {
  position: relative;
  flex: none;
  display: flex;
  flex-direction: column;
  min-height: 0;
  border-top: 1px solid var(--border);
  background: var(--bg-panel);
}
.terminal-resize {
  position: absolute;
  top: -4px;
  left: 0;
  right: 0;
  z-index: 3;
  height: 8px;
  cursor: row-resize;
}
.terminal-resize:hover {
  background: color-mix(in srgb, var(--accent) 22%, transparent);
}
.terminal-tabs {
  display: flex;
  align-items: stretch;
  flex: none;
  height: 34px;
  padding: 0 6px;
  gap: 2px;
  overflow-x: auto;
  overflow-y: hidden;
  border-bottom: 1px solid var(--border);
  scrollbar-width: thin;
}
.terminal-tab {
  display: flex;
  align-items: center;
  gap: 6px;
  flex: none;
  max-width: 240px;
  min-width: 120px;
  padding: 0 6px 0 9px;
  border-radius: 6px 6px 0 0;
  color: var(--fg-muted);
  font-size: 0.9167rem;
  white-space: nowrap;
}
.terminal-tab:hover {
  background: var(--bg-hover);
  color: var(--fg);
}
.terminal-tab.active {
  background: var(--bg-app);
  color: var(--fg);
}
.tab-title {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
}
.tab-status {
  flex: none;
  color: var(--fg-dim);
  font-size: 0.75rem;
}
.status-dot {
  flex: none;
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--fg-dim);
}
.status-dot.connecting {
  background: var(--accent);
  animation: terminal-pulse 1s ease-in-out infinite;
}
.status-dot.connected {
  background: #3fb950;
}
.status-dot.error {
  background: var(--danger);
}
@keyframes terminal-pulse {
  50% {
    opacity: 0.35;
  }
}
.tab-close {
  display: flex;
  align-items: center;
  justify-content: center;
  flex: none;
  width: 17px;
  height: 17px;
  border-radius: 4px;
  color: var(--fg-dim);
}
.tab-close:hover {
  background: var(--bg-active);
  color: var(--fg);
}
.tabs-spacer {
  flex: 1;
}
.dock-action {
  display: flex;
  align-items: center;
  justify-content: center;
  flex: none;
  width: 26px;
  border-radius: 5px;
  color: var(--fg-dim);
}
.dock-action:hover {
  background: var(--bg-hover);
  color: var(--fg);
}
.terminal-body {
  flex: 1;
  min-height: 0;
  display: flex;
  overflow: hidden;
}
.terminal-reopen {
  position: absolute;
  right: 16px;
  bottom: 14px;
  z-index: 30;
  display: flex;
  align-items: center;
  gap: 4px;
  height: 30px;
  padding: 0 10px;
  border: 1px solid var(--border-strong);
  border-radius: 999px;
  background: var(--bg-elevated);
  box-shadow: var(--shadow-md);
  color: var(--fg-muted);
  font-size: 0.8333rem;
}
.terminal-reopen:hover {
  color: var(--fg);
  border-color: var(--accent);
}
</style>

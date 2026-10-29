<script setup lang="ts">
// 单个 SSH 终端实例（xterm.js）：负责渲染、键盘输入与尺寸同步。
import { onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { Terminal } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import { useTerminalStore } from '../../stores/terminal'
import type { TerminalSession } from '../../stores/terminal'
import { useUiStore } from '../../stores/ui'

const props = defineProps<{
  session: TerminalSession
  active: boolean
}>()

const store = useTerminalStore()
const ui = useUiStore()
const hostRef = ref<HTMLElement | null>(null)

let term: Terminal | null = null
let fit: FitAddon | null = null
let detach: (() => void) | null = null
let observer: ResizeObserver | null = null
let resizeFrame = 0

/** 深浅色两套终端配色，与应用主题保持一致。 */
function theme(dark: boolean) {
  return dark
    ? {
        background: '#0f1115',
        foreground: '#d7dae0',
        cursor: '#d7dae0',
        cursorAccent: '#0f1115',
        selectionBackground: '#35507a',
        black: '#1c1f26',
        red: '#e06c75',
        green: '#98c379',
        yellow: '#e5c07b',
        blue: '#61afef',
        magenta: '#c678dd',
        cyan: '#56b6c2',
        white: '#d7dae0',
        brightBlack: '#5c6370',
        brightRed: '#ef7b85',
        brightGreen: '#a9d88a',
        brightYellow: '#f0d197',
        brightBlue: '#74bdf5',
        brightMagenta: '#d68ee6',
        brightCyan: '#67c6d2',
        brightWhite: '#f2f4f8',
      }
    : {
        background: '#ffffff',
        foreground: '#2c3038',
        cursor: '#2c3038',
        cursorAccent: '#ffffff',
        selectionBackground: '#cfe0ff',
        black: '#2c3038',
        red: '#c73642',
        green: '#3f8f45',
        yellow: '#a87200',
        blue: '#2563c9',
        magenta: '#9b3fb5',
        cyan: '#167d8a',
        white: '#e6e8ec',
        brightBlack: '#6b7280',
        brightRed: '#d94b58',
        brightGreen: '#4aa552',
        brightYellow: '#bd8200',
        brightBlue: '#3b7add',
        brightMagenta: '#ad55c7',
        brightCyan: '#2296a4',
        brightWhite: '#f7f8fa',
      }
}

function syncSize() {
  if (!term || !fit) return
  fit.fit()
  store.resize(props.session.id, term.cols, term.rows)
}

onMounted(() => {
  if (!hostRef.value) return
  term = new Terminal({
    cursorBlink: true,
    cursorStyle: 'bar',
    fontSize: 13,
    lineHeight: 1.25,
    fontFamily: "'Cascadia Mono', 'JetBrains Mono', Consolas, 'Microsoft YaHei UI', monospace",
    scrollback: 8000,
    theme: theme(ui.resolvedTheme === 'dark'),
    allowProposedApi: true,
  })
  fit = new FitAddon()
  term.loadAddon(fit)
  term.open(hostRef.value)
  fit.fit()

  term.onData((data) => store.write(props.session.id, data))
  term.attachCustomKeyEventHandler((event) => {
    if (event.type !== 'keydown' || !event.ctrlKey || !event.shiftKey) return true
    const key = event.key.toLowerCase()
    if (key === 'c') {
      const selection = term?.getSelection() ?? ''
      if (selection) void navigator.clipboard?.writeText(selection).catch(() => {})
      return false
    }
    if (key === 'v') {
      void navigator.clipboard
        ?.readText()
        .then((text) => store.write(props.session.id, text))
        .catch(() => {})
      return false
    }
    return true
  })

  detach = store.attach(props.session.id, (bytes) => term?.write(bytes))
  if (props.active) term.focus()

  observer = new ResizeObserver(() => {
    cancelAnimationFrame(resizeFrame)
    resizeFrame = requestAnimationFrame(syncSize)
  })
  observer.observe(hostRef.value)
})

watch(
  () => props.active,
  (active) => {
    if (active) term?.focus()
  },
)

watch(
  () => ui.resolvedTheme,
  (value) => {
    if (term) term.options.theme = theme(value === 'dark')
  },
)

onBeforeUnmount(() => {
  cancelAnimationFrame(resizeFrame)
  observer?.disconnect()
  observer = null
  detach?.()
  detach = null
  term?.dispose()
  term = null
  fit = null
})
</script>

<template>
  <div class="terminal-view">
    <div ref="hostRef" class="terminal-host" />
    <div v-if="session.status === 'connecting'" class="terminal-overlay">
      <span class="spinner" aria-hidden="true" />
      正在连接 {{ session.location.host }}…
    </div>
    <div v-else-if="session.status === 'error'" class="terminal-overlay error">
      <strong>连接失败</strong>
      <span>{{ session.message }}</span>
    </div>
  </div>
</template>

<style scoped>
.terminal-view {
  position: relative;
  flex: 1;
  min-height: 0;
  padding: 6px 8px 4px;
  background: var(--bg-app);
}
.terminal-host {
  width: 100%;
  height: 100%;
}
.terminal-host :deep(.xterm) {
  height: 100%;
}
.terminal-host :deep(.xterm-viewport) {
  background: transparent !important;
  scrollbar-width: thin;
}
.terminal-overlay {
  position: absolute;
  inset: 6px 8px 4px;
  z-index: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 8px;
  border-radius: var(--radius);
  background: color-mix(in srgb, var(--bg-app) 88%, transparent);
  color: var(--fg-muted);
  font-size: 1rem;
  pointer-events: none;
}
.terminal-overlay.error {
  flex-direction: column;
  align-items: flex-start;
  justify-content: center;
  padding: 0 24px;
  color: var(--danger);
  overflow-wrap: anywhere;
  background: color-mix(in srgb, var(--bg-app) 94%, transparent);
  pointer-events: auto;
  user-select: text;
}
.spinner {
  width: 12px;
  height: 12px;
  border: 2px solid var(--border-strong);
  border-top-color: var(--accent);
  border-radius: 50%;
  animation: terminal-spin 0.8s linear infinite;
}
@keyframes terminal-spin {
  to {
    transform: rotate(360deg);
  }
}
</style>

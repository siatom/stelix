<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { api } from '../../api'
import type { DriveInfo, TabState } from '../../types'
import { usePaneStore } from '../../stores/pane'
import { useUiStore } from '../../stores/ui'
import { formatBytes } from '../../utils/formatBytes'
import Icon from '../icons/Icon.vue'
import { t } from '../../i18n'

const props = defineProps<{ paneId: string; tab: TabState }>()
const paneStore = usePaneStore()
const ui = useUiStore()
const drives = ref<DriveInfo[]>([])
const loading = ref(false)
const error = ref<string | null>(null)
let unlisten: UnlistenFn | null = null

const hasUnreadable = computed(() => drives.value.some((drive) => drive.total <= 0 || drive.free < 0))

function usedBytes(drive: DriveInfo): number | null {
  return drive.total > 0 && drive.free >= 0 ? Math.max(0, drive.total - drive.free) : null
}
function usage(drive: DriveInfo): number | null {
  const used = usedBytes(drive)
  return used == null ? null : Math.min(100, Math.max(0, (used / drive.total) * 100))
}
function displayBytes(bytes: number | null): string {
  return bytes == null ? '—' : formatBytes(bytes)
}
function driveTitle(drive: DriveInfo): string {
  return drive.name && drive.name !== drive.mount ? `${drive.mount} · ${drive.name}` : drive.mount
}

async function loadDrives() {
  loading.value = true
  error.value = null
  try {
    drives.value = await api.getRoots()
  } catch (cause) {
    error.value = String(cause)
    drives.value = []
  } finally {
    loading.value = false
  }
}

function openDrive(drive: DriveInfo) {
  void paneStore.navigateTo(props.paneId, drive.mount)
}

async function showMenu(event: MouseEvent, drive: DriveInfo) {
  event.preventDefault()
  event.stopPropagation()
  try {
    const shown = await api.showNativeShellMenu([drive.mount], {
      x: event.clientX,
      y: event.clientY,
      scale: window.devicePixelRatio || 1,
    })
    if (!shown) ui.setStatusText(t('没有可用的系统菜单项'))
  } catch (cause) {
    ui.setStatusText(t(`无法打开系统菜单：${String(cause)}`, `Could not open system menu: ${String(cause)}`))
  }
}

onMounted(async () => {
  await loadDrives()
  unlisten = await listen('computer:drives-changed', () => void loadDrives())
})
onBeforeUnmount(() => unlisten?.())
watch(() => paneStore.computerRefreshSeq, () => void loadDrives())
</script>

<template>
  <main class="computer-view" :aria-label="t('本机磁盘概览')">
    <div v-if="loading && !drives.length" class="computer-state">{{ t('正在读取驱动器…') }}</div>
    <div v-else-if="error && !drives.length" class="computer-state error">{{ t('驱动器读取失败：') }}{{ error }}</div>
    <div v-else-if="!drives.length" class="computer-state">{{ t('未检测到驱动器') }}</div>
    <div v-else class="drive-grid">
      <article
        v-for="drive in drives"
        :key="drive.mount"
        class="drive-card"
        :class="{ warning: (usage(drive) ?? 0) >= 85 }"
        tabindex="0"
        :aria-label="driveTitle(drive)"
        @click="openDrive(drive)"
        @keydown.enter="openDrive(drive)"
        @contextmenu="showMenu($event, drive)"
      >
        <div class="drive-card-head">
          <Icon :name="drive.ejectable ? 'disc' : 'drive'" :size="28" />
          <div><strong>{{ drive.mount }}</strong><span>{{ drive.name || t('本地磁盘') }}</span></div>
        </div>
        <div class="drive-meta"><span>{{ t('文件系统') }}</span><b>{{ drive.filesystem || '—' }}</b></div>
        <div
          class="drive-bar"
          role="progressbar"
          :aria-valuenow="usage(drive) ?? undefined"
          aria-valuemin="0"
          aria-valuemax="100"
          :aria-label="t(`${drive.mount} 已用容量`)">
          <span :style="{ width: `${usage(drive) ?? 0}%` }" />
        </div>
        <div class="drive-capacity"><span>{{ displayBytes(usedBytes(drive)) }} {{ t('已用') }}</span><span>{{ displayBytes(drive.total > 0 && drive.free >= 0 ? drive.free : null) }} {{ t('可用') }}</span></div>
        <div class="drive-total">{{ t('总容量') }} {{ displayBytes(drive.total > 0 ? drive.total : null) }}</div>
      </article>
    </div>
    <p v-if="hasUnreadable && drives.length" class="computer-hint">{{ t('部分驱动器容量暂时不可读取，已用“—”显示。') }}</p>
  </main>
</template>

<style scoped>
.computer-view { flex: 1; min-height: 0; overflow: auto; padding: 28px; background: var(--pane-content-bg); }
.drive-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(260px, 1fr)); gap: 14px; max-width: 1100px; margin: 0 auto; }
.drive-card { padding: 18px; border: 1px solid var(--border); border-radius: 12px; background: var(--bg-panel); cursor: pointer; transition: border-color .15s, box-shadow .15s, transform .15s; }
.drive-card:hover, .drive-card:focus-visible { border-color: var(--accent); box-shadow: 0 5px 18px #0002; transform: translateY(-1px); outline: none; }
.drive-card.warning .drive-bar span { background: var(--danger, #d97706); }
.drive-card-head { display: flex; align-items: center; gap: 13px; color: var(--accent); }
.drive-card-head div { min-width: 0; display: grid; gap: 3px; }
.drive-card-head strong { font-size: 1.2rem; color: var(--fg); }
.drive-card-head span, .drive-meta, .drive-capacity, .drive-total, .computer-hint { color: var(--fg-muted); font-size: .9rem; }
.drive-meta { display: flex; justify-content: space-between; margin: 20px 0 8px; }
.drive-meta b { color: var(--fg); }
.drive-bar { height: 8px; overflow: hidden; border-radius: 999px; background: var(--bg-hover); }
.drive-bar span { display: block; height: 100%; border-radius: inherit; background: var(--accent); }
.drive-capacity, .drive-total { display: flex; justify-content: space-between; margin-top: 9px; }
.drive-total { justify-content: flex-start; margin-top: 5px; }
.computer-state { min-height: 220px; display: grid; place-items: center; color: var(--fg-dim); }
.computer-state.error { color: var(--danger, #d55); }
.computer-hint { max-width: 1100px; margin: 16px auto 0; }
</style>

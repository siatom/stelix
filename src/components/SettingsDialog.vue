<script setup lang="ts">
import { nextTick, onBeforeUnmount, ref, watch } from 'vue'
import { useUiStore } from '../stores/ui'
import Icon from './icons/Icon.vue'
import MacWindowLights from './MacWindowLights.vue'
import SettingsPanel from './Sidebar/SettingsPanel.vue'
import { IS_MACOS } from '../utils/platform'

const ui = useUiStore()

const dialog = ref<HTMLDialogElement | null>(null)
let opener: HTMLElement | null = null

function focusableControls() {
  return Array.from(dialog.value?.querySelectorAll<HTMLElement>(
    'button, a[href], input, select, textarea, [tabindex], [contenteditable="true"]',
  ) ?? []).filter(element => element.tabIndex >= 0
    && !element.matches(':disabled, [inert], [inert] *')
    && element.getClientRects().length > 0
    && getComputedStyle(element).visibility !== 'hidden')
}

function onKeydown(event: KeyboardEvent) {
  if (!dialog.value?.open || event.key !== 'Tab') return
  const controls = focusableControls()
  const first = controls[0]
  const last = controls[controls.length - 1]
  const active = document.activeElement
  if (!first || !last) {
    event.preventDefault()
    dialog.value?.focus()
  } else if (!controls.includes(active as HTMLElement)) {
    event.preventDefault()
    ;(event.shiftKey ? last : first).focus()
  } else if (event.shiftKey && active === first) {
    event.preventDefault()
    last.focus()
  } else if (!event.shiftKey && active === last) {
    event.preventDefault()
    first.focus()
  }
}

function restoreFocus() {
  const target = opener
  opener = null
  if (target?.isConnected && !target.closest('[inert]')) target.focus({ preventScroll: true })
  // The trigger may have disappeared (for example after a pane is removed).
  if (!target || document.activeElement !== target) {
    document.querySelector<HTMLElement>('.app-root')?.focus({ preventScroll: true })
  }
}

watch(() => ui.settingsOpen, async (open, wasOpen) => {
  if (!open) {
    document.removeEventListener('keydown', onKeydown, true)
    dialog.value?.close()
    if (wasOpen) restoreFocus()
    return
  }
  opener = document.activeElement instanceof HTMLElement ? document.activeElement : null
  await nextTick()
  if (!ui.settingsOpen || !dialog.value) return
  // Native modality makes the rest of the document inert, including other teleports.
  dialog.value.showModal()
  document.addEventListener('keydown', onKeydown, true)
  const selected = dialog.value.querySelector<HTMLElement>('[role="tab"][aria-selected="true"]')
  ;(selected ?? focusableControls()[0] ?? dialog.value).focus({ preventScroll: true })
}, { immediate: true })

onBeforeUnmount(() => {
  document.removeEventListener('keydown', onKeydown, true)
  if (dialog.value?.open) {
    dialog.value.close()
    restoreFocus()
  }
})
</script>

<template>
  <Teleport to="body">
    <dialog
      v-if="ui.settingsOpen"
      ref="dialog"
      class="settings-backdrop"
      aria-labelledby="settings-title"
      tabindex="-1"
      @cancel.prevent="ui.closeSettings()"
      @mousedown.self.prevent="ui.closeSettings()"
    >
      <section class="settings-dialog">
        <header class="dialog-head">
          <div class="dialog-head-main">
            <MacWindowLights class="dialog-lights" close-only @close="ui.closeSettings()" />
            <div>
              <h2 id="settings-title">设置</h2>
              <p>管理界面、文件索引与 AI 大模型接入</p>
            </div>
          </div>
          <button
            v-if="!IS_MACOS"
            type="button"
            class="close-btn"
            title="关闭设置"
            aria-label="关闭设置"
            @click="ui.closeSettings()"
          >
            <Icon name="x" :size="16" />
          </button>
        </header>
        <div class="dialog-body">
          <SettingsPanel />
        </div>
      </section>
    </dialog>
  </Teleport>
</template>

<style scoped>
.settings-backdrop {
  position: fixed;
  width: 100%;
  height: 100%;
  max-width: none;
  max-height: none;
  margin: 0;
  border: 0;
  box-sizing: border-box;
  color: inherit;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 28px;
  background: rgb(0 0 0 / 42%);
}
.settings-backdrop::backdrop {
  background: transparent;
}
.settings-dialog {
  display: flex;
  flex-direction: column;
  width: min(960px, 100%);
  height: min(780px, 100%);
  background: var(--bg-panel);
  border: 1px solid var(--border-strong);
  border-radius: calc(var(--radius) * 1.5);
  box-shadow: 0 18px 60px rgb(0 0 0 / 28%);
  overflow: hidden;
}
.dialog-head {
  display: flex;
  flex: none;
  align-items: center;
  justify-content: space-between;
  padding: 18px 22px;
  border-bottom: 1px solid var(--border);
}
.dialog-head-main {
  display: flex;
  align-items: center;
  gap: 14px;
  min-width: 0;
}
.dialog-head h2 {
  margin: 0;
  font-size: 1.3333rem;
}
.dialog-head p {
  margin: 4px 0 0;
  color: var(--fg-dim);
  font-size: 0.9167rem;
}
.close-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 30px;
  height: 30px;
  border-radius: var(--radius-sm);
  color: var(--fg-muted);
}
.close-btn:hover {
  background: var(--bg-hover);
  color: var(--fg);
}
.dialog-body {
  flex: 1;
  min-height: 0;
  overflow: hidden;
}
.dialog-body :deep(.settings-panel) {
  min-height: 0;
}
.dialog-body :deep(.card) {
  border-radius: var(--radius-lg);
}
.dialog-body :deep(.card-head) {
  padding: 10px 14px;
  font-size: 1rem;
}
.dialog-body :deep(.card-body) {
  padding: 12px;
}
@media (max-width: 680px) {
  .settings-backdrop { padding: 12px; }
  .settings-dialog { max-height: 100%; }
}
</style>

<script setup lang="ts">
import { onMounted, onUnmounted } from 'vue'
import { useUiStore } from '../stores/ui'
import Icon from './icons/Icon.vue'
import SettingsPanel from './Sidebar/SettingsPanel.vue'

const ui = useUiStore()

function onKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape') ui.closeSettings()
}

onMounted(() => window.addEventListener('keydown', onKeydown))
onUnmounted(() => window.removeEventListener('keydown', onKeydown))
</script>

<template>
  <Teleport to="body">
    <div v-if="ui.settingsOpen" class="settings-backdrop" @mousedown.self="ui.closeSettings()">
      <section class="settings-dialog" role="dialog" aria-modal="true" aria-labelledby="settings-title">
        <header class="dialog-head">
          <div>
            <h2 id="settings-title">设置</h2>
            <p>管理索引、界面显示和启动行为</p>
          </div>
          <button type="button" class="close-btn" title="关闭设置" aria-label="关闭设置" @click="ui.closeSettings()">
            <Icon name="x" :size="16" />
          </button>
        </header>
        <div class="dialog-body">
          <SettingsPanel />
        </div>
      </section>
    </div>
  </Teleport>
</template>

<style scoped>
.settings-backdrop {
  position: fixed;
  z-index: 100;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 28px;
  background: rgb(0 0 0 / 42%);
}
.settings-dialog {
  display: flex;
  flex-direction: column;
  width: min(860px, 100%);
  max-height: min(820px, 100%);
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
  min-height: 0;
  padding: 20px 22px;
  overflow: auto;
}
.dialog-body :deep(.settings-panel) {
  min-height: 0;
}
.dialog-body :deep(.settings-scroll) {
  display: grid;
  grid-template-columns: minmax(0, 1.1fr) minmax(0, 0.9fr);
  grid-template-areas:
    'status view'
    'roots startup'
    'error error';
  align-items: start;
  gap: 16px;
  overflow: visible;
}
.dialog-body :deep(.settings-scroll > .card:nth-child(1)) { grid-area: status; }
.dialog-body :deep(.settings-scroll > .card:nth-child(2)) { grid-area: roots; }
.dialog-body :deep(.settings-scroll > .card:nth-child(3)) { grid-area: view; }
.dialog-body :deep(.settings-scroll > .card:nth-child(4)) { grid-area: startup; }
.dialog-body :deep(.settings-scroll > .error) { grid-area: error; }
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
.dialog-body :deep(.build-info) {
  margin-top: 16px;
}
@media (max-width: 680px) {
  .settings-backdrop { padding: 12px; }
  .settings-dialog { max-height: 100%; }
  .dialog-body { padding: 14px; }
  .dialog-body :deep(.settings-scroll) {
    grid-template-columns: minmax(0, 1fr);
    grid-template-areas: 'status' 'roots' 'view' 'startup' 'error';
  }
}
</style>

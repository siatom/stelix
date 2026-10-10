<script setup lang="ts">
import { ref } from 'vue'
import { api } from '../../api'
import Icon from '../icons/Icon.vue'

const props = defineProps<{
  protocol: 'ftp' | 'sftp'
  host: string
  port: number
  username: string
  password: string
  path: string
  name: string
  depth: number
}>()

const expanded = ref(false)
const loading = ref(false)
const loaded = ref(false)
const error = ref('')
const children = ref<string[]>([])

async function toggle() {
  if (expanded.value) { expanded.value = false; return }
  expanded.value = true
  if (loaded.value) return
  loading.value = true
  error.value = ''
  try {
    children.value = await api.listRemote({
      protocol: props.protocol, host: props.host, port: props.port,
      username: props.username, password: props.password, path: props.path,
    })
    loaded.value = true
  } catch (cause) {
    error.value = String(cause)
  } finally {
    loading.value = false
  }
}
</script>

<template>
  <div class="remote-node">
    <button class="row" :style="{ paddingLeft: `${depth * 12 + 6}px` }" :title="path" @click="toggle">
      <span class="twist" :class="{ open: expanded }"><Icon name="chevron-right" :size="11" /></span>
      <Icon name="folder" :size="14" />
      <span class="label">{{ name }}</span>
    </button>
    <div v-if="expanded">
      <p v-if="loading" class="state">读取中…</p>
      <p v-else-if="error" class="state error">{{ error }}</p>
      <p v-else-if="!children.length" class="state">空目录</p>
      <RemoteTreeNode
        v-for="child in children" :key="child" :protocol="protocol" :host="host" :port="port"
        :username="username" :password="password" :name="child"
        :path="`${path.replace(/\/$/, '')}/${child}`" :depth="depth + 1"
      />
    </div>
  </div>
</template>

<style scoped>
.row { display: flex; align-items: center; gap: 4px; width: 100%; height: 22px; padding-right: 6px; border-radius: var(--radius); text-align: left; color: var(--fg-folder); font-size: 1rem; }
.row:hover { background: var(--bg-hover); }
.twist { display: flex; align-items: center; justify-content: center; width: 12px; flex: none; color: var(--fg-dim); transition: transform .12s ease; }
.twist.open { transform: rotate(90deg); }
.label { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.state { margin: 3px 4px 3px 28px; color: var(--fg-dim); font-size: 0.9167rem; overflow-wrap: anywhere; }
.error { color: var(--danger); }
</style>

<script setup lang="ts">
// 远程（FTP / SFTP）目录树节点：三角只负责展开下一级，点名称在文件窗格打开该目录。
import { ref } from 'vue'
import { api } from '../../api'
import { usePaneStore } from '../../stores/pane'
import { setRemotePassword } from '../../shell/remote'
import type { RemoteLocation } from '../../types'
import Icon from '../icons/Icon.vue'

const props = defineProps<{
  location: RemoteLocation
  /** 当前密码（由侧栏密码输入框提供，内存态） */
  password: string
  path: string
  name: string
  depth: number
}>()

const pane = usePaneStore()

const expanded = ref(false)
const loading = ref(false)
const loaded = ref(false)
const error = ref('')
const children = ref<{ name: string; path: string }[]>([])

/** 三角：只在点击展开按钮时列出下一级（且只列子目录，与本地目录树一致）。 */
async function toggle() {
  if (expanded.value) {
    expanded.value = false
    return
  }
  expanded.value = true
  if (loaded.value) return
  loading.value = true
  error.value = ''
  try {
    const listing = await api.listRemoteDir({
      ...props.location,
      password: props.password,
      path: props.path,
    })
    children.value = listing.entries
      .filter((entry) => entry.isDir)
      .map((entry) => ({ name: entry.name, path: entry.path }))
    loaded.value = true
  } catch (cause) {
    error.value = String(cause)
  } finally {
    loading.value = false
  }
}

/** 点名称：在（当前活动的）文件窗格打开该远程目录。 */
function openInPane() {
  setRemotePassword(props.location.id, props.password)
  void pane.openRemote(pane.activePaneId, props.location, props.path)
}
</script>

<template>
  <div class="remote-node">
    <div class="row" :style="{ paddingLeft: `${depth * 12 + 6}px` }">
      <span
        class="twist"
        :class="{ open: expanded }"
        role="button"
        :title="expanded ? '折叠' : '展开下一级'"
        @click.stop="toggle"
      >
        <Icon name="chevron-right" :size="11" />
      </span>
      <button class="row-main" :title="path" @click="openInPane">
        <Icon name="folder" :size="18" />
        <span class="label">{{ name }}</span>
      </button>
    </div>
    <div v-if="expanded">
      <p v-if="loading" class="state">读取中…</p>
      <p v-else-if="error" class="state error">{{ error }}</p>
      <p v-else-if="!children.length" class="state">无子目录</p>
      <RemoteTreeNode
        v-for="child in children"
        :key="child.path"
        :location="location"
        :password="password"
        :name="child.name"
        :path="child.path"
        :depth="depth + 1"
      />
    </div>
  </div>
</template>

<style scoped>
.row { display: flex; align-items: center; gap: 4px; height: var(--sidebar-row-h); padding-right: 6px; border-radius: var(--radius); }
.row:hover { background: var(--bg-hover); }
.row-main { display: flex; align-items: center; gap: 4px; flex: 1; min-width: 0; height: 100%; text-align: left; color: var(--fg-folder); font-size: 1rem; }
.twist { display: flex; align-items: center; justify-content: center; width: 12px; flex: none; color: var(--fg-dim); cursor: pointer; transition: transform .12s ease; }
.twist:hover { color: var(--fg); }
.twist.open { transform: rotate(90deg); }
.label { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.state { margin: 3px 4px 3px 28px; color: var(--fg-dim); font-size: 0.9167rem; overflow-wrap: anywhere; }
.error { color: var(--danger); }
</style>

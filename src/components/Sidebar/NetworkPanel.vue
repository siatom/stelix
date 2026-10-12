<script setup lang="ts">
import { ref } from 'vue'
import { api } from '../../api'
import { usePaneStore } from '../../stores/pane'
import { setRemotePassword } from '../../shell/remote'
import TreeNode from './TreeNode.vue'
import RemoteTreeNode from './RemoteTreeNode.vue'
import Icon from '../icons/Icon.vue'

interface RemoteHost {
  id: string
  protocol: 'ftp' | 'sftp'
  label: string
  host: string
  port: number
  username: string
}

interface SmbNeighbor {
  server: string
  shares: string[]
}

const SHARES_KEY = 'stelix.smbShares'
const HOSTS_KEY = 'stelix.remoteHosts'

function readShares(): string[] {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(SHARES_KEY) || '[]')
    return Array.isArray(value) ? value.filter((item): item is string => typeof item === 'string' && /^\\\\[^\\]+\\[^\\]+/.test(item)) : []
  } catch { return [] }
}

function readHosts(): RemoteHost[] {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(HOSTS_KEY) || '[]')
    if (!Array.isArray(value)) return []
    return value.filter((item): item is RemoteHost =>
      !!item && (item.protocol === 'ftp' || item.protocol === 'sftp') &&
      typeof item.id === 'string' && typeof item.host === 'string' &&
      typeof item.label === 'string' && typeof item.username === 'string' &&
      Number.isInteger(item.port) && item.port > 0 && item.port <= 65535)
  } catch { return [] }
}

const pane = usePaneStore()
const shares = ref(readShares())
const neighbors = ref<SmbNeighbor[]>([])
const expandedServers = ref<Record<string, boolean>>({})
const serverShares = ref<Record<string, string[]>>({})
const serverLoading = ref<Record<string, boolean>>({})
const serverErrors = ref<Record<string, string>>({})
const scanLoading = ref(false)
const scanError = ref('')
const hosts = ref(readHosts())
const sections = ref({ smb: false, remote: false, cloud: false })
const smbDraft = ref('')
const showSmbForm = ref(false)
const showHostForm = ref(false)
const editingHostId = ref<string | null>(null)
const protocol = ref<'ftp' | 'sftp'>('sftp')
const label = ref('')
const host = ref('')
const port = ref(22)
const username = ref('')
const draftPassword = ref('')
const expandedHosts = ref<Record<string, boolean>>({})
/** 主机 id → 该主机根目录下的**子目录**（展开树用；进入目录在文件窗格进行）。 */
const hostEntries = ref<Record<string, { name: string; path: string }[]>>({})
const passwords = ref<Record<string, string>>({})
const hostLoading = ref<Record<string, boolean>>({})
const hostErrors = ref<Record<string, string>>({})
const error = ref('')

async function scanNeighbors() {
  scanLoading.value = true
  scanError.value = ''
  try {
    neighbors.value = await api.discoverSmbNeighbors()
    serverShares.value = {}
    serverErrors.value = {}
  } catch (cause) {
    scanError.value = String(cause)
  } finally {
    scanLoading.value = false
  }
}

function toggleSmb() {
  sections.value.smb = !sections.value.smb
  if (sections.value.smb) void scanNeighbors()
}

function openSmbForm() {
  const wasCollapsed = !sections.value.smb
  sections.value.smb = true
  showSmbForm.value = true
  error.value = ''
  if (wasCollapsed) void scanNeighbors()
}

function closeSmbForm() {
  showSmbForm.value = false
  smbDraft.value = ''
  error.value = ''
}

function openHostForm() {
  sections.value.remote = true
  editingHostId.value = null
  protocol.value = 'sftp'
  label.value = ''
  host.value = ''
  port.value = 22
  username.value = ''
  draftPassword.value = ''
  showHostForm.value = true
  error.value = ''
}

function closeHostForm() {
  showHostForm.value = false
  editingHostId.value = null
  protocol.value = 'sftp'
  label.value = ''
  host.value = ''
  port.value = 22
  username.value = ''
  draftPassword.value = ''
  error.value = ''
}

function refreshSmb() {
  sections.value.smb = true
  void scanNeighbors()
}

async function loadShares(server: string) {
  serverLoading.value[server] = true
  serverErrors.value[server] = ''
  try {
    serverShares.value[server] = await api.listSmbShares(server)
  } catch (cause) {
    serverErrors.value[server] = String(cause)
  } finally {
    serverLoading.value[server] = false
  }
}

function toggleServer(server: string) {
  expandedServers.value[server] = !expandedServers.value[server]
  if (expandedServers.value[server] && !serverShares.value[server]) void loadShares(server)
}

function saveShare() {
  const path = smbDraft.value.trim().replace(/\//g, '\\').replace(/\\+$/, '')
  if (!/^\\\\[^\\]+\\[^\\]+(?:\\.*)?$/.test(path)) {
    error.value = '请输入共享路径，例如 \\\\服务器\\共享名'
    return
  }
  if (!shares.value.some((item) => item.toLowerCase() === path.toLowerCase())) {
    shares.value.push(path)
    localStorage.setItem(SHARES_KEY, JSON.stringify(shares.value))
  }
  smbDraft.value = ''
  closeSmbForm()
  error.value = ''
  void pane.navigate(path)
}

function removeShare(path: string) {
  shares.value = shares.value.filter((item) => item !== path)
  localStorage.setItem(SHARES_KEY, JSON.stringify(shares.value))
}

async function saveHost() {
  const address = host.value.trim()
  if (!address || !/^[a-zA-Z0-9.-]+$/.test(address) || !Number.isInteger(port.value) || port.value < 1 || port.value > 65535) {
    error.value = '请填写有效的主机地址和端口'
    return
  }
  const id = editingHostId.value || crypto.randomUUID()
  if (draftPassword.value) {
    try {
      await api.saveRemotePassword(id, draftPassword.value)
    } catch (cause) {
      error.value = String(cause)
      return
    }
  }
  const item: RemoteHost = {
    id, protocol: protocol.value, label: label.value.trim() || address,
    host: address, port: port.value, username: username.value.trim(),
  }
  const existingIndex = hosts.value.findIndex((hostItem) => hostItem.id === id)
  if (existingIndex >= 0) hosts.value[existingIndex] = item
  else hosts.value.push(item)
  localStorage.setItem(HOSTS_KEY, JSON.stringify(hosts.value))
  if (draftPassword.value) passwords.value[item.id] = draftPassword.value
  delete hostEntries.value[item.id]
  delete hostErrors.value[item.id]
  expandedHosts.value[item.id] = false
  closeHostForm()
}

async function editHost(item: RemoteHost) {
  editingHostId.value = item.id
  protocol.value = item.protocol
  label.value = item.label
  host.value = item.host
  port.value = item.port
  username.value = item.username
  draftPassword.value = passwords.value[item.id] || ''
  sections.value.remote = true
  showHostForm.value = true
  error.value = ''
  try {
    const storedPassword = await api.getRemotePassword(item.id)
    if (editingHostId.value === item.id && storedPassword) draftPassword.value = storedPassword
  } catch (cause) {
    if (editingHostId.value === item.id) error.value = String(cause)
  }
}

async function removeHost(id: string) {
  try {
    await api.deleteRemotePassword(id)
  } catch (cause) {
    hostErrors.value[id] = String(cause)
    return
  }
  hosts.value = hosts.value.filter((item) => item.id !== id)
  localStorage.setItem(HOSTS_KEY, JSON.stringify(hosts.value))
  delete passwords.value[id]
  delete hostEntries.value[id]
}

async function loadHost(item: RemoteHost) {
  hostLoading.value[item.id] = true
  hostErrors.value[item.id] = ''
  try {
    if (passwords.value[item.id] === undefined) {
      passwords.value[item.id] = (await api.getRemotePassword(item.id)) || ''
    }
    const password = passwords.value[item.id] || ''
    // 密码与文件窗格共享（内存态），这样在窗格里浏览时无需再读一次凭据库。
    setRemotePassword(item.id, password)
    const listing = await api.listRemoteDir({ ...item, password, path: '/' })
    hostEntries.value[item.id] = listing.entries
      .filter((entry) => entry.isDir)
      .map((entry) => ({ name: entry.name, path: entry.path }))
  } catch (cause) {
    // 密码统一在「编辑主机」里维护，这里只在未设置密码时补一句提示。
    const hint = passwords.value[item.id] ? '' : '（可在「编辑主机」中填写密码）'
    hostErrors.value[item.id] = `${String(cause)}${hint}`
  } finally {
    hostLoading.value[item.id] = false
  }
}

/**
 * 点击主机（连接）名称：在文件窗格打开该连接。
 *
 * 与「展开三角」分工：三角才在左侧树里逐级列出子目录（见 `toggleHost`）。
 */
async function openHostInPane(item: RemoteHost) {
  if (passwords.value[item.id] === undefined) {
    try {
      passwords.value[item.id] = (await api.getRemotePassword(item.id)) || ''
    } catch {
      passwords.value[item.id] = ''
    }
  }
  setRemotePassword(item.id, passwords.value[item.id] || '')
  await pane.openRemote(
    pane.activePaneId,
    {
      id: item.id,
      protocol: item.protocol,
      host: item.host,
      port: item.port,
      username: item.username,
      label: item.label,
    },
    '/',
  )
}

function toggleHost(item: RemoteHost) {
  expandedHosts.value[item.id] = !expandedHosts.value[item.id]
  // 每次展开都重新列一次（这就是该主机唯一的刷新入口 —— 密码在「编辑主机」里维护）。
  if (expandedHosts.value[item.id]) void loadHost(item)
}
</script>

<template>
  <div class="network-tree">
    <section>
      <div class="tree-row category">
        <button class="row-main" @click="toggleSmb">
          <span class="twist" :class="{ open: sections.smb }"><Icon name="chevron-right" :size="11" /></span>
          <Icon class="category-icon" :name="sections.smb ? 'folder-open' : 'folder'" :size="14" /><span>局域网 · SMB</span>
        </button>
        <button class="action" title="刷新网上邻居" aria-label="刷新网上邻居" :disabled="scanLoading" @click="refreshSmb"><Icon name="refresh" :size="13" /></button>
        <button class="action" title="添加共享" aria-label="添加 SMB 共享" @click="openSmbForm"><Icon name="plus" :size="13" /></button>
      </div>
      <div v-if="sections.smb">
        <p v-if="scanLoading" class="state">正在扫描网上邻居…</p>
        <p v-else-if="scanError" class="state error">{{ scanError }}</p>
        <div v-for="neighbor in neighbors" :key="neighbor.server">
          <div class="tree-row server-row">
            <button class="row-main" :title="`\\\\${neighbor.server}`" @click="toggleServer(neighbor.server)">
              <span class="twist" :class="{ open: expandedServers[neighbor.server] }"><Icon name="chevron-right" :size="11" /></span>
              <Icon name="drive" :size="14" /><span class="label">{{ neighbor.server }}</span>
            </button>
          </div>
          <div v-if="expandedServers[neighbor.server]">
            <p v-if="serverLoading[neighbor.server]" class="state nested">正在读取共享…</p>
            <p v-else-if="serverErrors[neighbor.server]" class="state nested error">{{ serverErrors[neighbor.server] }}</p>
            <p v-else-if="!(serverShares[neighbor.server] ?? neighbor.shares).length" class="state nested">未发现可访问的共享</p>
            <TreeNode v-for="share in serverShares[neighbor.server] ?? neighbor.shares" :key="share"
              :node="{ name: share.split('\\').filter(Boolean).slice(1).join('\\'), path: share }"
              :depth="2" @navigate="pane.navigate($event)" />
          </div>
        </div>
        <p v-if="!scanLoading && !neighbors.length && !shares.length" class="state">未发现设备，可手动添加共享</p>
        <p v-if="shares.length" class="state manual-label">手动添加</p>
        <div v-for="share in shares" :key="share" class="share-node">
          <TreeNode :node="{ name: share.replace(/^\\\\/, ''), path: share }" :depth="1" @navigate="pane.navigate($event)" />
          <button class="remove share-remove" title="移除共享" @click="removeShare(share)"><Icon name="x" :size="11" /></button>
        </div>
      </div>
    </section>

    <section>
      <div class="tree-row category">
        <button class="row-main" @click="sections.remote = !sections.remote">
          <span class="twist" :class="{ open: sections.remote }"><Icon name="chevron-right" :size="11" /></span>
          <Icon class="category-icon" :name="sections.remote ? 'folder-open' : 'folder'" :size="14" /><span>FTP / SFTP</span>
        </button>
        <button class="action" title="添加主机" aria-label="添加 FTP 或 SFTP 主机" @click="openHostForm"><Icon name="plus" :size="13" /></button>
      </div>
      <div v-if="sections.remote">
        <p v-if="!hosts.length" class="state">暂无主机</p>
        <div v-for="item in hosts" :key="item.id">
          <div class="tree-row host-row">
            <span class="twist" :class="{ open: expandedHosts[item.id] }" role="button"
              :title="expandedHosts[item.id] ? '折叠' : '展开下一级'"
              :aria-label="`${expandedHosts[item.id] ? '折叠' : '展开'}${item.label}`"
              @click.stop="toggleHost(item)">
              <Icon name="chevron-right" :size="11" />
            </span>
            <button class="row-main" :title="`打开 ${item.protocol}://${item.host}:${item.port}`" @click="openHostInPane(item)">
              <Icon name="drive" :size="14" /><span class="label">{{ item.label }}</span><small>{{ item.protocol.toUpperCase() }}</small>
            </button>
            <div class="host-actions">
              <button class="remove" title="编辑主机" aria-label="编辑主机" @click="editHost(item)"><Icon name="pencil" :size="12" /></button>
              <button class="remove" title="移除主机" aria-label="移除主机" @click="removeHost(item.id)"><Icon name="x" :size="11" /></button>
            </div>
          </div>
          <div v-if="expandedHosts[item.id]">
            <p v-if="hostLoading[item.id]" class="state nested">读取中…</p>
            <p v-else-if="hostErrors[item.id]" class="state nested error">{{ hostErrors[item.id] }}</p>
            <p v-else-if="hostEntries[item.id] && !hostEntries[item.id].length" class="state nested">无子目录</p>
            <RemoteTreeNode
              v-for="entry in hostEntries[item.id] || []" :key="entry.path"
              :location="{ id: item.id, protocol: item.protocol, host: item.host, port: item.port, username: item.username, label: item.label }"
              :password="passwords[item.id] || ''" :name="entry.name" :path="entry.path" :depth="2"
            />
          </div>
        </div>
      </div>
    </section>

    <section>
      <div class="tree-row category">
        <button class="row-main" @click="sections.cloud = !sections.cloud">
          <span class="twist" :class="{ open: sections.cloud }"><Icon name="chevron-right" :size="11" /></span>
          <Icon class="category-icon" :name="sections.cloud ? 'folder-open' : 'folder'" :size="14" /><span>云存储</span>
        </button>
      </div>
      <p v-if="sections.cloud" class="state">暂无云存储连接</p>
    </section>
    <Teleport to="body">
      <div v-if="showSmbForm" class="dialog-backdrop" tabindex="-1" @keydown.esc.prevent.stop>
        <section class="dialog" role="dialog" aria-modal="true" aria-labelledby="smb-dialog-title">
          <h2 id="smb-dialog-title">添加 SMB 共享</h2>
          <form class="dialog-form" @submit.prevent="saveShare">
            <label>共享路径<input v-model="smbDraft" autofocus placeholder="\\服务器\共享名" aria-label="SMB 共享路径" /></label>
            <p v-if="error" class="dialog-error">{{ error }}</p>
            <div class="dialog-actions">
              <button type="button" class="secondary" @click="closeSmbForm">取消</button>
              <button type="submit" class="primary">添加</button>
            </div>
          </form>
        </section>
      </div>

      <div v-if="showHostForm" class="dialog-backdrop" tabindex="-1" @keydown.esc.prevent.stop>
        <section class="dialog" role="dialog" aria-modal="true" aria-labelledby="host-dialog-title">
          <h2 id="host-dialog-title">{{ editingHostId ? '编辑 FTP / SFTP 主机' : '添加 FTP / SFTP 主机' }}</h2>
          <form class="dialog-form" @submit.prevent="saveHost">
            <label>协议
              <select v-model="protocol" aria-label="协议" @change="port = protocol === 'sftp' ? 22 : 21">
                <option value="sftp">SFTP</option><option value="ftp">FTP</option>
              </select>
            </label>
            <label>名称（可选）<input v-model="label" placeholder="主机名称" /></label>
            <label>主机地址<input v-model="host" placeholder="主机地址" required /></label>
            <label>端口<input v-model.number="port" type="number" min="1" max="65535" required /></label>
            <label>用户名<input v-model="username" placeholder="用户名" /></label>
            <label>密码<input v-model="draftPassword" type="password" autocomplete="new-password" placeholder="密码（保存在系统凭据库）" /></label>
            <p v-if="error" class="dialog-error">{{ error }}</p>
            <div class="dialog-actions">
              <button type="button" class="secondary" @click="closeHostForm">取消</button>
              <button type="submit" class="primary">{{ editingHostId ? '保存修改' : '保存主机' }}</button>
            </div>
          </form>
        </section>
      </div>
    </Teleport>
  </div>
</template>

<style scoped>
.network-tree { display: flex; flex-direction: column; gap: 3px; }
.tree-row { display: flex; align-items: center; height: var(--sidebar-row-h); border-radius: var(--radius); }
.tree-row:hover { background: var(--bg-hover); }
.row-main { display: flex; align-items: center; gap: 4px; flex: 1; min-width: 0; height: 100%; padding: 0 6px; text-align: left; color: var(--fg-folder); font-size: 1rem; }
.row-main span:last-of-type, .label { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.category-icon { flex: none; color: var(--accent); }
.host-row .twist { margin-left: 18px; }
.host-actions { display: flex; align-items: center; gap: 2px; flex: none; }
.server-row .row-main { padding-left: 18px; }
.twist { display: flex; align-items: center; justify-content: center; width: 12px; flex: none; color: var(--fg-dim); cursor: pointer; transition: transform .12s ease; }
.twist:hover { color: var(--fg); }
.twist.open { transform: rotate(90deg); }
.row-main small { margin-left: auto; color: var(--fg-dim); font-size: 0.8333rem; }
.action, .remove { display: flex; align-items: center; justify-content: center; flex: none; width: 20px; height: 20px; color: var(--fg-dim); border-radius: var(--radius); }
.action:hover, .remove:hover { background: var(--bg-active); color: var(--fg); }
.action:disabled { opacity: .4; cursor: default; }
.share-node { position: relative; }
.share-remove { position: absolute; right: 1px; top: 1px; }
.state { margin: 3px 6px 5px 30px; font-size: 0.9167rem; color: var(--fg-dim); overflow-wrap: anywhere; }
.state.nested { margin-left: 42px; }
.state.manual-label { margin-top: 6px; }
.state.error { color: var(--danger); }
.dialog-backdrop { position: fixed; inset: 0; z-index: 1000; display: flex; align-items: center; justify-content: center; padding: 20px; background: rgb(0 0 0 / 45%); }
.dialog { width: min(420px, 100%); max-height: min(85vh, 720px); overflow: auto; padding: 18px; border: 1px solid var(--border-strong); border-radius: 10px; background: var(--bg-panel); box-shadow: var(--shadow-lg); }
.dialog h2 { margin: 0 0 16px; color: var(--fg); font-size: 1.1667rem; font-weight: 600; }
.dialog-form { display: flex; flex-direction: column; gap: 12px; }
.dialog-form label { display: flex; flex-direction: column; gap: 5px; color: var(--fg-muted); font-size: 0.9167rem; }
.dialog-form input, .dialog-form select { min-width: 0; width: 100%; padding: 7px 8px; border: 1px solid var(--border-strong); border-radius: var(--radius); background: var(--bg-input); color: var(--fg); font: inherit; font-size: 1rem; }
.dialog-error { margin: 0; color: var(--danger); font-size: 0.9167rem; overflow-wrap: anywhere; }
.dialog-actions { display: flex; justify-content: flex-end; gap: 8px; margin-top: 4px; }
.dialog-actions button { padding: 6px 12px; border: 1px solid var(--border); border-radius: var(--radius); font-size: 0.9167rem; }
.dialog-actions .secondary { color: var(--fg); background: var(--bg-hover); }
.dialog-actions .primary { border-color: var(--accent); color: var(--accent-fg); background: var(--accent); }
</style>

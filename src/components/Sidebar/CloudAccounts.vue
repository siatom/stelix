<script setup lang="ts">
// 云存储账号（百度网盘 / 迅雷云盘）：只持久化非敏感账号信息，令牌由后端放在系统凭据库。
import { computed, ref } from 'vue'
import { openUrl } from '@tauri-apps/plugin-opener'
import { api } from '../../api'
import { usePaneStore } from '../../stores/pane'
import { cloudProviderName, resolveRemoteAuth } from '../../shell/remote'
import { compareNames } from '../../naturalSort'
import type { CloudAccount, CloudProvider, RemoteLocation } from '../../types'
import RemoteTreeNode from './RemoteTreeNode.vue'
import Icon from '../icons/Icon.vue'

const STORAGE_KEY = 'stelix.cloudAccounts'

interface RootEntry {
  name: string
  path: string
}

function isProvider(value: unknown): value is CloudProvider {
  return value === 'baidu' || value === 'xunlei'
}

function readAccounts(): CloudAccount[] {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(STORAGE_KEY) || '[]')
    if (!Array.isArray(value)) return []
    return value
      .filter(
        (item): item is Record<string, unknown> =>
          !!item &&
          isProvider(item.provider) &&
          typeof item.id === 'string' &&
          typeof item.label === 'string' &&
          typeof item.username === 'string',
      )
      .map((item) => ({
        id: item.id as string,
        provider: item.provider as CloudProvider,
        label: item.label as string,
        username: item.username as string,
      }))
  } catch {
    return []
  }
}

const pane = usePaneStore()
const accounts = ref<CloudAccount[]>(readAccounts())
const expanded = ref<Record<string, boolean>>({})
const loading = ref<Record<string, boolean>>({})
const errors = ref<Record<string, string>>({})
const roots = ref<Record<string, RootEntry[]>>({})
const openMenuId = ref<string | null>(null)

const showForm = ref(false)
const provider = ref<CloudProvider>('baidu')
const label = ref('')
const clientId = ref('')
const clientSecret = ref('')
const xunleiMode = ref<'password' | 'token'>('password')
const username = ref('')
const password = ref('')
const refreshToken = ref('')
const busy = ref(false)
const formError = ref('')

function providerHost(item: CloudProvider): string {
  return item === 'baidu' ? 'pan.baidu.com' : 'yun.xunlei.com'
}

/** 云盘的连接信息：`id` 同时是标签页身份与系统凭据库里的账号 id。 */
function locationOf(account: CloudAccount): RemoteLocation {
  return {
    id: account.id,
    protocol: account.provider,
    host: providerHost(account.provider),
    port: 443,
    username: account.username,
    label: account.label,
    accountId: account.id,
  }
}

function persist() {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(accounts.value))
}

function openLogin() {
  provider.value = 'baidu'
  label.value = ''
  clientId.value = ''
  clientSecret.value = ''
  xunleiMode.value = 'password'
  username.value = ''
  password.value = ''
  refreshToken.value = ''
  busy.value = false
  formError.value = ''
  showForm.value = true
}

function closeLogin() {
  if (busy.value) return
  showForm.value = false
  formError.value = ''
}

/** 迅雷触发人机验证时，错误里会带验证地址；提取出来供用户直接打开。 */
const captchaUrl = computed(() => {
  if (provider.value !== 'xunlei') return null
  const match = formError.value.match(/https?:\/\/[^\s，。；]+/)
  return match ? match[0] : null
})

async function openCaptcha() {
  if (!captchaUrl.value) return
  try {
    await openUrl(captchaUrl.value)
  } catch (cause) {
    formError.value = String(cause)
  }
}

async function submitLogin() {
  if (busy.value) return
  formError.value = ''
  if (provider.value === 'baidu') {
    if (!clientId.value.trim() || !clientSecret.value.trim()) {
      formError.value = '请填写百度网盘开放平台的 AppKey 和 SecretKey'
      return
    }
  } else if (xunleiMode.value === 'token') {
    if (!refreshToken.value.trim()) {
      formError.value = '请粘贴迅雷刷新令牌'
      return
    }
  } else if (!username.value.trim() || !password.value) {
    formError.value = '请填写迅雷账号和密码'
    return
  }

  busy.value = true
  const id = crypto.randomUUID()
  const title = label.value.trim() || undefined
  try {
    const info =
      provider.value === 'baidu'
        ? await api.cloudBaiduLogin({
            accountId: id,
            clientId: clientId.value.trim(),
            clientSecret: clientSecret.value.trim(),
            label: title,
          })
        : xunleiMode.value === 'token'
          ? await api.cloudXunleiTokenLogin({
              accountId: id,
              refreshToken: refreshToken.value.trim(),
              label: title,
            })
          : await api.cloudXunleiLogin({
              accountId: id,
              username: username.value.trim(),
              password: password.value,
              label: title,
            })

    const account: CloudAccount = {
      id: info.accountId,
      provider: info.provider,
      label: info.label,
      username: info.username,
    }
    accounts.value = accounts.value.filter((item) => item.id !== account.id)
    accounts.value.push(account)
    persist()
    showForm.value = false
    formError.value = ''
    expanded.value[account.id] = true
    await loadRoots(account)
  } catch (cause) {
    formError.value = String(cause)
  } finally {
    busy.value = false
  }
}

async function loadRoots(account: CloudAccount) {
  loading.value[account.id] = true
  errors.value[account.id] = ''
  try {
    const location = locationOf(account)
    const auth = await resolveRemoteAuth(location)
    const listing = await api.listRemoteDir({ ...location, ...auth, path: '/' })
    roots.value[account.id] = listing.entries
      .filter((entry) => entry.isDir)
      .map((entry) => ({ name: entry.name, path: entry.path }))
      .sort((a, b) => compareNames(a.name, b.name))
  } catch (cause) {
    errors.value[account.id] = String(cause)
  } finally {
    loading.value[account.id] = false
  }
}

function toggle(account: CloudAccount) {
  expanded.value[account.id] = !expanded.value[account.id]
  if (expanded.value[account.id]) void loadRoots(account)
}

function refresh(account: CloudAccount) {
  expanded.value[account.id] = true
  void loadRoots(account)
}

function open(account: CloudAccount) {
  void pane.openRemoteWorkspace(locationOf(account), '/')
}

async function removeAccount(account: CloudAccount) {
  openMenuId.value = null
  try {
    await api.cloudLogout(account.id)
  } catch (cause) {
    errors.value[account.id] = String(cause)
    return
  }
  accounts.value = accounts.value.filter((item) => item.id !== account.id)
  persist()
  delete expanded.value[account.id]
  delete loading.value[account.id]
  delete errors.value[account.id]
  delete roots.value[account.id]
}

defineExpose({ openLogin })
</script>

<template>
  <div class="cloud-tree">
    <p v-if="!accounts.length" class="state">暂无云存储连接</p>
    <div v-for="item in accounts" :key="item.id">
      <div class="tree-row host-row">
        <span
          class="twist"
          :class="{ open: expanded[item.id] }"
          role="button"
          :title="expanded[item.id] ? '折叠' : '展开下一级'"
          :aria-label="`${expanded[item.id] ? '折叠' : '展开'}${item.label}`"
          @click.stop="toggle(item)"
        >
          <Icon name="chevron-right" :size="11" />
        </span>
        <button class="row-main" :title="`打开 ${cloudProviderName(item.provider)}`" @click="open(item)">
          <Icon
            :name="item.provider === 'baidu' ? 'cloud' : 'cloud-bolt'"
            :size="18"
            class="provider-icon"
            :class="`provider-${item.provider}`"
          />
          <span class="label">{{ item.label }}</span>
          <small>{{ cloudProviderName(item.provider) }}</small>
        </button>
        <div class="row-actions" :class="{ 'menu-open': openMenuId === item.id }" @keydown.esc.stop="openMenuId = null">
          <button
            class="icon-btn"
            title="刷新"
            aria-label="刷新云盘目录"
            :disabled="loading[item.id]"
            @click.stop="refresh(item)"
          >
            <Icon name="refresh" :size="13" />
          </button>
          <div class="menu-wrap">
            <button
              class="icon-btn"
              title="更多操作"
              :aria-label="`${item.label}更多操作`"
              aria-haspopup="menu"
              :aria-expanded="openMenuId === item.id"
              @click.stop="openMenuId = openMenuId === item.id ? null : item.id"
            >
              <Icon name="more" :size="14" />
            </button>
            <div v-if="openMenuId === item.id" class="menu" role="menu" @click.stop>
              <button role="menuitem" @click="removeAccount(item)"><Icon name="x" :size="13" />删除账号</button>
            </div>
          </div>
        </div>
      </div>
      <div v-if="expanded[item.id]">
        <p v-if="loading[item.id]" class="state nested">正在读取云盘目录…</p>
        <p v-else-if="errors[item.id]" class="state nested error">{{ errors[item.id] }}</p>
        <p v-else-if="!(roots[item.id] ?? []).length" class="state nested">云盘根目录为空</p>
        <RemoteTreeNode
          v-for="entry in roots[item.id] || []"
          :key="entry.path"
          :location="locationOf(item)"
          :name="entry.name"
          :path="entry.path"
          :depth="2"
        />
      </div>
    </div>

    <Teleport to="body">
      <div v-if="showForm" class="dialog-backdrop" tabindex="-1" @keydown.esc.prevent.stop="closeLogin">
        <section class="dialog" role="dialog" aria-modal="true" aria-labelledby="cloud-dialog-title">
          <h2 id="cloud-dialog-title">添加云存储账号</h2>
          <form class="dialog-form" @submit.prevent="submitLogin">
            <label>服务商
              <select v-model="provider" aria-label="云存储服务商" @change="formError = ''">
                <option value="baidu">百度网盘</option>
                <option value="xunlei">迅雷云盘</option>
              </select>
            </label>
            <label>名称（可选）
              <input v-model="label" :placeholder="provider === 'baidu' ? '例如：我的百度网盘' : '例如：我的迅雷云盘'" />
            </label>

            <template v-if="provider === 'baidu'">
              <label>AppKey
                <input v-model="clientId" placeholder="百度网盘开放平台 AppKey" autocomplete="off" />
              </label>
              <label>SecretKey
                <input v-model="clientSecret" type="password" placeholder="百度网盘开放平台 SecretKey" autocomplete="new-password" />
              </label>
              <p class="dialog-hint">
                登录会打开系统浏览器完成百度账号授权，令牌保存在系统凭据库。请在百度网盘开放平台创建应用，
                并将回调地址设为 <code>http://127.0.0.1:17890/oauth/callback</code>。
              </p>
            </template>

            <template v-else>
              <label>登录方式
                <select v-model="xunleiMode" aria-label="迅雷登录方式" @change="formError = ''">
                  <option value="password">账号密码</option>
                  <option value="token">刷新令牌</option>
                </select>
              </label>
              <template v-if="xunleiMode === 'password'">
                <label>账号
                  <input v-model="username" placeholder="迅雷手机号 / 邮箱" autocomplete="username" />
                </label>
                <label>密码
                  <input v-model="password" type="password" autocomplete="current-password" />
                </label>
              </template>
              <label v-else>刷新令牌
                <input v-model="refreshToken" placeholder="粘贴迅雷 refresh token" autocomplete="off" />
              </label>
              <p class="dialog-hint">
                迅雷登录可能触发人机验证；若提示需要验证，请先打开验证链接完成，再改用刷新令牌登录。
              </p>
            </template>

            <p v-if="formError" class="dialog-error">{{ formError }}</p>
            <p v-if="captchaUrl" class="dialog-hint">
              <button type="button" class="link" @click="openCaptcha">打开验证页面</button>
            </p>
            <div class="dialog-actions">
              <button type="button" class="secondary" :disabled="busy" @click="closeLogin">取消</button>
              <button type="submit" class="primary" :disabled="busy">
                {{ busy ? (provider === 'baidu' ? '等待浏览器授权…' : '登录中…') : '登录' }}
              </button>
            </div>
          </form>
        </section>
      </div>
    </Teleport>
  </div>
</template>

<style scoped>
.cloud-tree { display: flex; flex-direction: column; gap: 3px; }
.tree-row { display: flex; align-items: center; height: var(--sidebar-row-h); border-radius: var(--radius); }
.tree-row:hover { background: var(--bg-hover); }
.row-main { display: flex; align-items: center; gap: 4px; flex: 1; min-width: 0; height: 100%; padding: 0 6px; text-align: left; color: var(--fg-folder); font-size: 1em; }
.label { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.row-main small { margin-left: auto; color: var(--fg-dim); font-size: 0.8333em; }
.host-row .twist { margin-left: 18px; }
.twist { display: flex; align-items: center; justify-content: center; width: 12px; flex: none; color: var(--fg-dim); cursor: pointer; transition: transform .12s ease; }
.twist:hover { color: var(--fg); }
.twist.open { transform: rotate(90deg); }
.provider-icon { flex: none; }
.provider-icon :deep(path) { fill: currentColor; }
.provider-baidu { color: #3f7cf5; }
.provider-xunlei { color: #17a2d8; }
.row-actions { display: flex; align-items: center; gap: 2px; flex: none; }
.menu-wrap { position: relative; }
.icon-btn { display: flex; align-items: center; justify-content: center; flex: none; width: 20px; height: 20px; color: var(--fg-dim); border-radius: var(--radius); }
.icon-btn:hover { background: var(--bg-active); color: var(--fg); }
.icon-btn:disabled { cursor: default; }
.icon-btn:disabled :deep(svg) { opacity: .4; }
.menu { position: absolute; z-index: 20; top: calc(100% + 3px); right: 0; min-width: 112px; padding: 4px; border: 1px solid var(--border); border-radius: var(--radius); background: var(--bg-panel); box-shadow: var(--shadow-md, var(--shadow-lg)); }
.menu button { display: flex; align-items: center; gap: 8px; width: 100%; padding: 6px 8px; border-radius: var(--radius); color: var(--fg); text-align: left; font-size: 0.9167em; white-space: nowrap; }
.menu button:hover { background: var(--bg-hover); }
.row-actions { opacity: 0; transition: opacity 120ms ease; }
.tree-row:hover .row-actions, .tree-row:focus-within .row-actions, .tree-row .row-actions.menu-open { opacity: 1; }
.state { margin: 3px 6px 5px 30px; font-size: 0.9167em; color: var(--fg-dim); overflow-wrap: anywhere; }
.state.nested { margin-left: 42px; }
.state.error { color: var(--danger); }
.dialog-backdrop { position: fixed; inset: 0; z-index: 1000; display: flex; align-items: center; justify-content: center; padding: 20px; background: rgb(0 0 0 / 45%); }
.dialog { width: min(420px, 100%); max-height: min(85vh, 720px); overflow: auto; padding: 18px; border: 1px solid var(--border-strong); border-radius: 10px; background: var(--bg-panel); box-shadow: var(--shadow-lg); }
.dialog h2 { margin: 0 0 16px; color: var(--fg); font-size: 1.1667em; font-weight: 600; }
.dialog-form { display: flex; flex-direction: column; gap: 12px; }
.dialog-form label { display: flex; flex-direction: column; gap: 5px; color: var(--fg-muted); font-size: 0.9167em; }
.dialog-form input, .dialog-form select { min-width: 0; width: 100%; padding: 7px 8px; border: 1px solid var(--border-strong); border-radius: var(--radius); background: var(--bg-input); color: var(--fg); font: inherit; font-size: 1em; }
.dialog-hint { margin: 0; color: var(--fg-dim); font-size: 0.8333em; line-height: 1.5; overflow-wrap: anywhere; }
.dialog-hint code { padding: 1px 4px; border-radius: 3px; background: var(--bg-hover); color: var(--fg); font-size: 0.95em; }
.dialog-error { margin: 0; color: var(--danger); font-size: 0.9167em; overflow-wrap: anywhere; }
.link { padding: 0; color: var(--accent); font: inherit; text-decoration: underline; }
.dialog-actions { display: flex; align-items: center; justify-content: flex-end; gap: 8px; margin-top: 4px; }
.dialog-actions button { padding: 6px 12px; border: 1px solid var(--border); border-radius: var(--radius); font-size: 0.9167em; }
.dialog-actions button:disabled { opacity: 0.6; cursor: default; }
.dialog-actions .secondary { color: var(--fg); background: var(--bg-hover); }
.dialog-actions .primary { border-color: var(--accent); color: var(--accent-fg); background: var(--accent); }
</style>

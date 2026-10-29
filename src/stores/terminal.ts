// SSH 终端会话状态：一对多事件路由 + 每个 xterm 实例的字节缓冲。
import { computed, ref } from 'vue'
import { defineStore } from 'pinia'
import { api, events } from '../api'
import { resolveRemoteAuth } from '../shell/remote'
import type { RemoteLocation } from '../types'

export type TerminalStatus = 'connecting' | 'connected' | 'closed' | 'error'

export interface TerminalSession {
  id: string
  title: string
  location: RemoteLocation
  status: TerminalStatus
  /** 主机密钥指纹（连接成功后展示，便于用户核对） */
  fingerprint: string
  /** 连接失败 / 会话结束原因 */
  message: string
  createdAt: number
}

type DataHandler = (bytes: Uint8Array) => void

const DOCK_HEIGHT_KEY = 'stelix.terminalDockHeight'
const DOCK_HEIGHT_MIN = 140
const DOCK_HEIGHT_MAX = 720
const DOCK_HEIGHT_DEF = 280

function readDockHeight(): number {
  try {
    const value = Number(localStorage.getItem(DOCK_HEIGHT_KEY))
    return Number.isFinite(value) && value >= DOCK_HEIGHT_MIN && value <= DOCK_HEIGHT_MAX
      ? value
      : DOCK_HEIGHT_DEF
  } catch {
    return DOCK_HEIGHT_DEF
  }
}

function decodeBase64(data: string): Uint8Array {
  const binary = atob(data)
  const bytes = new Uint8Array(binary.length)
  for (let i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i)
  return bytes
}

export const useTerminalStore = defineStore('terminal', () => {
  const sessions = ref<TerminalSession[]>([])
  const activeId = ref<string | null>(null)
  const visible = ref(false)
  const dockHeight = ref(readDockHeight())

  /** sessionId -> 已挂载 xterm 的写入回调 */
  const handlers = new Map<string, Set<DataHandler>>()
  /** 终端组件尚未挂载时暂存输出（连接成功瞬间可能先于 DOM 到达） */
  const pending = new Map<string, Uint8Array[]>()
  const pendingBytes = new Map<string, number>()

  const activeSession = computed(
    () => sessions.value.find((session) => session.id === activeId.value) ?? null,
  )

  let listening = false

  async function ensureListening() {
    if (listening) return
    listening = true
    await events.onSshData(({ sessionId, data }) => {
      dispatch(sessionId, decodeBase64(data))
    })
    await events.onSshExit(({ sessionId, message, code }) => {
      const session = sessions.value.find((item) => item.id === sessionId)
      if (!session) return
      session.status = 'closed'
      session.message = message
      const suffix = code === null ? '' : `，退出码 ${code}`
      dispatch(
        sessionId,
        new TextEncoder().encode(`\r\n\x1b[2m[${message}${suffix}]\x1b[0m\r\n`),
      )
      // 会话已结束：释放尚未挂载的缓冲，避免内存长期占用。
      if (!handlers.has(sessionId)) {
        pending.delete(sessionId)
        pendingBytes.delete(sessionId)
      }
    })
  }

  function dispatch(sessionId: string, bytes: Uint8Array) {
    const set = handlers.get(sessionId)
    if (set?.size) {
      for (const handler of set) handler(bytes)
      return
    }
    const chunks = pending.get(sessionId) ?? []
    chunks.push(bytes)
    pending.set(sessionId, chunks)
    const total = (pendingBytes.get(sessionId) ?? 0) + bytes.length
    pendingBytes.set(sessionId, total)
    // 防御性上限：标签未挂载时最多缓存 4MB 输出。
    if (total > 4 * 1024 * 1024) {
      pending.delete(sessionId)
      pendingBytes.delete(sessionId)
    }
  }

  /** 终端组件挂载：注册写入回调并回放挂起输出。返回解绑函数。 */
  function attach(sessionId: string, handler: DataHandler): () => void {
    const set = handlers.get(sessionId) ?? new Set<DataHandler>()
    set.add(handler)
    handlers.set(sessionId, set)
    const chunks = pending.get(sessionId)
    if (chunks?.length) {
      for (const chunk of chunks) handler(chunk)
      pending.delete(sessionId)
      pendingBytes.delete(sessionId)
    }
    return () => {
      const current = handlers.get(sessionId)
      if (!current) return
      current.delete(handler)
      if (!current.size) handlers.delete(sessionId)
    }
  }

  /** 连接尚未完成就被用户关闭的会话：拿到后端会话 id 后立即补发关闭。 */
  const cancelled = new Set<string>()

  function patch(id: string, values: Partial<TerminalSession>) {
    const session = sessions.value.find((item) => item.id === id)
    if (session) Object.assign(session, values)
  }

  /** 建立一条 SSH 终端会话（密码 / 私钥由 `resolveRemoteAuth` 从凭据库解析）。 */
  async function open(location: RemoteLocation): Promise<string> {
    await ensureListening()
    const id = crypto.randomUUID()
    const title = location.label || location.host
    sessions.value.push({
      id,
      title,
      location,
      status: 'connecting',
      fingerprint: '',
      message: '',
      createdAt: Date.now(),
    })
    activeId.value = id
    visible.value = true
    try {
      const auth = await resolveRemoteAuth(location)
      if (cancelled.delete(id)) return id
      const info = await api.sshConnect({
        sessionId: id,
        host: location.host,
        port: location.port,
        username: location.username,
        ...auth,
        cols: 120,
        rows: 32,
      })
      if (cancelled.delete(id)) {
        // 握手期间用户已关闭标签：后端此刻才登记会话，立刻回收。
        void api.sshClose(id).catch(() => {})
        return id
      }
      patch(id, { status: 'connected', fingerprint: info.fingerprint })
    } catch (error) {
      cancelled.delete(id)
      if (!sessions.value.some((item) => item.id === id)) return id
      patch(id, { status: 'error', message: String(error) })
    }
    return id
  }

  /** 向终端写入键盘输入。 */
  function write(id: string, data: string) {
    const session = sessions.value.find((item) => item.id === id)
    if (!session || session.status !== 'connected') return
    void api.sshWrite(id, data).catch((error) => {
      patch(id, { status: 'error', message: String(error) })
    })
  }

  /** 终端尺寸变化（xterm cols / rows）。 */
  function resize(id: string, cols: number, rows: number) {
    const session = sessions.value.find((item) => item.id === id)
    if (!session || session.status !== 'connected') return
    void api.sshResize(id, cols, rows).catch(() => {})
  }

  /** 关闭终端标签；`notifyBackend=false` 用于后端已结束的会话。 */
  function close(id: string, notifyBackend = true) {
    const session = sessions.value.find((item) => item.id === id)
    if (session?.status === 'connecting') cancelled.add(id)
    else if (notifyBackend) void api.sshClose(id).catch(() => {})
    const index = sessions.value.findIndex((item) => item.id === id)
    if (index < 0) return
    sessions.value.splice(index, 1)
    handlers.delete(id)
    pending.delete(id)
    pendingBytes.delete(id)
    if (activeId.value === id) {
      activeId.value = sessions.value[Math.max(0, index - 1)]?.id ?? null
    }
    if (!sessions.value.length) visible.value = false
  }

  function setActive(id: string) {
    if (sessions.value.some((item) => item.id === id)) activeId.value = id
  }

  function setVisible(value: boolean) {
    visible.value = value
  }

  function toggleVisible() {
    if (!sessions.value.length) return
    visible.value = !visible.value
  }

  function setDockHeight(height: number) {
    dockHeight.value = Math.min(DOCK_HEIGHT_MAX, Math.max(DOCK_HEIGHT_MIN, Math.round(height)))
    try {
      localStorage.setItem(DOCK_HEIGHT_KEY, String(dockHeight.value))
    } catch {
      /* 忽略持久化失败 */
    }
  }

  return {
    sessions,
    activeId,
    activeSession,
    visible,
    dockHeight,
    open,
    write,
    resize,
    close,
    setActive,
    setVisible,
    toggleVisible,
    setDockHeight,
    attach,
  }
})

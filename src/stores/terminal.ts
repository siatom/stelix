// SSH 终端会话状态：一对多事件路由 + 每个 xterm 实例的字节缓冲。
//
// 终端不再使用独立浮层：每个会话对应文件窗格里的一个 `kind === 'terminal'` 标签，
// 随标签挂载 / 关闭（见 `stores/pane.ts` 的 `openTerminal` 与 `Pane.vue`）。
import { ref } from 'vue'
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

function decodeBase64(data: string): Uint8Array {
  const binary = atob(data)
  const bytes = new Uint8Array(binary.length)
  for (let i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i)
  return bytes
}

export const useTerminalStore = defineStore('terminal', () => {
  const sessions = ref<TerminalSession[]>([])

  /** sessionId -> 已挂载 xterm 的写入回调 */
  const handlers = new Map<string, Set<DataHandler>>()
  /** 终端组件尚未挂载时暂存输出（连接成功瞬间可能先于 DOM 到达） */
  const pending = new Map<string, Uint8Array[]>()
  const pendingBytes = new Map<string, number>()

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

  /**
   * 建立一条 SSH 终端会话（密码 / 私钥由 `resolveRemoteAuth` 从凭据库解析）。
   *
   * 同步登记会话并返回 id（窗格据此创建标签），连接过程在后台继续；
   * 同一主机重复调用由调用方复用标签，这里不做去重。
   */
  function open(location: RemoteLocation): string {
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
    void connect(id, location)
    return id
  }

  async function connect(id: string, location: RemoteLocation) {
    try {
      await ensureListening()
      const auth = await resolveRemoteAuth(location)
      if (cancelled.delete(id)) return
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
        return
      }
      patch(id, { status: 'connected', fingerprint: info.fingerprint })
    } catch (error) {
      cancelled.delete(id)
      if (!sessions.value.some((item) => item.id === id)) return
      patch(id, { status: 'error', message: String(error) })
    }
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

  /**
   * 关闭终端会话（关闭标签时调用）。
   *
   * `notifyBackend = false` 仅用于后端已结束的会话；连接中的会话先登记取消，
   * 等握手结束拿到后端会话后立即回收。
   */
  function close(id: string, notifyBackend = true) {
    const session = sessions.value.find((item) => item.id === id)
    if (session?.status === 'connecting') cancelled.add(id)
    else if (notifyBackend) void api.sshClose(id).catch(() => {})
    const index = sessions.value.findIndex((item) => item.id === id)
    if (index >= 0) sessions.value.splice(index, 1)
    handlers.delete(id)
    pending.delete(id)
    pendingBytes.delete(id)
  }

  return {
    sessions,
    open,
    write,
    resize,
    close,
    attach,
  }
})

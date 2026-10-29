// 远程（FTP / SFTP / SSH 终端）连接的前端辅助：凭据内存缓存 + POSIX 路径工具。
import { api } from '../api'
import type { RemoteLocation } from '../types'

/**
 * 连接密码 / 私钥口令的内存缓存。
 *
 * - 密码**不**进 `TabState`，也**不**落 localStorage（标签页持久化会跳过远程标签）；
 * - 未命中时回落到系统凭据库（`api.getRemotePassword`）；侧栏输入的新密码写回这里，
 *   这样「侧栏改密码 → 窗格重连」不需要重新读取凭据库。
 */
const passwords = new Map<string, string>()
const keyPassphrases = new Map<string, string>()

export function setRemotePassword(hostId: string, password: string) {
  if (password) passwords.set(hostId, password)
  else passwords.delete(hostId)
}

export async function resolveRemotePassword(hostId: string): Promise<string> {
  const cached = passwords.get(hostId)
  if (cached !== undefined) return cached
  try {
    const stored = (await api.getRemotePassword(hostId)) ?? ''
    passwords.set(hostId, stored)
    return stored
  } catch {
    // 凭据库不可用时按空密码尝试（允许匿名 FTP）。
    return ''
  }
}

export function setRemoteKeyPassphrase(hostId: string, passphrase: string) {
  if (passphrase) keyPassphrases.set(hostId, passphrase)
  else keyPassphrases.delete(hostId)
}

export async function resolveRemoteKeyPassphrase(hostId: string): Promise<string> {
  const cached = keyPassphrases.get(hostId)
  if (cached !== undefined) return cached
  try {
    const stored = (await api.getRemoteKeyPassphrase(hostId)) ?? ''
    keyPassphrases.set(hostId, stored)
    return stored
  } catch {
    // 没有口令的私钥是常见情况，读取失败按空口令处理。
    return ''
  }
}

/** 一次连接实际要传给后端的认证信息。 */
export interface ResolvedRemoteAuth {
  password: string
  authMethod: 'password' | 'key'
  privateKey: string
  keyPassphrase: string
}

/** 依据主机配置解析出密码或私钥 + 口令（需要读系统凭据库）。 */
export async function resolveRemoteAuth(location: RemoteLocation): Promise<ResolvedRemoteAuth> {
  if (location.authMethod === 'key') {
    return {
      password: '',
      authMethod: 'key',
      privateKey: location.privateKey ?? '',
      keyPassphrase: await resolveRemoteKeyPassphrase(location.id),
    }
  }
  return {
    password: await resolveRemotePassword(location.id),
    authMethod: 'password',
    privateKey: '',
    keyPassphrase: '',
  }
}

/** 去掉尾部斜杠（根路径保持 `/`）。 */
export function normalizeRemotePath(path: string): string {
  const trimmed = path.replace(/\/+$/, '')
  return trimmed || '/'
}

/** 远程路径的父目录（POSIX 语义；根返回自身）。 */
export function remoteDirname(path: string): string {
  const normalized = normalizeRemotePath(path)
  if (normalized === '/') return '/'
  const index = normalized.lastIndexOf('/')
  return index <= 0 ? '/' : normalized.slice(0, index)
}

/** 展示用地址：`sftp://host:22/var/www`。 */
export function formatRemoteUrl(location: RemoteLocation, path: string): string {
  const protocol = location.protocol === 'sftp' ? 'sftp' : 'ftp'
  return `${protocol}://${location.host}:${location.port}${normalizeRemotePath(path)}`
}

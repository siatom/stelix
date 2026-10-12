// 远程（FTP / SFTP）连接的前端辅助：密码内存缓存 + POSIX 路径工具。
import { api } from '../api'
import type { RemoteLocation } from '../types'

/**
 * 连接密码的内存缓存。
 *
 * - 密码**不**进 `TabState`，也**不**落 localStorage（标签页持久化会跳过远程标签）；
 * - 未命中时回落到系统凭据库（`api.getRemotePassword`）；侧栏输入的新密码写回这里，
 *   这样「侧栏改密码 → 窗格重连」不需要重新读取凭据库。
 */
const passwords = new Map<string, string>()

export function setRemotePassword(hostId: string, password: string) {
  if (password) passwords.set(hostId, password)
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
  return `${location.protocol}://${location.host}:${location.port}${normalizeRemotePath(path)}`
}

import { api } from '../api'

/**
 * 目录监听注册表。
 *
 * 各来源（窗格标签集合 / 目录树节点）上报自己关心的目录，取**并集**后全量同步给后端；
 * 后端为这些目录挂 `notify` watcher，变更经防抖后 emit `fs:changed`。
 *
 * 同一路径被多个来源引用没问题 —— 取并集，来源各自增删互不影响。
 */

/** 广播给目录树等组件的变更事件（`detail` = 内容变化的目录列表）。 */
export const FS_CHANGED_EVENT = 'stelix:fs-changed'

/** 本地绝对路径才值得挂 watcher（Windows 盘符 / UNC、macOS / Linux 的 / 开头路径）。 */
const LOCAL_PATH_RE = /^(?:[A-Za-z]:[\\/]|\\\\|\/)/

/** 归一化路径用于比较：统一分隔符、去掉尾部分隔符、小写。 */
export function normalizePath(path: string): string {
  const sep = /^[A-Za-z]:/.test(path) || path.startsWith('\\\\') ? '\\' : '/'
  return path.replace(/[\\/]+$/, '').replace(/\\/g, sep).toLowerCase()
}

const sources = new Map<string, Set<string>>()
let timer: number | null = null
let lastSent = ''

function unionPaths(): string[] {
  const all = new Set<string>()
  for (const set of sources.values()) {
    for (const path of set) all.add(path)
  }
  return Array.from(all).sort()
}

async function flush() {
  timer = null
  const paths = unionPaths()
  const key = paths.join('\n')
  if (key === lastSent) return
  lastSent = key
  try {
    await api.watchDirsSet(paths)
  } catch {
    lastSent = '' // 同步失败后允许下次重试
  }
}

function schedule() {
  if (timer !== null) return
  timer = window.setTimeout(() => void flush(), 150)
}

/** 某个来源上报它关心的目录集合（全量替换该来源此前的内容）。 */
export function setWatchSource(sourceId: string, paths: string[]) {
  const filtered = paths.filter((path) => LOCAL_PATH_RE.test(path))
  if (!filtered.length) {
    if (sources.delete(sourceId)) schedule()
    return
  }
  sources.set(sourceId, new Set(filtered))
  schedule()
}

/** 移除某个来源（组件卸载 / 树节点折叠）。 */
export function clearWatchSource(sourceId: string) {
  if (sources.delete(sourceId)) schedule()
}

/** 把后端上报的目录集合广播给目录树等组件。 */
export function broadcastFsChanged(dirs: string[]) {
  window.dispatchEvent(new CustomEvent<string[]>(FS_CHANGED_EVENT, { detail: dirs }))
}

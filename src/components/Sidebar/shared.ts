import { desktopDir, documentDir, downloadDir, homeDir, pictureDir } from '@tauri-apps/api/path'
import type { IconName } from '../icons/paths'

/** 快速访问条目 */
export interface QuickLocation {
  label: string
  icon: IconName
  path: string
}

/** 最近一个有活动的 24 小时窗口内双击打开过文件的目录统计 */
export interface RecentDirectory extends QuickLocation {
  count: number
  lastOpened: number
}

/** 目录树节点 */
export interface TreeNodeData {
  name: string
  path: string
}

/** 单个窗格在标签页组中的快照 */
export interface TabGroupPane {
  tabs: string[]
  active: number
}

/** 标签页组（工作区）快照 */
export interface TabGroup {
  id: string
  name: string
  panes: TabGroupPane[]
  activePaneId: string
  createdAt: number
}

export const FAVORITES_KEY = 'file-povit.favorites'
export const TABGROUPS_KEY = 'file-povit.tabgroups'
export const FAVORITES_CHANGED_EVENT = 'file-povit:favorites-changed'
export const RECENT_DIRECTORIES_KEY = 'file-povit.recent-directories'
export const RECENT_DIRECTORIES_CHANGED_EVENT = 'file-povit:recent-directories-changed'
const RECENT_WINDOW_MS = 24 * 60 * 60 * 1000
const RECENT_EVENT_LIMIT = 10000

interface FileOpenEvent {
  path: string
  openedAt: number
}

/** 系统路径不可用时的兜底常用位置 */
const FALLBACK_LOCATIONS: QuickLocation[] = [
  { label: 'C 盘根目录', icon: 'drive', path: 'C:\\' },
  { label: '公共用户', icon: 'user', path: 'C:\\Users\\Public' },
  { label: 'Windows', icon: 'windows', path: 'C:\\Windows' },
]

/**
 * 解析用户主目录 / 桌面 / 下载 / 文档 / 图片。
 * 依赖 Tauri 的 path 插件（core:default 已包含 path 权限），
 * 任何失败都回退到固定常用路径，保证界面不白屏。
 */
export async function resolveSystemLocations(): Promise<QuickLocation[]> {
  try {
    const [home, desktop, downloads, documents, pictures] = await Promise.all([
      homeDir(),
      desktopDir(),
      downloadDir(),
      documentDir(),
      pictureDir(),
    ])
    const items: QuickLocation[] = [
      { label: '主目录', icon: 'home', path: home },
      { label: '桌面', icon: 'monitor', path: desktop },
      { label: '下载', icon: 'download', path: downloads },
      { label: '文档', icon: 'file-text', path: documents },
      { label: '图片', icon: 'image', path: pictures },
    ]
    return items.filter((it) => !!it.path)
  } catch {
    return FALLBACK_LOCATIONS
  }
}

export function loadFavorites(): QuickLocation[] {
  try {
    const raw = localStorage.getItem(FAVORITES_KEY)
    if (!raw) return []
    const parsed: unknown = JSON.parse(raw)
    if (!Array.isArray(parsed)) return []
    return (parsed as unknown[])
      .filter(
        (x): x is QuickLocation =>
          !!x && typeof x === 'object' && typeof (x as QuickLocation).path === 'string',
      )
      .map((x) => ({
        label: x.label || x.path,
        icon: (x.icon || 'star') as IconName,
        path: x.path,
      }))
  } catch {
    return []
  }
}

export function saveFavorites(items: QuickLocation[]): void {
  try {
    localStorage.setItem(FAVORITES_KEY, JSON.stringify(items))
    window.dispatchEvent(new Event(FAVORITES_CHANGED_EVENT))
  } catch {
    /* 忽略持久化失败（隐私模式 / 配额） */
  }
}

export function addFavorite(path: string): boolean {
  const items = loadFavorites()
  if (items.some((item) => item.path === path)) return false
  const label = basename(path) || path
  saveFavorites([...items, { label, icon: 'star', path }])
  return true
}

/** 记录一次通过双击成功打开文件的行为，供最近高频目录排序。 */
export function recordOpenedFileDirectory(path: string): void {
  const now = Date.now()
  let events: FileOpenEvent[] = []
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(RECENT_DIRECTORIES_KEY) || '[]')
    if (Array.isArray(parsed)) {
      events = parsed.filter(
        (event): event is FileOpenEvent =>
          !!event &&
          typeof event === 'object' &&
          typeof (event as FileOpenEvent).path === 'string' &&
          typeof (event as FileOpenEvent).openedAt === 'number',
      )
    }
    events.push({ path, openedAt: now })
    if (events.length > RECENT_EVENT_LIMIT) events = events.slice(-RECENT_EVENT_LIMIT)
    localStorage.setItem(RECENT_DIRECTORIES_KEY, JSON.stringify(events))
    window.dispatchEvent(new Event(RECENT_DIRECTORIES_CHANGED_EVENT))
  } catch {
    /* 忽略本地记录失败，不影响文件打开 */
  }
}

/** 返回滚动 24 小时内按打开次数排序的目录；无记录时回退到更早的最近活动窗口。 */
export function loadRecentDirectories(limit = 8): RecentDirectory[] {
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(RECENT_DIRECTORIES_KEY) || '[]')
    if (!Array.isArray(parsed)) return []
    const events = (parsed as unknown[]).filter(
      (event): event is FileOpenEvent =>
        !!event &&
        typeof event === 'object' &&
        typeof (event as FileOpenEvent).path === 'string' &&
        typeof (event as FileOpenEvent).openedAt === 'number',
    )
    if (!events.length) return []

    const now = Date.now()
    const hasRecentActivity = events.some((event) => event.openedAt > now - RECENT_WINDOW_MS)
    const anchor = hasRecentActivity ? now : Math.max(...events.map((event) => event.openedAt))
    const cutoff = anchor - RECENT_WINDOW_MS
    const counts = new Map<string, RecentDirectory>()
    for (const { path, openedAt } of events) {
      if (openedAt <= cutoff || openedAt > anchor) continue
      const key = path.replace(/\//g, '\\').replace(/\\+$/, '').toLowerCase()
      const item = counts.get(key)
      if (item) {
        item.count += 1
        item.lastOpened = Math.max(item.lastOpened, openedAt)
      } else {
        counts.set(key, { path, label: basename(path) || path, icon: 'folder', count: 1, lastOpened: openedAt })
      }
    }
    return [...counts.values()]
      .sort((a, b) => b.count - a.count || b.lastOpened - a.lastOpened)
      .slice(0, limit)
  } catch {
    return []
  }
}

export function loadTabGroups(): TabGroup[] {
  try {
    const raw = localStorage.getItem(TABGROUPS_KEY)
    if (!raw) return []
    const parsed: unknown = JSON.parse(raw)
    if (!Array.isArray(parsed)) return []
    return parsed as TabGroup[]
  } catch {
    return []
  }
}

export function saveTabGroups(items: TabGroup[]): void {
  try {
    localStorage.setItem(TABGROUPS_KEY, JSON.stringify(items))
  } catch {
    /* 忽略持久化失败 */
  }
}

/** 取路径最后一段作为显示名 */
export function basename(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean)
  return parts.length ? parts[parts.length - 1] : path
}

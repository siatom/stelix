import { desktopDir, documentDir, downloadDir, homeDir, pictureDir } from '@tauri-apps/api/path'
import type { IconName } from '../icons/paths'

/** 快速访问条目 */
export interface QuickLocation {
  label: string
  icon: IconName
  path: string
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

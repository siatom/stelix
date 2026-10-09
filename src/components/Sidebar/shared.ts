import { desktopDir, documentDir, downloadDir, homeDir } from '@tauri-apps/api/path'

/** 快速访问条目 */
export interface QuickLocation {
  label: string
  icon: string
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

/** 系统路径不可用时的兜底常用位置 */
const FALLBACK_LOCATIONS: QuickLocation[] = [
  { label: 'C 盘根目录', icon: '💽', path: 'C:\\' },
  { label: '公共用户', icon: '👤', path: 'C:\\Users\\Public' },
  { label: 'Windows', icon: '🪟', path: 'C:\\Windows' },
]

/**
 * 解析用户主目录 / 桌面 / 下载 / 文档。
 * 依赖 Tauri 的 path 插件（core:default 已包含 path 权限），
 * 任何失败都回退到固定常用路径，保证界面不白屏。
 */
export async function resolveSystemLocations(): Promise<QuickLocation[]> {
  try {
    const [home, desktop, downloads, documents] = await Promise.all([
      homeDir(),
      desktopDir(),
      downloadDir(),
      documentDir(),
    ])
    const items: QuickLocation[] = [
      { label: '主目录', icon: '🏠', path: home },
      { label: '桌面', icon: '🖥', path: desktop },
      { label: '下载', icon: '⬇', path: downloads },
      { label: '文档', icon: '📄', path: documents },
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
        icon: x.icon || '⭐',
        path: x.path,
      }))
  } catch {
    return []
  }
}

export function saveFavorites(items: QuickLocation[]): void {
  try {
    localStorage.setItem(FAVORITES_KEY, JSON.stringify(items))
  } catch {
    /* 忽略持久化失败（隐私模式 / 配额） */
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

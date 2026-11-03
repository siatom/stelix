// Pane 视图层纯工具函数（路径 / 格式化 / 图标）—— WS-D 拥有。
import type { FileEntry } from '../../types'
import type { IconName } from '../icons/paths'

/** Windows 回收站的 Shell 命名空间路径，用于向系统请求回收站图标。 */
export const RECYCLE_BIN_SHELL_PATH = '::{645FF040-5081-101B-9F08-00AA002F954E}'

/** 去掉尾部斜杠后的路径 */
function trimTrailing(p: string): string {
  const t = p.replace(/[\\/]+$/, '')
  return t.length ? t : p
}

/** 取路径最后一段（目录名 / 文件名） */
export function basename(p: string): string {
  const parts = trimTrailing(p).split(/[\\/]/).filter(Boolean)
  return parts.length ? parts[parts.length - 1] : p
}

/** 取父目录（Windows 风格；根返回自身） */
export function dirname(p: string): string {
  const norm = trimTrailing(p)
  const shareRoot = norm.match(/^(\\\\[^\\/]+[\\/][^\\/]+)/)
  if (shareRoot && norm.length <= shareRoot[1].length) return shareRoot[1]
  const idx = Math.max(norm.lastIndexOf('\\'), norm.lastIndexOf('/'))
  if (idx < 0) return p
  // C:\foo -> C:\
  if (idx === 2 && norm[1] === ':') return norm.slice(0, 3)
  const head = norm.slice(0, idx)
  if (head.length <= 2 && head[1] === ':') return `${head}\\`
  return head
}

/** 拼接子路径（自动选择分隔符） */
export function joinPath(dir: string, name: string): string {
  if (!dir) return name
  const sep = /^[A-Za-z]:/.test(dir) || dir.includes('\\') ? '\\' : '/'
  const trimmed = dir.replace(/[\\/]+$/, '')
  if (!trimmed) return name
  return `${trimmed}${sep}${name}`
}

/** 人类可读大小 */
export function formatSize(bytes: number): string {
  if (!bytes || bytes < 0) return '0 B'
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  let value = bytes
  let i = 0
  while (value >= 1024 && i < units.length - 1) {
    value /= 1024
    i += 1
  }
  if (i === 0) return `${value} B`
  return `${value >= 100 ? value.toFixed(0) : value.toFixed(1)} ${units[i]}`
}

/** YYYY-MM-DD HH:mm */
export function formatTime(ms: number): string {
  if (!ms) return ''
  const d = new Date(ms)
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`
}

/** 文件大类，用于选择图标与配色 */
export type FileKind =
  | 'folder'
  | 'doc'
  | 'image'
  | 'video'
  | 'audio'
  | 'archive'
  | 'app'
  | 'code'
  | 'disc'
  | 'file'

const KIND_BY_EXT: Record<string, FileKind> = {}

function register(kind: FileKind, exts: string) {
  for (const e of exts.split(' ')) KIND_BY_EXT[e] = kind
}

/**
 * 某个文件大类收录的扩展名（小写、不含点），供搜索类型筛选复用同一份表。
 * 按「最终归属」取（如 `ts` 同时出现在 video 与 code，取后注册的 code），与图标判定保持一致。
 */
export function extsOfKind(kind: FileKind): string[] {
  return Object.keys(KIND_BY_EXT).filter((ext) => KIND_BY_EXT[ext] === kind)
}

register('doc', 'txt md log rtf pdf doc docx xls xlsx ppt pptx csv odt ods')
register('image', 'jpg jpeg png gif bmp webp svg ico tif tiff avif heic psd ai')
register('video', 'mp4 mkv avi mov wmv flv webm m4v mpg mpeg ts')
register('audio', 'mp3 wav flac m4a ogg aac wma opus mid')
register('archive', 'zip rar 7z tar gz bz2 xz zst cab lz4')
register('app', 'exe msi bat cmd dll sys com scr lnk')
register('code', 'js mjs cjs ts jsx tsx vue json html htm css scss less py java c cpp h hpp cs go rs rb php swift kt scala sh ps1 yml yaml toml ini xml sql lua r pl')
register('disc', 'iso img vhd vmdk bin cue')

const KIND_ICON: Record<FileKind, IconName> = {
  folder: 'folder',
  doc: 'file-text',
  image: 'image',
  video: 'video',
  audio: 'music',
  archive: 'archive',
  app: 'executable',
  code: 'code',
  disc: 'disc',
  file: 'file',
}

/** 判断文件大类 */
export function fileKind(entry: Pick<FileEntry, 'isDir' | 'ext'>): FileKind {
  if (entry.isDir) return 'folder'
  const ext = (entry.ext ?? '').replace(/^\./, '').toLowerCase()
  return KIND_BY_EXT[ext] ?? 'file'
}

/** 根据大类给出图标名（系统图标不可用时的回退） */
export function fileIcon(entry: Pick<FileEntry, 'isDir' | 'ext'>): IconName {
  return KIND_ICON[fileKind(entry)]
}

/** 目录树里的展开/收起图标 */
export function folderIcon(expanded: boolean): IconName {
  return expanded ? 'folder-open' : 'folder'
}

/** 类型列文案 */
export function typeLabel(entry: FileEntry): string {
  if (entry.isDir) return '文件夹'
  const ext = (entry.ext ?? '').replace(/^\./, '').toUpperCase()
  return ext ? `${ext} 文件` : '文件'
}

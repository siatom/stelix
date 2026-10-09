// Pane 视图层纯工具函数（路径 / 格式化 / 图标）—— WS-D 拥有。
import type { FileEntry } from '../../types'

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
export function formatSize(bytes: number, isDir: boolean): string {
  if (isDir) return ''
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

const EXT_ICONS: Record<string, string> = {
  txt: '📃', md: '📝', log: '📃', rtf: '📃',
  pdf: '📕', doc: '📘', docx: '📘', xls: '📗', xlsx: '📗', ppt: '📙', pptx: '📙',
  jpg: '🖼️', jpeg: '🖼️', png: '🖼️', gif: '🖼️', bmp: '🖼️', webp: '🖼️', svg: '🖼️', ico: '🖼️',
  mp3: '🎵', wav: '🎵', flac: '🎵', m4a: '🎵', ogg: '🎵',
  mp4: '🎬', mkv: '🎬', avi: '🎬', mov: '🎬', wmv: '🎬', flv: '🎬',
  zip: '🗜️', rar: '🗜️', '7z': '🗜️', tar: '🗜️', gz: '🗜️', bz2: '🗜️', xz: '🗜️',
  exe: '⚙️', msi: '⚙️', bat: '⚙️', cmd: '⚙️', dll: '⚙️', sys: '⚙️',
  js: '📜', ts: '📜', jsx: '📜', tsx: '📜', vue: '📜', json: '📜', html: '📜', css: '📜',
  py: '🐍', java: '☕', c: '📜', cpp: '📜', h: '📜', hpp: '📜', cs: '📜', go: '📜', rs: '📜',
  sh: '⌨️', ps1: '⌨️', yml: '📜', yaml: '📜', toml: '📜', ini: '📜', xml: '📜',
  iso: '💿', apk: '📦', dmg: '📦',
}

/** 根据扩展名给出 emoji 图标 */
export function fileIcon(entry: FileEntry): string {
  if (entry.isDir) return '📁'
  const ext = (entry.ext ?? '').replace(/^\./, '').toLowerCase()
  return EXT_ICONS[ext] ?? '📄'
}

/** 类型列文案 */
export function typeLabel(entry: FileEntry): string {
  if (entry.isDir) return '文件夹'
  const ext = (entry.ext ?? '').replace(/^\./, '').toUpperCase()
  return ext ? `${ext} 文件` : '文件'
}

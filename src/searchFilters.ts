// 搜索「文件类型」筛选：分类 → 扩展名白名单 / 仅目录。
// 单一事实来源：搜索面板的 chips、search store（全局查询）、pane store（当前目录本地过滤）共用。
import type { FileEntry } from './types'
import { extsOfKind } from './components/Pane/utils'

export interface SearchTypeCategory {
  key: string
  label: string
  /** 仅目录 */
  isDir?: boolean
  /** 扩展名白名单（小写、不含点） */
  exts?: string[]
}

export const SEARCH_CATEGORIES: SearchTypeCategory[] = [
  { key: 'all', label: '全部' },
  { key: 'folder', label: '文件夹', isDir: true },
  { key: 'doc', label: '文档', exts: 'pdf txt rtf csv log odt ods epub chm'.split(' ') },
  { key: 'image', label: '图片', exts: extsOfKind('image') },
  { key: 'audio', label: '音频', exts: extsOfKind('audio') },
  { key: 'video', label: '视频', exts: extsOfKind('video') },
  { key: 'archive', label: '压缩包', exts: extsOfKind('archive') },
  // 可执行程序：与图标分类里的 app 不完全一致，排除 dll / sys / lnk 这类库、驱动与快捷方式
  { key: 'app', label: '可执行程序', exts: 'exe msi msix appx bat cmd com scr jar'.split(' ') },
  { key: 'code', label: '代码', exts: extsOfKind('code') },
  { key: 'word', label: 'Word', exts: 'doc docx docm dot dotx dotm'.split(' ') },
  { key: 'excel', label: 'Excel', exts: 'xls xlsx xlsm xlsb et'.split(' ') },
  { key: 'ppt', label: 'PPT', exts: 'ppt pptx pptm pps ppsx pot potx'.split(' ') },
  { key: 'pdf', label: 'PDF', exts: ['pdf'] },
  { key: 'markdown', label: 'Markdown', exts: ['md', 'markdown', 'mdx'] },
]

/** 取分类定义；`all` / 空值表示不筛选 */
export function categoryOf(key: string | null | undefined): SearchTypeCategory | undefined {
  if (!key || key === 'all') return undefined
  return SEARCH_CATEGORIES.find((c) => c.key === key)
}

/** 条目是否命中类型筛选（规则与后端 search_engine::matches 一致：目录无扩展名，选扩展名类时自动排除目录） */
export function matchesTypeFilter(
  entry: Pick<FileEntry, 'isDir' | 'ext'>,
  key: string | null | undefined,
): boolean {
  const cat = categoryOf(key)
  if (!cat) return true
  if (cat.isDir) return entry.isDir
  if (entry.isDir) return false
  const ext = (entry.ext ?? '').replace(/^\./, '').toLowerCase()
  return !!ext && !!cat.exts?.includes(ext)
}

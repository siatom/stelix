/**
 * 文件预览弹窗状态（空格预览，类似 Finder Quick Look）。
 *
 * 打开时把「当前列表里可预览的文件」拍成一份快照，上一张 / 下一张只在这份
 * 快照内移动：列表在后台刷新（目录监听）时预览内容不会被换掉，关闭后重新
 * 打开才会取新列表。
 */
import { computed, ref } from 'vue'
import { defineStore } from 'pinia'
import type { FileEntry } from '../types'

export type PreviewKind =
  | 'image'
  | 'text'
  | 'markdown'
  | 'pdf'
  | 'word'
  | 'excel'
  | 'powerpoint'
  | 'unsupported'

/** WebView2（Chromium）能直接解码的图片格式。 */
const IMAGE_EXTS = new Set(['png', 'jpg', 'jpeg', 'webp', 'gif', 'svg', 'bmp', 'ico', 'avif'])

const PDF_EXTS = new Set(['pdf'])

/** Markdown 单独成一类：默认渲染成排版好的文档，可切回源码。 */
const MARKDOWN_EXTS = new Set(['md', 'markdown', 'mdown', 'mkd'])

/** Office Open XML：只读渲染，不支持 97-2003 的 .doc/.xls/.ppt 二进制格式。 */
const WORD_EXTS = new Set(['docx', 'docm'])
const EXCEL_EXTS = new Set(['xlsx', 'xlsm'])
const POWERPOINT_EXTS = new Set(['pptx', 'pptm'])

/** 按纯文本展示的扩展名（预览时按语言做语法高亮）。 */
const TEXT_EXTS = new Set([
  'txt', 'text', 'log', 'mdx', 'rst', 'adoc',
  'json', 'jsonc', 'json5', 'jsonl', 'ndjson', 'geojson',
  'js', 'mjs', 'cjs', 'jsx', 'ts', 'mts', 'cts', 'tsx', 'vue', 'svelte', 'astro',
  'rs', 'go', 'py', 'pyi', 'rb', 'php', 'java', 'kt', 'kts', 'scala', 'groovy', 'clj', 'cljs',
  'c', 'h', 'cc', 'cpp', 'cxx', 'hpp', 'hh', 'm', 'mm', 'cs', 'fs', 'fsx', 'vb',
  'swift', 'dart', 'lua', 'pl', 'pm', 'r', 'jl', 'ex', 'exs', 'erl', 'hrl', 'hs', 'ml', 'nim', 'zig',
  'sh', 'bash', 'zsh', 'fish', 'ps1', 'psm1', 'bat', 'cmd',
  'html', 'htm', 'xhtml', 'css', 'scss', 'sass', 'less', 'styl', 'stylus',
  'xml', 'xsl', 'xslt', 'svgz', 'plist', 'csproj', 'vcxproj', 'sln', 'props', 'targets',
  'toml', 'yaml', 'yml', 'ini', 'cfg', 'conf', 'config', 'properties', 'env', 'editorconfig',
  'csv', 'tsv', 'sql', 'graphql', 'gql', 'proto', 'thrift', 'avsc',
  'diff', 'patch', 'lock', 'gitignore', 'gitattributes', 'dockerignore', 'npmrc',
  'gradle', 'cmake', 'mk', 'makefile', 'dockerfile', 'terraform', 'tf', 'hcl', 'nix',
  'asm', 's', 'v', 'sv', 'vhd', 'vhdl', 'tex', 'bib', 'srt', 'ass', 'vtt',
])

/** 无扩展名但通常是文本的文件名（小写比较）。 */
const TEXT_NAMES = new Set([
  'readme', 'license', 'licence', 'changelog', 'notice', 'authors', 'contributing',
  'makefile', 'dockerfile', 'procfile', 'gemfile', 'rakefile', 'cargo.lock', 'go.mod',
  'go.sum', '.gitignore', '.gitattributes', '.editorconfig', '.env', '.npmrc',
])

export function previewKindOf(ext: string | null | undefined, name?: string): PreviewKind {
  const value = (ext ?? '').toLowerCase().replace(/^\./, '')
  if (value) {
    if (IMAGE_EXTS.has(value)) return 'image'
    if (PDF_EXTS.has(value)) return 'pdf'
    if (MARKDOWN_EXTS.has(value)) return 'markdown'
    if (WORD_EXTS.has(value)) return 'word'
    if (EXCEL_EXTS.has(value)) return 'excel'
    if (POWERPOINT_EXTS.has(value)) return 'powerpoint'
    if (TEXT_EXTS.has(value)) return 'text'
    return 'unsupported'
  }
  if (name && TEXT_NAMES.has(name.toLowerCase())) return 'text'
  return 'unsupported'
}

export interface PreviewItem {
  path: string
  name: string
  size: number
  modified: number
  ext: string | null
  kind: PreviewKind
}

function toItem(entry: FileEntry): PreviewItem {
  return {
    path: entry.path,
    name: entry.name,
    size: entry.size,
    modified: entry.modified,
    ext: entry.ext,
    kind: previewKindOf(entry.ext, entry.name),
  }
}

export const usePreviewStore = defineStore('preview', () => {
  const open = ref(false)
  /** 可预览文件快照（按当前列表顺序）；当前文件不可预览时只含它自己。 */
  const items = ref<PreviewItem[]>([])
  const index = ref(-1)
  /** 远程标签（FTP / SFTP）的路径无法本地读取，只能提示用外部程序打开。 */
  const isRemote = ref(false)

  const current = computed<PreviewItem | null>(() => items.value[index.value] ?? null)
  const kind = computed<PreviewKind>(() => current.value?.kind ?? 'unsupported')
  const canNavigate = computed(() => items.value.length > 1)

  /**
   * 打开预览。
   *
   * `entries` 用调用方可见的列表顺序（含筛选 / 排序结果），上一张下一张与它一致。
   */
  function show(entries: FileEntry[], targetPath: string, options?: { remote?: boolean }) {
    const target = entries.find((entry) => entry.path === targetPath)
    if (!target) return
    const remote = options?.remote === true
    isRemote.value = remote
    const targetKind = previewKindOf(target.ext, target.name)
    if (remote || targetKind === 'unsupported') {
      items.value = [toItem(target)]
      index.value = 0
    } else {
      const list = entries.filter(
        (entry) => !entry.isDir && previewKindOf(entry.ext, entry.name) !== 'unsupported',
      )
      items.value = list.map(toItem)
      const found = items.value.findIndex((item) => item.path === targetPath)
      index.value = found >= 0 ? found : 0
    }
    open.value = true
  }

  function toggle(entries: FileEntry[], targetPath: string, options?: { remote?: boolean }) {
    if (open.value) {
      close()
      return
    }
    show(entries, targetPath, options)
  }

  function close() {
    open.value = false
    items.value = []
    index.value = -1
    isRemote.value = false
  }

  function step(delta: number) {
    const count = items.value.length
    if (count < 2) return
    index.value = (index.value + delta + count) % count
  }

  function next() {
    step(1)
  }

  function prev() {
    step(-1)
  }

  return {
    open,
    items,
    index,
    isRemote,
    current,
    kind,
    canNavigate,
    show,
    toggle,
    close,
    next,
    prev,
  }
})

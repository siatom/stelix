<script setup lang="ts">
// 文件预览弹窗（空格预览，类似 Finder Quick Look）。
//
// - 图片走 asset 协议直接加载，支持缩放 / 适应窗口 / 原始尺寸 / 滚轮缩放 / 拖拽平移；
// - 文本走后端 read_text_preview（带 1 MiB 上限、二进制与编码识别）；
// - Markdown 复用 AI 消息的渲染器，默认显示排版结果，可切回源码；
// - Word / Excel / PowerPoint（OOXML）在前端按需加载渲染库解析，只读、不启动外部 Office；
// - PDF 交给 WebView2 内置阅读器（iframe + asset 协议）；
// - 其余格式给出占位提示，保留「用默认应用打开」。
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { convertFileSrc } from '@tauri-apps/api/core'
import { openPath } from '@tauri-apps/plugin-opener'
import { api } from '../api'
import { usePreviewStore } from '../stores/preview'
import { useUiStore } from '../stores/ui'
import type { TextPreview } from '../types'
import type { Cell as ExcelCell, Color as ExcelColor, Workbook as ExcelWorkbook } from 'exceljs'
import { renderMarkdown } from './ai/markdown'
import Icon from './icons/Icon.vue'
import type { IconName } from './icons/paths'
import { formatSize, formatTime } from './Pane/utils'

/** 超过这个大小的图片不解码：避免一次性占满显存 / 内存。 */
const MAX_IMAGE_BYTES = 64 * 1024 * 1024
/** PDF 交给内置阅读器，上限放宽一些。 */
const MAX_PDF_BYTES = 128 * 1024 * 1024
/** 文本读取上限（与后端默认值一致）。 */
const TEXT_LIMIT = 1024 * 1024
/** Office 文档要整份读进内存解析，超过这个大小直接给占位提示。 */
const MAX_OFFICE_BYTES = 48 * 1024 * 1024
/** Excel 表格只渲染前若干行列：整张表可能有几十万行，铺满 DOM 会卡死。 */
const SHEET_MAX_ROWS = 400
const SHEET_MAX_COLS = 40
/** 超过这个字符数就跳过语法高亮：高亮是同步 CPU 运算，大文件会卡住界面。 */
const HIGHLIGHT_LIMIT = 200_000

const store = usePreviewStore()
const ui = useUiStore()

const item = computed(() => store.current)
const kind = computed(() => store.kind)
const assetUrl = computed(() => (item.value ? convertFileSrc(item.value.path) : ''))

const kindIcon = computed<IconName>(() => {
  switch (kind.value) {
    case 'image':
      return 'image'
    case 'pdf':
    case 'text':
    case 'markdown':
    case 'word':
      return 'file-text'
    case 'excel':
      return 'view-details'
    case 'powerpoint':
      return 'monitor'
    default:
      return 'file'
  }
})

/** 按文本方式读取的格式（Markdown 也走这条，拿到源码后再渲染）。 */
const textual = computed(() => kind.value === 'text' || kind.value === 'markdown')
const isOffice = computed(
  () => kind.value === 'word' || kind.value === 'excel' || kind.value === 'powerpoint',
)

// ---- 文本 ----
const textPreview = ref<TextPreview | null>(null)
const textLoading = ref(false)
const textError = ref<string | null>(null)
const wrap = ref(false)
/** 刚复制完内容的短暂反馈（按钮变成「已复制」）。 */
const copied = ref(false)
let copiedTimer: number | undefined
let loadToken = 0

/** Markdown 的展示形态：默认渲染成文档，可切回源码。 */
const mdView = ref<'preview' | 'source'>('preview')
const mdHtml = computed(() => {
  const text = textPreview.value
  if (kind.value !== 'markdown' || !text || text.binary) return ''
  return renderMarkdown(text.text)
})

// ---- 语法高亮 ----

/** 高亮后的 HTML；为空时退回纯文本渲染。 */
const codeHtml = ref('')
let hlToken = 0
type Hljs = {
  highlight: (code: string, options: { language: string; ignoreIllegals: boolean }) => { value: string }
  registerLanguage: (name: string, language: unknown) => void
}
let hljsPromise: Promise<Hljs> | null = null

/** 扩展名 → highlight.js 语言标识（common 包里没有的语言单独注册）。 */
const LANG_BY_EXT: Record<string, string> = {
  js: 'javascript', mjs: 'javascript', cjs: 'javascript', jsx: 'javascript',
  ts: 'typescript', mts: 'typescript', cts: 'typescript', tsx: 'typescript',
  vue: 'xml', svelte: 'xml', astro: 'xml',
  html: 'xml', htm: 'xml', xhtml: 'xml', svg: 'xml', svgz: 'xml', xml: 'xml',
  xsl: 'xml', xslt: 'xml', plist: 'xml', csproj: 'xml', vcxproj: 'xml', props: 'xml', targets: 'xml',
  css: 'css', scss: 'scss', sass: 'scss', less: 'less', styl: 'css', stylus: 'stylus',
  json: 'json', jsonc: 'json', json5: 'json', jsonl: 'json', ndjson: 'json', geojson: 'json', avsc: 'json',
  rs: 'rust', go: 'go', py: 'python', pyi: 'python', rb: 'ruby', php: 'php', java: 'java',
  kt: 'kotlin', kts: 'kotlin', scala: 'scala', groovy: 'groovy', clj: 'clojure', cljs: 'clojure',
  c: 'c', h: 'c', cc: 'cpp', cpp: 'cpp', cxx: 'cpp', hpp: 'cpp', hh: 'cpp', m: 'objectivec', mm: 'objectivec',
  cs: 'csharp', fs: 'fsharp', fsx: 'fsharp', vb: 'vbnet', swift: 'swift', dart: 'dart',
  lua: 'lua', pl: 'perl', pm: 'perl', r: 'r', jl: 'julia', ex: 'elixir', exs: 'elixir',
  erl: 'erlang', hrl: 'erlang', hs: 'haskell', ml: 'ocaml', nim: 'nim',
  sh: 'bash', bash: 'bash', zsh: 'bash', fish: 'bash', ps1: 'powershell', psm1: 'powershell',
  bat: 'dos', cmd: 'dos',
  toml: 'ini', ini: 'ini', cfg: 'ini', conf: 'ini', config: 'ini', properties: 'ini', env: 'ini', editorconfig: 'ini',
  yaml: 'yaml', yml: 'yaml',
  sql: 'sql', graphql: 'graphql', gql: 'graphql', proto: 'protobuf', thrift: 'thrift',
  diff: 'diff', patch: 'diff', gradle: 'groovy', cmake: 'cmake', mk: 'makefile', makefile: 'makefile',
  dockerfile: 'dockerfile', nix: 'nix',
  asm: 'x86asm', s: 'x86asm', tex: 'latex', bib: 'latex',
}

/** common 包之外、常用但需要单独注册的语言。 */
const EXTRA_LANGUAGES: Array<[string, () => Promise<{ default: unknown }>]> = [
  ['powershell', () => import('highlight.js/lib/languages/powershell')],
  ['dos', () => import('highlight.js/lib/languages/dos')],
  ['dockerfile', () => import('highlight.js/lib/languages/dockerfile')],
  ['cmake', () => import('highlight.js/lib/languages/cmake')],
  ['protobuf', () => import('highlight.js/lib/languages/protobuf')],
  ['thrift', () => import('highlight.js/lib/languages/thrift')],
  ['dart', () => import('highlight.js/lib/languages/dart')],
  ['scala', () => import('highlight.js/lib/languages/scala')],
  ['elixir', () => import('highlight.js/lib/languages/elixir')],
  ['erlang', () => import('highlight.js/lib/languages/erlang')],
  ['haskell', () => import('highlight.js/lib/languages/haskell')],
  ['clojure', () => import('highlight.js/lib/languages/clojure')],
  ['julia', () => import('highlight.js/lib/languages/julia')],
  ['nim', () => import('highlight.js/lib/languages/nim')],
  ['ocaml', () => import('highlight.js/lib/languages/ocaml')],
  ['fsharp', () => import('highlight.js/lib/languages/fsharp')],
  ['x86asm', () => import('highlight.js/lib/languages/x86asm')],
  ['latex', () => import('highlight.js/lib/languages/latex')],
  ['groovy', () => import('highlight.js/lib/languages/groovy')],
  ['nix', () => import('highlight.js/lib/languages/nix')],
]

function languageOf(ext: string | null | undefined, name?: string): string | null {
  const key = (ext ?? '').toLowerCase().replace(/^\./, '')
  if (key && LANG_BY_EXT[key]) return LANG_BY_EXT[key]
  const lower = (name ?? '').toLowerCase()
  if (lower === 'dockerfile') return 'dockerfile'
  if (lower === 'makefile') return 'makefile'
  if (lower === 'cmakelists.txt') return 'cmake'
  if (lower === '.gitignore' || lower === '.gitattributes' || lower === '.editorconfig') return 'ini'
  return null
}

/** 按需加载 highlight.js（主包不带语法解析器）。 */
function loadHljs(): Promise<Hljs> {
  if (!hljsPromise) {
    hljsPromise = (async () => {
      const mod = (await import('highlight.js/lib/common')) as unknown as { default?: Hljs }
      const hljs = (mod.default ?? (mod as unknown as Hljs)) as Hljs
      await Promise.all(
        EXTRA_LANGUAGES.map(async ([name, load]) => {
          const language = (await load()) as { default?: unknown }
          hljs.registerLanguage(name, language.default ?? language)
        }),
      )
      return hljs
    })()
  }
  return hljsPromise
}

async function applyHighlight(text: string, ext: string | null | undefined, name: string) {
  codeHtml.value = ''
  const language = languageOf(ext, name)
  if (!language || !text || text.length > HIGHLIGHT_LIMIT) return
  const token = ++hlToken
  try {
    const hljs = await loadHljs()
    if (token !== hlToken) return
    codeHtml.value = hljs.highlight(text, { language, ignoreIllegals: true }).value
  } catch {
    /* 高亮失败就保持纯文本，不影响预览 */
  }
}

// ---- Word / Excel / PowerPoint ----
const docRef = ref<HTMLElement | null>(null)
const docStyleRef = ref<HTMLElement | null>(null)
const docLoading = ref(false)
const docError = ref<string | null>(null)
/** Excel 的工作表名与当前选中项（工作表对象放在非响应式变量里，避免被 Vue 深度代理）。 */
const sheetNames = ref<string[]>([])
const sheetIndex = ref(0)
const sheetTruncated = ref(false)
let docToken = 0
let docStage: HTMLElement | null = null
let workbook: ExcelWorkbook | null = null
let pptViewer: { destroy?: () => void } | null = null

// ---- 图片 ----
const canvasRef = ref<HTMLElement | null>(null)
const natural = ref({ w: 0, h: 0 })
const canvasSize = ref({ w: 0, h: 0 })
const zoom = ref<number | 'fit'>('fit')
const imageLoading = ref(false)
const imageError = ref(false)
const panning = ref(false)

const fitScale = computed(() => {
  const n = natural.value
  const c = canvasSize.value
  if (!n.w || !n.h || !c.w || !c.h) return 1
  return Math.min(1, c.w / n.w, c.h / n.h)
})
const displayScale = computed(() => (zoom.value === 'fit' ? fitScale.value : zoom.value))
const percent = computed(() => Math.max(1, Math.round(displayScale.value * 100)))
const imageStyle = computed(() => {
  const n = natural.value
  if (!n.w || !n.h) return undefined
  const scale = displayScale.value
  return {
    width: `${Math.max(1, Math.round(n.w * scale))}px`,
    height: `${Math.max(1, Math.round(n.h * scale))}px`,
  }
})

const lineCount = computed(() => {
  const text = textPreview.value?.text
  if (!text) return 0
  let count = 1
  for (let i = 0; i < text.length; i += 1) if (text.charCodeAt(i) === 10) count += 1
  return count
})

const counter = computed(() =>
  store.canNavigate ? `${store.index + 1} / ${store.items.length}` : '',
)

function resetView() {
  natural.value = { w: 0, h: 0 }
  zoom.value = 'fit'
  imageLoading.value = false
  imageError.value = false
  textPreview.value = null
  textError.value = null
  textLoading.value = false
  wrap.value = false
  copied.value = false
  mdView.value = 'preview'
  codeHtml.value = ''
  hlToken += 1
  docLoading.value = false
  docError.value = null
  sheetNames.value = []
  sheetIndex.value = 0
  sheetTruncated.value = false
  workbook = null
  docStage = null
  pptViewer?.destroy?.()
  pptViewer = null
  docToken += 1
  if (docRef.value) docRef.value.innerHTML = ''
  if (docStyleRef.value) docStyleRef.value.innerHTML = ''
}

function loadText(path: string) {
  const token = ++loadToken
  textLoading.value = true
  void api
    .readTextPreview(path, TEXT_LIMIT)
    .then((result) => {
      if (token === loadToken) {
        textPreview.value = result
        const target = item.value
        if (target && target.kind === 'text' && !result.binary) {
          void applyHighlight(result.text, target.ext, target.name)
        }
      }
    })
    .catch((error: unknown) => {
      if (token === loadToken) textError.value = String(error)
    })
    .finally(() => {
      if (token === loadToken) textLoading.value = false
    })
}

/**
 * 复制当前文本预览的内容。
 *
 * 只复制已加载的部分：超大文件被截断时页脚会提示，复制结果同样是前 1 MiB。
 * 选区内的 Ctrl+C 由浏览器原生处理（`.pv-text` 允许选中），两者互不影响。
 */
async function copyText() {
  const text = textPreview.value?.text
  if (!text) return
  try {
    await navigator.clipboard.writeText(text)
    copied.value = true
    if (copiedTimer !== undefined) window.clearTimeout(copiedTimer)
    copiedTimer = window.setTimeout(() => {
      copied.value = false
      copiedTimer = undefined
    }, 1500)
  } catch (error) {
    ui.setStatusText(`复制失败：${String(error)}`)
  }
}

// ---- Word / Excel / PowerPoint ----

/** asset 协议带 CORS 头，可以直接 fetch 成 ArrayBuffer。 */
async function readBytes(path: string): Promise<ArrayBuffer> {
  const response = await fetch(convertFileSrc(path))
  if (!response.ok) throw new Error(`无法读取文件（HTTP ${response.status}）`)
  return response.arrayBuffer()
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

/** 解析 Office 文档并渲染到 `docRef`；渲染库按需加载，不进主包。 */
async function loadDocument(target: { path: string; kind: string }) {
  const token = ++docToken
  docLoading.value = true
  docError.value = null
  try {
    const buffer = await readBytes(target.path)
    if (token !== docToken) return
    await nextTick()
    const host = docRef.value
    if (!host) return
    host.innerHTML = ''
    // 同一个容器会在不同文件之间复用，滚动位置要手动归零。
    host.scrollTop = 0
    host.scrollLeft = 0
    // 每次渲染用一个新容器：快速连按方向键时，上一次的异步渲染只会写进
    // 已经被摘掉的节点，不会污染当前文件。
    const stage = document.createElement('div')
    stage.className = 'pv-doc-content'
    host.append(stage)
    docStage = stage

    if (target.kind === 'word') await renderWord(buffer, stage)
    else if (target.kind === 'excel') await renderExcel(buffer, stage)
    else await renderPpt(buffer, stage)
    if (token !== docToken) stage.remove()
  } catch (error) {
    if (token === docToken) docError.value = errorText(error)
  } finally {
    if (token === docToken) docLoading.value = false
  }
}

async function renderWord(buffer: ArrayBuffer, host: HTMLElement) {
  const { renderAsync } = await import('docx-preview')
  await renderAsync(buffer, host, docStyleRef.value ?? host, {
    inWrapper: true,
    breakPages: true,
    ignoreLastRenderedPageBreak: true,
    experimental: true,
    useBase64URL: true,
  })
}

async function renderExcel(buffer: ArrayBuffer, host: HTMLElement) {
  const mod = (await import('exceljs')) as unknown as { default?: typeof import('exceljs') }
  const ExcelJS = (mod.default ?? mod) as typeof import('exceljs')
  const book = new ExcelJS.Workbook()
  await book.xlsx.load(buffer)
  workbook = book
  sheetNames.value = book.worksheets.map((sheet) => sheet.name)
  sheetIndex.value = 0
  renderSheet(host)
}

async function renderPpt(buffer: ArrayBuffer, host: HTMLElement) {
  const { init } = await import('pptx-preview')
  // 幻灯片比例写在文件里（4:3 / 16:9 都可能），先按可用空间渲染一次，
  // 再用真实尺寸算出缩放后的宽度重渲染，避免页面被容器裁掉。
  const maxWidth = Math.max(480, host.clientWidth - 32)
  const maxHeight = Math.max(320, host.clientHeight - 32)
  const width = Math.round(Math.min(1200, maxWidth, maxHeight))
  const viewer = init(host, { width, height: Math.round((width * 9) / 16), mode: 'list' })
  pptViewer = viewer
  await viewer.preview(buffer)

  const first = host.querySelector('.pptx-preview-slide-wrapper') as HTMLElement | null
  const renderedWidth = first?.offsetWidth || 0
  const renderedHeight = first?.offsetHeight || 0
  if (!renderedWidth || !renderedHeight) return
  // 预留 12px 给幻灯片之间的 10px 间距，避免容器里多出一条内滚动条。
  const usableHeight = maxHeight - 12
  const scale = Math.min(maxWidth / renderedWidth, usableHeight / renderedHeight, 1.6)
  if (Math.abs(scale - 1) < 0.02) return

  const fittedWidth = Math.round(renderedWidth * scale)
  const fittedHeight = Math.round(renderedHeight * scale) + 12
  viewer.destroy?.()
  host.innerHTML = ''
  const fitted = init(host, { width: fittedWidth, height: fittedHeight, mode: 'list' })
  pptViewer = fitted
  await fitted.preview(buffer)
}

function selectSheet(index: number) {
  sheetIndex.value = index
  renderSheet()
}

/** 把 `A1:B2` 解析成 1-based 的行列范围。 */
function parseRange(range: string): { top: number; left: number; bottom: number; right: number } | null {
  const match = /^([A-Z]+)(\d+):([A-Z]+)(\d+)$/i.exec(range)
  if (!match) return null
  const col = (letters: string) => {
    let value = 0
    for (const char of letters.toUpperCase()) value = value * 26 + (char.charCodeAt(0) - 64)
    return value
  }
  return {
    top: Number(match[2]),
    left: col(match[1]),
    bottom: Number(match[4]),
    right: col(match[3]),
  }
}

function argbColor(color: Partial<ExcelColor> | undefined): string | null {
  if (!color || typeof color !== 'object' || typeof color.argb !== 'string') return null
  const hex = color.argb.slice(-6)
  return /^[0-9a-f]{6}$/i.test(hex) ? `#${hex}` : null
}

function primitiveText(value: unknown): string {
  if (value === null || value === undefined) return ''
  if (typeof value === 'boolean') return value ? 'TRUE' : 'FALSE'
  if (typeof value === 'number' || typeof value === 'string') return String(value)
  if (value instanceof Date) return value.toLocaleString()
  return ''
}

/** 取出单元格的显示文本（富文本 / 超链接 / 公式结果都尽量还原）。 */
function cellText(cell: ExcelCell): string {
  const value = cell.value
  if (value === null || value === undefined) return ''
  if (typeof value === 'string' || typeof value === 'number' || typeof value === 'boolean') {
    return primitiveText(value)
  }
  if (value instanceof Date) return value.toLocaleString()
  if (typeof value !== 'object') return String(value)
  const record = value as unknown as Record<string, unknown>
  if (Array.isArray(record.richText)) {
    return (record.richText as { text?: string }[]).map((run) => run.text ?? '').join('')
  }
  if (typeof record.text === 'string') return record.text
  if ('result' in record) return primitiveText(record.result)
  if ('error' in record) return String(record.error)
  // 公式没有缓存值时（例如非 Excel 生成的文件）退而显示公式本身，比空白更有信息量。
  if (typeof record.formula === 'string') return `=${record.formula}`
  return ''
}

function applyCellStyle(td: HTMLTableCellElement, cell: ExcelCell) {
  const font = cell.font
  if (font) {
    if (font.bold) td.style.fontWeight = '600'
    if (font.italic) td.style.fontStyle = 'italic'
    if (font.underline) td.style.textDecoration = 'underline'
    if (font.size) td.style.fontSize = `${Math.min(20, Math.max(8, font.size))}px`
    const color = argbColor(font.color)
    if (color) td.style.color = color
  }
  const fill = cell.fill
  if (fill && fill.type === 'pattern' && fill.pattern !== 'none') {
    const background = argbColor(fill.fgColor)
    if (background) td.style.background = background
  }
  const alignment = cell.alignment
  if (alignment) {
    if (alignment.horizontal) {
      td.style.textAlign = alignment.horizontal === 'centerContinuous' ? 'center' : alignment.horizontal
    }
    if (alignment.vertical) td.style.verticalAlign = alignment.vertical
    if (alignment.wrapText) td.style.whiteSpace = 'pre-wrap'
  }
}

/** 渲染当前工作表（只铺前若干行列，避免超大表格撑爆 DOM）。 */
function renderSheet(host?: HTMLElement) {
  const el = host ?? docStage ?? docRef.value
  const book = workbook
  if (!el || !book) return
  const sheet = book.getWorksheet(sheetNames.value[sheetIndex.value] ?? '')
  if (!sheet) return

  const rowCount = Math.min(sheet.rowCount, SHEET_MAX_ROWS)
  const colCount = Math.min(sheet.columnCount, SHEET_MAX_COLS)
  sheetTruncated.value = sheet.rowCount > rowCount || sheet.columnCount > colCount

  el.innerHTML = ''
  const table = document.createElement('table')
  table.className = 'sheet-table'

  const colgroup = document.createElement('colgroup')
  for (let c = 1; c <= colCount; c += 1) {
    const col = document.createElement('col')
    const width = sheet.getColumn(c).width ?? 9
    col.style.width = `${Math.round(width * 7 + 6)}px`
    colgroup.append(col)
  }
  table.append(colgroup)

  // 合并单元格：左上角记跨度，其余格子跳过。
  const spans = new Map<string, { rowspan: number; colspan: number }>()
  const covered = new Set<string>()
  for (const range of sheet.model.merges ?? []) {
    const parsed = parseRange(range)
    if (!parsed) continue
    const { top, left, bottom, right } = parsed
    if (top > rowCount || left > colCount) continue
    spans.set(`${top}:${left}`, {
      rowspan: Math.min(bottom, rowCount) - top + 1,
      colspan: Math.min(right, colCount) - left + 1,
    })
    for (let r = top; r <= Math.min(bottom, rowCount); r += 1) {
      for (let c = left; c <= Math.min(right, colCount); c += 1) {
        if (r !== top || c !== left) covered.add(`${r}:${c}`)
      }
    }
  }

  const tbody = document.createElement('tbody')
  for (let r = 1; r <= rowCount; r += 1) {
    const tr = document.createElement('tr')
    const height = sheet.getRow(r).height
    if (height) tr.style.height = `${Math.round((height * 96) / 72)}px`
    for (let c = 1; c <= colCount; c += 1) {
      const key = `${r}:${c}`
      if (covered.has(key)) continue
      const cell = sheet.getCell(r, c)
      const td = document.createElement('td')
      const span = spans.get(key)
      if (span) {
        if (span.rowspan > 1) td.rowSpan = span.rowspan
        if (span.colspan > 1) td.colSpan = span.colspan
      }
      td.textContent = cellText(cell)
      applyCellStyle(td, cell)
      tr.append(td)
    }
    tbody.append(tr)
  }
  table.append(tbody)
  el.append(table)
}

watch(
  () => item.value?.path,
  () => {
    resetView()
    const target = item.value
    if (!target) return
    if (textual.value) loadText(target.path)
    if (target.kind === 'image') imageLoading.value = true
    if (isOffice.value) void loadDocument(target)
  },
  { immediate: true },
)

// 打开时（从关闭 → 打开）重新拍一次画布尺寸：关闭期间元素不在 DOM 里。
watch(
  () => store.open,
  (open) => {
    if (!open) {
      loadToken += 1
      return
    }
    void nextTick(() => {
      measureCanvas()
      if (kind.value === 'image') imageLoading.value = true
    })
  },
)

// ---- 图片尺寸与缩放 ----

let observer: ResizeObserver | null = null

function measureCanvas() {
  const el = canvasRef.value
  if (!el) return
  canvasSize.value = { w: el.clientWidth, h: el.clientHeight }
}

watch(canvasRef, (el) => {
  observer?.disconnect()
  observer = null
  if (!el) return
  measureCanvas()
  observer = new ResizeObserver(() => measureCanvas())
  observer.observe(el)
})

function onImageLoad(event: Event) {
  const image = event.target as HTMLImageElement
  natural.value = {
    w: image.naturalWidth || image.width || 0,
    h: image.naturalHeight || image.height || 0,
  }
  imageLoading.value = false
  void nextTick(measureCanvas)
}

function onImageError() {
  imageLoading.value = false
  imageError.value = true
}

function zoomIn() {
  zoom.value = Math.min(16, displayScale.value * 1.25)
}

function zoomOut() {
  zoom.value = Math.max(0.05, displayScale.value / 1.25)
}

function fitToWindow() {
  zoom.value = 'fit'
}

function actualSize() {
  zoom.value = 1
}

function onWheel(event: WheelEvent) {
  if (kind.value !== 'image') return
  event.preventDefault()
  if (event.deltaY < 0) zoomIn()
  else zoomOut()
}

let panOrigin: { x: number; y: number; left: number; top: number } | null = null

function onCanvasDown(event: MouseEvent) {
  const el = canvasRef.value
  if (!el || event.button !== 0) return
  if (el.scrollWidth <= el.clientWidth && el.scrollHeight <= el.clientHeight) return
  panOrigin = { x: event.clientX, y: event.clientY, left: el.scrollLeft, top: el.scrollTop }
  panning.value = true
  window.addEventListener('mousemove', onPanMove)
  window.addEventListener('mouseup', onPanEnd)
}

function onPanMove(event: MouseEvent) {
  const el = canvasRef.value
  if (!el || !panOrigin) return
  el.scrollLeft = panOrigin.left - (event.clientX - panOrigin.x)
  el.scrollTop = panOrigin.top - (event.clientY - panOrigin.y)
}

function onPanEnd() {
  panOrigin = null
  panning.value = false
  window.removeEventListener('mousemove', onPanMove)
  window.removeEventListener('mouseup', onPanEnd)
}

async function openExternal() {
  const target = item.value
  if (!target) return
  try {
    await openPath(target.path)
  } catch (error) {
    console.error('无法用默认应用打开', error)
  }
}

// ---- 键盘 ----

function onKeydown(event: KeyboardEvent) {
  if (!store.open) return
  if (event.isComposing) return
  // 事件已被更早的处理器消费（例如列表刚用空格打开预览）：不要把它当成「关闭」。
  if (event.defaultPrevented) return
  const key = event.key
  if (key === 'Escape' || key === ' ') {
    event.preventDefault()
    event.stopPropagation()
    store.close()
    return
  }
  if (key === 'ArrowLeft') {
    event.preventDefault()
    event.stopPropagation()
    store.prev()
    return
  }
  if (key === 'ArrowRight') {
    event.preventDefault()
    event.stopPropagation()
    store.next()
    return
  }
  if (kind.value !== 'image') return
  if (key === '+' || key === '=') {
    event.preventDefault()
    zoomIn()
  } else if (key === '-') {
    event.preventDefault()
    zoomOut()
  } else if (key === '0') {
    event.preventDefault()
    fitToWindow()
  } else if (key === '1') {
    event.preventDefault()
    actualSize()
  }
}

onMounted(() => window.addEventListener('keydown', onKeydown, true))
onBeforeUnmount(() => {
  window.removeEventListener('keydown', onKeydown, true)
  observer?.disconnect()
  observer = null
  onPanEnd()
  if (copiedTimer !== undefined) window.clearTimeout(copiedTimer)
})
</script>

<template>
  <div v-if="store.open && item" class="mask" @mousedown.self="store.close()">
    <section class="preview-card" role="dialog" aria-modal="true" :aria-label="`预览 ${item.name}`">
      <header class="pv-head">
        <div class="pv-title">
          <Icon :name="kindIcon" :size="15" class="pv-kind-icon" />
          <span class="pv-name" :title="item.path">{{ item.name }}</span>
        </div>

        <div class="pv-meta">
          <span>{{ formatSize(item.size) }}</span>
          <span v-if="item.modified">{{ formatTime(item.modified) }}</span>
          <span v-if="counter" class="pv-counter">{{ counter }}</span>
        </div>

        <div class="pv-actions">
          <template v-if="kind === 'image' && !imageError">
            <button type="button" title="缩小（-）" @click="zoomOut"><Icon name="minus" :size="15" /></button>
            <span class="pv-percent">{{ percent }}%</span>
            <button type="button" title="放大（+）" @click="zoomIn"><Icon name="plus" :size="15" /></button>
            <button type="button" class="pv-text-btn" title="适应窗口（0）" @click="fitToWindow">适应</button>
            <button type="button" class="pv-text-btn" title="原始尺寸（1）" @click="actualSize">1:1</button>
          </template>
          <span v-else-if="textual && textPreview" class="pv-text-meta">
            {{ lineCount }} 行 · {{ textPreview.encoding.toUpperCase() }}
          </span>
          <button type="button" class="pv-text-btn" title="用默认应用打开" @click="openExternal">
            <Icon name="external-link" :size="13" /><span>打开</span>
          </button>
          <button type="button" class="pv-close" title="关闭（Esc）" aria-label="关闭预览" @click="store.close()">
            <Icon name="x" :size="15" />
          </button>
        </div>
      </header>

      <div class="pv-body" :class="{ 'has-nav': store.canNavigate }">
        <button
          v-if="store.canNavigate"
          type="button"
          class="pv-nav prev"
          title="上一个（←）"
          aria-label="上一个"
          @click.stop="store.prev()"
        >
          <Icon name="chevron-left" :size="22" />
        </button>

        <div class="pv-stage">
          <!-- 远程（FTP / SFTP）：路径不在本地，无法读取 -->
          <div v-if="store.isRemote" class="pv-empty">
            <Icon name="network" :size="30" />
            <p class="pv-empty-title">远程文件暂不支持预览</p>
            <p class="pv-empty-desc">请下载到本地后再预览，或用外部程序打开</p>
          </div>

          <!-- 图片 -->
          <template v-else-if="kind === 'image'">
            <div v-if="item.size > MAX_IMAGE_BYTES" class="pv-empty">
              <Icon name="image" :size="30" />
              <p class="pv-empty-title">图片过大，暂不预览</p>
              <p class="pv-empty-desc">文件大小 {{ formatSize(item.size) }}，超过 {{ formatSize(MAX_IMAGE_BYTES) }} 上限</p>
              <button type="button" class="pv-primary" @click="openExternal">用默认应用打开</button>
            </div>
            <div v-else-if="imageError" class="pv-empty">
              <Icon name="alert-triangle" :size="28" />
              <p class="pv-empty-title">无法加载这张图片</p>
              <p class="pv-empty-desc">格式可能不受支持，或文件已被移动</p>
              <button type="button" class="pv-primary" @click="openExternal">用默认应用打开</button>
            </div>
            <div
              v-else
              ref="canvasRef"
              class="pv-canvas"
              :class="{ panning }"
              @wheel="onWheel"
              @mousedown="onCanvasDown"
            >
              <img
                :src="assetUrl"
                :style="imageStyle"
                alt=""
                draggable="false"
                @load="onImageLoad"
                @error="onImageError"
              />
              <span v-if="imageLoading" class="pv-loading">正在加载…</span>
            </div>
          </template>

          <!-- 文本 / Markdown -->
          <template v-else-if="textual">
            <div v-if="textLoading" class="pv-empty">
              <span class="pv-loading">正在读取…</span>
            </div>
            <div v-else-if="textError" class="pv-empty">
              <Icon name="alert-triangle" :size="28" />
              <p class="pv-empty-title">无法读取文件内容</p>
              <p class="pv-empty-desc">{{ textError }}</p>
            </div>
            <div v-else-if="textPreview?.binary" class="pv-empty">
              <Icon name="file" :size="30" />
              <p class="pv-empty-title">这不是文本文件</p>
              <p class="pv-empty-desc">内容包含二进制数据，无法以文本方式预览</p>
              <button type="button" class="pv-primary" @click="openExternal">用默认应用打开</button>
            </div>
            <div v-else class="pv-text-wrap">
              <div v-if="kind === 'markdown' && mdView === 'preview'" class="pv-md" data-native-context-menu v-html="mdHtml" />
              <pre v-else class="pv-text" :class="{ wrap }" data-native-context-menu><code v-if="codeHtml" class="hljs" v-html="codeHtml"></code><template v-else>{{ textPreview?.text ?? '' }}</template></pre>
              <div v-if="textPreview?.truncated" class="pv-truncated">
                文件较大，仅显示前 {{ formatSize(TEXT_LIMIT) }}（共 {{ formatSize(textPreview.size) }}），复制内容也只含已加载部分
              </div>
            </div>
          </template>

          <!-- Word / Excel / PowerPoint -->
          <template v-else-if="isOffice">
            <div v-if="item.size > MAX_OFFICE_BYTES" class="pv-empty">
              <Icon name="file-text" :size="30" />
              <p class="pv-empty-title">文档过大，暂不预览</p>
              <p class="pv-empty-desc">文件大小 {{ formatSize(item.size) }}，超过 {{ formatSize(MAX_OFFICE_BYTES) }} 上限</p>
              <button type="button" class="pv-primary" @click="openExternal">用默认应用打开</button>
            </div>
            <div v-else class="pv-doc-wrap">
              <div ref="docStyleRef" class="pv-doc-style" />
              <div ref="docRef" class="pv-doc" :class="`pv-doc-${kind}`" />
              <div v-if="kind === 'excel' && sheetNames.length > 1" class="pv-sheets">
                <button
                  v-for="(name, index) in sheetNames"
                  :key="name + index"
                  type="button"
                  class="pv-sheet-tab"
                  :class="{ active: index === sheetIndex }"
                  @click="selectSheet(index)"
                >
                  {{ name }}
                </button>
              </div>
              <div v-if="sheetTruncated && kind === 'excel'" class="pv-truncated">
                表格较大，仅显示前 {{ SHEET_MAX_ROWS }} 行 × {{ SHEET_MAX_COLS }} 列
              </div>
              <div v-if="docLoading" class="pv-doc-status">
                <span class="pv-loading">正在解析文档…</span>
              </div>
              <div v-else-if="docError" class="pv-doc-status">
                <Icon name="alert-triangle" :size="26" />
                <p class="pv-empty-title">无法解析这个文档</p>
                <p class="pv-empty-desc">{{ docError }}</p>
                <button type="button" class="pv-primary" @click="openExternal">用默认应用打开</button>
              </div>
            </div>
          </template>

          <!-- PDF -->
          <template v-else-if="kind === 'pdf'">
            <div v-if="item.size > MAX_PDF_BYTES" class="pv-empty">
              <Icon name="file-text" :size="30" />
              <p class="pv-empty-title">PDF 过大，暂不预览</p>
              <p class="pv-empty-desc">文件大小 {{ formatSize(item.size) }}</p>
              <button type="button" class="pv-primary" @click="openExternal">用默认应用打开</button>
            </div>
            <iframe v-else class="pv-pdf" :src="assetUrl" :title="item.name" />
          </template>

          <!-- 其他格式 -->
          <div v-else class="pv-empty">
            <Icon name="file" :size="30" />
            <p class="pv-empty-title">该格式暂不支持预览</p>
            <p class="pv-empty-desc">扩展名：{{ item.ext || '无' }}</p>
            <button type="button" class="pv-primary" @click="openExternal">用默认应用打开</button>
          </div>
        </div>

        <button
          v-if="store.canNavigate"
          type="button"
          class="pv-nav next"
          title="下一个（→）"
          aria-label="下一个"
          @click.stop="store.next()"
        >
          <Icon name="chevron-right" :size="22" />
        </button>
      </div>

      <footer class="pv-foot">
        <template v-if="textual && textPreview && !textPreview.binary">
          <button
            v-if="kind === 'markdown'"
            type="button"
            class="pv-foot-btn"
            :title="mdView === 'preview' ? '查看 Markdown 源码' : '查看渲染效果'"
            @click="mdView = mdView === 'preview' ? 'source' : 'preview'"
          >
            <Icon :name="mdView === 'preview' ? 'code' : 'eye'" :size="13" />{{ mdView === 'preview' ? '源码' : '预览' }}
          </button>
          <button v-if="kind === 'text' || mdView === 'source'" type="button" class="pv-foot-btn" @click="wrap = !wrap">
            <Icon name="view-list" :size="13" />{{ wrap ? '取消换行' : '自动换行' }}
          </button>
          <button type="button" class="pv-foot-btn" :title="copied ? '已复制' : '复制全部内容（Ctrl+C 可复制选中部分）'" @click="copyText">
            <Icon :name="copied ? 'check' : 'copy'" :size="13" />{{ copied ? '已复制' : '复制内容' }}
          </button>
        </template>
        <span class="pv-path" :title="item.path">{{ item.path }}</span>
        <span class="pv-hints">
          <template v-if="store.canNavigate">← → 切换 · </template>
          Esc / 空格 关闭
        </span>
      </footer>
    </section>
  </div>
</template>

<style scoped>
.mask {
  position: fixed;
  inset: 0;
  z-index: 340;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 32px;
  background: color-mix(in srgb, var(--mask) 82%, transparent);
  backdrop-filter: blur(2px);
}

.preview-card {
  display: flex;
  flex-direction: column;
  width: min(1180px, 100%);
  height: min(820px, 100%);
  min-height: 0;
  border: 1px solid var(--border-strong);
  border-radius: var(--radius-lg);
  background: var(--bg-elevated);
  box-shadow: var(--shadow-lg);
  overflow: hidden;
}

.pv-head {
  display: flex;
  align-items: center;
  gap: 12px;
  flex: none;
  height: 42px;
  padding: 0 10px 0 14px;
  border-bottom: 1px solid var(--border);
  background: var(--bg-panel);
}

.pv-title {
  display: flex;
  align-items: center;
  gap: 7px;
  min-width: 0;
  flex: 0 1 auto;
}

.pv-kind-icon {
  flex: none;
  color: var(--fg-dim);
}

.pv-name {
  min-width: 0;
  overflow: hidden;
  font-size: 0.9167rem;
  font-weight: 600;
  color: var(--fg);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.pv-meta {
  display: flex;
  align-items: center;
  gap: 10px;
  flex: none;
  color: var(--fg-dim);
  font-size: 0.8333rem;
  font-variant-numeric: tabular-nums;
}

.pv-counter {
  padding: 1px 7px;
  border-radius: 999px;
  background: var(--bg-hover);
  color: var(--fg-muted);
}

.pv-actions {
  display: flex;
  align-items: center;
  gap: 2px;
  margin-left: auto;
  flex: none;
}

.pv-actions button {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  height: 26px;
  padding: 0 7px;
  border-radius: var(--radius);
  color: var(--fg-muted);
  font-size: 0.8333rem;
}

.pv-actions button:hover {
  background: var(--bg-hover);
  color: var(--fg);
}

.pv-percent {
  min-width: 44px;
  color: var(--fg-muted);
  font-size: 0.7917rem;
  font-variant-numeric: tabular-nums;
  text-align: center;
}

.pv-text-meta {
  margin-right: 4px;
  color: var(--fg-dim);
  font-size: 0.7917rem;
  font-variant-numeric: tabular-nums;
}

.pv-close {
  margin-left: 4px;
}

.pv-body {
  position: relative;
  display: flex;
  flex: 1;
  min-height: 0;
  background: var(--bg-app);
}

/* 有上一张 / 下一张按钮时，两侧留出让位空间，别压住文字或表格。 */
.pv-body.has-nav {
  padding-left: 44px;
  padding-right: 44px;
}

.pv-stage {
  position: relative;
  flex: 1;
  min-width: 0;
  min-height: 0;
  display: flex;
}

.pv-nav {
  position: absolute;
  top: 50%;
  z-index: 3;
  display: flex;
  align-items: center;
  justify-content: center;
  width: 34px;
  height: 54px;
  border-radius: var(--radius);
  background: color-mix(in srgb, var(--bg-elevated) 78%, transparent);
  border: 1px solid var(--border);
  color: var(--fg-muted);
  transform: translateY(-50%);
  transition: background var(--t-fast), color var(--t-fast), opacity var(--t-fast);
}

.pv-nav:hover {
  background: var(--bg-hover);
  color: var(--fg);
}

.pv-nav.prev {
  left: 10px;
}

.pv-nav.next {
  right: 10px;
}

/* ---- 图片 ---- */
.pv-canvas {
  flex: 1;
  min-width: 0;
  min-height: 0;
  display: grid;
  overflow: auto;
  padding: 12px;
}

.pv-canvas.panning {
  cursor: grabbing;
}

.pv-canvas img {
  display: block;
  margin: auto;
  max-width: none;
  user-select: none;
  box-shadow: var(--shadow-md);
  background:
    conic-gradient(from 90deg, color-mix(in srgb, var(--fg) 6%, transparent) 25%, transparent 0 50%,
      color-mix(in srgb, var(--fg) 6%, transparent) 0 75%, transparent 0) 0 0 / 16px 16px;
}

.pv-loading {
  position: absolute;
  inset: 0;
  display: grid;
  place-items: center;
  color: var(--fg-dim);
  font-size: 0.9167rem;
  pointer-events: none;
}

/* ---- 文本 ---- */
.pv-text-wrap {
  display: flex;
  flex-direction: column;
  flex: 1;
  min-width: 0;
  min-height: 0;
}

.pv-text {
  flex: 1;
  min-height: 0;
  margin: 0;
  padding: 14px 16px;
  overflow: auto;
  color: var(--fg);
  font-family: var(--font-mono, 'Cascadia Mono', 'Consolas', monospace);
  font-size: 0.8333rem;
  line-height: 1.55;
  tab-size: 4;
  white-space: pre;
  /* 全局 body 禁用了选中；预览文本要能拖选后用 Ctrl+C 复制。 */
  user-select: text;
  cursor: text;
}

.pv-text.wrap {
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}

/* 语法高亮：颜色取自 tokens.css 的 --code-*（v-html 内容需要 :deep） */
.pv-text :deep(.hljs-comment),
.pv-text :deep(.hljs-quote) {
  color: var(--code-comment);
  font-style: italic;
}

.pv-text :deep(.hljs-keyword),
.pv-text :deep(.hljs-selector-tag),
.pv-text :deep(.hljs-literal),
.pv-text :deep(.hljs-doctag),
.pv-text :deep(.hljs-name),
.pv-text :deep(.hljs-section),
.pv-text :deep(.hljs-tag),
.pv-text :deep(.hljs-selector-id),
.pv-text :deep(.hljs-selector-class) {
  color: var(--code-keyword);
}

.pv-text :deep(.hljs-string),
.pv-text :deep(.hljs-regexp),
.pv-text :deep(.hljs-addition),
.pv-text :deep(.hljs-meta .hljs-string) {
  color: var(--code-string);
}

.pv-text :deep(.hljs-number),
.pv-text :deep(.hljs-symbol),
.pv-text :deep(.hljs-bullet),
.pv-text :deep(.hljs-link),
.pv-text :deep(.hljs-selector-attr),
.pv-text :deep(.hljs-selector-pseudo),
.pv-text :deep(.hljs-attr),
.pv-text :deep(.hljs-attribute) {
  color: var(--code-number);
}

.pv-text :deep(.hljs-title),
.pv-text :deep(.hljs-function .hljs-title) {
  color: var(--code-function);
}

.pv-text :deep(.hljs-type),
.pv-text :deep(.hljs-built_in),
.pv-text :deep(.hljs-class .hljs-title),
.pv-text :deep(.hljs-title.class_) {
  color: var(--code-type);
}

.pv-text :deep(.hljs-meta),
.pv-text :deep(.hljs-property) {
  color: var(--code-meta);
}

.pv-text :deep(.hljs-variable),
.pv-text :deep(.hljs-template-variable) {
  color: var(--code-removed);
}

.pv-text :deep(.hljs-deletion) {
  color: var(--code-removed);
}

.pv-text :deep(.hljs-emphasis) {
  font-style: italic;
}

.pv-text :deep(.hljs-strong) {
  font-weight: 600;
}

.pv-truncated {
  flex: none;
  padding: 7px 16px;
  border-top: 1px solid var(--border);
  background: var(--bg-panel);
  color: var(--warn);
  font-size: 0.7917rem;
}

/* ---- Markdown ---- */
.pv-md {
  flex: 1;
  min-height: 0;
  padding: 20px 24px 28px;
  overflow: auto;
  color: var(--fg);
  font-size: 0.9167rem;
  line-height: 1.7;
  user-select: text;
}

.pv-md :deep(> :first-child) {
  margin-top: 0;
}

.pv-md :deep(> :last-child) {
  margin-bottom: 0;
}

.pv-md :deep(h1) {
  margin: 1.2em 0 0.6em;
  font-size: 1.5em;
  line-height: 1.35;
}

.pv-md :deep(h2) {
  margin: 1.3em 0 0.6em;
  padding-bottom: 0.3em;
  border-bottom: 1px solid var(--border);
  font-size: 1.25em;
  line-height: 1.4;
}

.pv-md :deep(h3) {
  margin: 1.1em 0 0.5em;
  font-size: 1.1em;
}

.pv-md :deep(h4) {
  margin: 1em 0 0.4em;
  font-size: 1em;
}

.pv-md :deep(p) {
  margin: 0.7em 0;
}

.pv-md :deep(ul),
.pv-md :deep(ol) {
  margin: 0.7em 0;
  padding-left: 1.6em;
}

.pv-md :deep(li) {
  margin: 0.25em 0;
}

.pv-md :deep(code) {
  padding: 1px 5px;
  border-radius: 4px;
  background: var(--bg-hover);
  font-family: var(--font-mono, 'Cascadia Mono', 'Consolas', monospace);
  font-size: 0.9em;
}

.pv-md :deep(pre) {
  margin: 12px 0;
  padding: 12px 14px;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--bg-hover);
  overflow-x: auto;
}

.pv-md :deep(pre code) {
  padding: 0;
  background: transparent;
  font-size: 0.875em;
  line-height: 1.6;
}

.pv-md :deep(blockquote) {
  margin: 12px 0;
  padding: 2px 0 2px 12px;
  border-left: 3px solid var(--border-strong);
  color: var(--fg-muted);
}

.pv-md :deep(table) {
  display: block;
  max-width: 100%;
  margin: 12px 0;
  overflow-x: auto;
  border-collapse: collapse;
  font-size: 0.9167em;
}

.pv-md :deep(th),
.pv-md :deep(td) {
  padding: 6px 10px;
  border: 1px solid var(--border-strong);
}

.pv-md :deep(th) {
  background: var(--bg-panel);
  font-weight: 600;
  text-align: left;
}

.pv-md :deep(a) {
  color: var(--accent);
  text-decoration: underline;
  text-underline-offset: 3px;
}

.pv-md :deep(hr) {
  margin: 18px 0;
  border: 0;
  border-top: 1px solid var(--border);
}

/* ---- Word / Excel / PowerPoint ---- */
.pv-doc-wrap {
  position: relative;
  display: flex;
  flex: 1;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
}

/* docx-preview 会把样式表塞进这个容器；它本身不参与布局。 */
.pv-doc-style {
  display: none;
}

.pv-doc {
  flex: 1;
  min-height: 0;
  overflow: auto;
  background: var(--bg-app);
  user-select: text;
}

.pv-doc-content {
  min-height: 100%;
}

/* 文档页面固定是白底，深色主题下给页面加投影，避免糊成一片。 */
.pv-doc-word :deep(.docx-wrapper) {
  padding: 20px 0;
  background: transparent;
}

.pv-doc-word :deep(.docx-wrapper > section.docx) {
  margin: 0 auto 18px;
  box-shadow: 0 2px 14px rgba(0, 0, 0, 0.28);
}

.pv-doc-excel {
  padding: 14px;
  background: color-mix(in srgb, var(--bg-app) 88%, #000);
}

.pv-doc-powerpoint {
  padding: 16px;
  background: color-mix(in srgb, var(--bg-app) 78%, #000);
}

/* 表格由 JS 创建，拿不到 scoped 属性，必须走 :deep */
.pv-doc :deep(.sheet-table) {
  border-collapse: collapse;
  table-layout: fixed;
  background: #ffffff;
  color: #1f2328;
  font-family: var(--font);
  font-size: 12px;
  box-shadow: 0 2px 12px rgba(0, 0, 0, 0.22);
}

.pv-doc :deep(.sheet-table td) {
  padding: 3px 7px;
  border: 1px solid #d7dbe0;
  overflow: hidden;
  white-space: pre;
  text-overflow: ellipsis;
}

.pv-sheets {
  display: flex;
  flex: none;
  gap: 2px;
  padding: 6px 10px;
  border-top: 1px solid var(--border);
  background: var(--bg-panel);
  overflow-x: auto;
}

.pv-sheet-tab {
  flex: none;
  height: 24px;
  padding: 0 10px;
  border-radius: var(--radius);
  color: var(--fg-muted);
  font-size: 0.8333rem;
  white-space: nowrap;
}

.pv-sheet-tab:hover {
  background: var(--bg-hover);
  color: var(--fg);
}

.pv-sheet-tab.active {
  background: var(--bg-active);
  color: var(--fg);
}

.pv-doc-status {
  position: absolute;
  inset: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  padding: 24px;
  background: color-mix(in srgb, var(--bg-elevated) 88%, transparent);
  text-align: center;
  backdrop-filter: blur(2px);
}

/* ---- PDF ---- */
.pv-pdf {
  flex: 1;
  width: 100%;
  height: 100%;
  border: 0;
  background: var(--bg-app);
}

/* ---- 占位 ---- */
.pv-empty {
  display: flex;
  flex: 1;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  padding: 24px;
  color: var(--fg-dim);
  text-align: center;
}

.pv-empty-title {
  margin: 4px 0 0;
  color: var(--fg);
  font-size: 0.9583rem;
  font-weight: 600;
}

.pv-empty-desc {
  max-width: 460px;
  margin: 0;
  color: var(--fg-dim);
  font-size: 0.8333rem;
  word-break: break-all;
}

.pv-primary {
  margin-top: 8px;
  height: 30px;
  padding: 0 14px;
  border-radius: var(--radius);
  background: var(--accent);
  color: #fff;
  font-size: 0.875rem;
}

.pv-primary:hover {
  background: var(--accent-hover, var(--accent));
  filter: brightness(1.08);
}

/* ---- 底部 ---- */
.pv-foot {
  display: flex;
  align-items: center;
  gap: 10px;
  flex: none;
  height: 30px;
  padding: 0 12px;
  border-top: 1px solid var(--border);
  background: var(--bg-panel);
  color: var(--fg-dim);
  font-size: 0.7917rem;
}

.pv-foot-btn {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  height: 22px;
  padding: 0 6px;
  border-radius: var(--radius-sm, var(--radius));
  color: var(--fg-muted);
  font-size: 0.7917rem;
}

.pv-foot-btn:hover {
  background: var(--bg-hover);
  color: var(--fg);
}

.pv-path {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  direction: rtl;
  text-align: left;
}

.pv-hints {
  flex: none;
  color: var(--fg-dim);
}
</style>

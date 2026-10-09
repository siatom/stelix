// 前后端共享类型（与 src-tauri/src/types.rs 一一对应）
// ⚠️ Phase 0 冻结：各 workstream 可扩展但不得改名字段。

export interface FileEntry {
  id: number
  name: string
  path: string
  isDir: boolean
  size: number
  modified: number
  ext: string | null
  /** Windows 隐藏属性（FILE_ATTRIBUTE_HIDDEN） */
  hidden: boolean
  /** Windows 系统属性（FILE_ATTRIBUTE_SYSTEM） */
  system: boolean
}

export interface DirListing {
  path: string
  entries: FileEntry[]
}

export interface DriveInfo {
  name: string
  mount: string
  total: number
  free: number
}

export interface SearchQuery {
  text: string
  scope?: string | null
  isDir?: boolean | null
  ext?: string[]
  minSize?: number | null
  maxSize?: number | null
  modifiedAfter?: number | null
  limit?: number
}

export interface SearchHit {
  entry: FileEntry
  score: number
}

export interface IndexStatus {
  roots: string[]
  total: number
  scanning: boolean
  scanned: number
}

export interface IndexProgress {
  scanned: number
  total: number
  current: string
  done: boolean
}

export type FsOpKind = 'copy' | 'move' | 'delete' | 'rename' | 'mkdir'

export interface FsOp {
  op: FsOpKind
  sources?: string[]
  target?: string | null
}

export interface FsOpResult {
  ok: boolean
  errors: string[]
}

export interface AiMessage {
  role: 'system' | 'user' | 'assistant'
  content: string
}

export interface AiToolCall {
  id: string
  name: string
  args: unknown
}

export interface AiContext {
  activePath?: string | null
  selected?: string[]
}

export interface AiReply {
  text: string
  toolCalls: AiToolCall[]
  pendingConfirm?: FsOp | null
}

export type ToolOutcome =
  | { kind: 'search'; payload: SearchHit[] }
  | { kind: 'listing'; payload: DirListing }
  | { kind: 'fs'; payload: FsOpResult }
  | { kind: 'text'; payload: string }

export interface AiConfig {
  baseUrl: string
  apiKey: string
  model: string
  enabled: boolean
}

// ---- 视图模型（前端专用） ----

export interface TabState {
  id: string
  path: string
  entries: FileEntry[]
  loading: boolean
  error: string | null
  /** 当前目录内的本地过滤词 */
  filter: string
  sortKey: 'name' | 'size' | 'modified' | 'ext'
  sortAsc: boolean
  selected: string[]
}

export type ViewMode = 'details' | 'list' | 'icons'

export interface PaneState {
  id: string
  tabs: TabState[]
  activeTabId: string
  viewMode: ViewMode
}

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

export type FsOpKind = 'copy' | 'move' | 'delete' | 'rename' | 'mkdir' | 'createFile'

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
  /** 搜索结果标签页使用的标题和查询词 */
  kind?: 'files' | 'search'
  title?: string
  searchQuery?: string
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

// ---- 定时任务 ----

/** 触发规则（简单预设）。 */
export interface TaskSchedule {
  kind: 'interval' | 'daily' | 'weekly' | 'monthly'
  /** interval：间隔分钟数 */
  intervalMinutes?: number | null
  /** daily / weekly / monthly：小时（0-23） */
  hour?: number | null
  /** daily / weekly / monthly：分钟（0-59） */
  minute?: number | null
  /** weekly：0=周日 … 6=周六 */
  weekday?: number | null
  /** monthly：1-28 */
  dayOfMonth?: number | null
}

/** 文件筛选条件；全部为空表示「不限」。 */
export interface TaskFilters {
  /** 扩展名白名单（小写、不含点）；空 = 不限 */
  extensions: string[]
  nameContains?: string | null
  minAgeDays?: number | null
  minSizeBytes?: number | null
  maxSizeBytes?: number | null
}

/** 任务动作；只作用于文件，绝不删除目录。 */
export interface TaskAction {
  kind: 'delete' | 'move'
  root: string
  recursive: boolean
  moveTarget?: string | null
  filters: TaskFilters
}

/** 一条定时任务。 */
export interface ScheduledTask {
  id: string
  name: string
  enabled: boolean
  schedule: TaskSchedule
  action: TaskAction
  createdAt: number
  lastRun?: number | null
  lastResult?: string | null
  nextRun?: number | null
  runCount: number
}

/** AI 规划出的任务草稿。 */
export interface TaskDraft {
  name: string
  schedule: TaskSchedule
  action: TaskAction
}

/** AI 任务规划回复。 */
export interface TaskPlanReply {
  text: string
  draft?: TaskDraft | null
}

/** 预演（dry-run）结果。 */
export interface TaskPreview {
  count: number
  totalSize: number
  samples: string[]
  truncated: boolean
}

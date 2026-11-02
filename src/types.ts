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
  usingEverything: boolean
  waitingForEverything: boolean
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
  /** 因权限不足（Windows ERROR_ACCESS_DENIED）失败的路径；用于发起 UAC 提权重试。 */
  denied?: string[]
}

export interface AiMessage {
  role: 'system' | 'user' | 'assistant'
  content: string
}

export interface AiChatMessage extends AiMessage {
  id: string
  createdAt: number
  status: 'pending' | 'complete' | 'error' | 'interrupted'
  model: string
  kind: 'message' | 'operation'
  operation?: 'pending' | 'completed' | 'cancelled' | 'expired' | 'error' | null
  /** 该轮回复的 token 用量（旧记录无此字段） */
  usage?: AiUsage | null
  /** 该轮回复耗时（毫秒，旧记录无此字段） */
  durationMs?: number | null
}

export interface AiConversation {
  id: string
  title: string
  createdAt: number
  updatedAt: number
  messages: AiChatMessage[]
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
  /** 用户中途停止了生成，text 为已收到的部分 */
  interrupted: boolean
  usage?: AiUsage | null
}

/** 一次回复的 token 用量。 */
export interface AiUsage {
  inputTokens: number
  outputTokens: number
}

export type ToolOutcome =
  | { kind: 'search'; payload: SearchHit[] }
  | { kind: 'listing'; payload: DirListing }
  | { kind: 'fs'; payload: FsOpResult }
  | { kind: 'text'; payload: string }

export interface AiConfig {
  enabled: boolean
  providers: AiProvider[]
  activeProviderId: string
  activeModelId: string
}

export type AiProtocol = 'openai-chat' | 'openai-responses' | 'anthropic' | 'gemini'

export interface AiModel {
  id: string
  name: string
  vendor: string
}

export interface AiProvider {
  id: string
  name: string
  vendor: string
  protocol: AiProtocol
  baseUrl: string
  apiKey: string
  models: AiModel[]
}

// ---- 视图模型（前端专用） ----

/** FTP / SFTP 的认证方式：密码 / SSH 私钥。 */
export type RemoteAuthMethod = 'password' | 'key'

/** 一个 FTP / SFTP 连接的连接信息（密码不在此结构中，见 `src/shell/remote.ts`）。 */
export interface RemoteLocation {
  /** 侧栏主机的稳定 id（也用于系统凭据库的 service 名） */
  id: string
  protocol: 'ftp' | 'sftp'
  host: string
  port: number
  username: string
  /** 侧栏显示名（用作标签标题） */
  label: string
  /** 认证方式；缺省为密码，兼容旧配置 */
  authMethod?: RemoteAuthMethod
  /** 私钥文件路径（`authMethod === 'key'` 时有效） */
  privateKey?: string
}

/** 一个待传输条目：下载时 path 为远端 POSIX 路径，上传时为本地路径。 */
export interface RemoteTransferItem {
  path: string
  isDir: boolean
}

export interface RemoteTransferRequest {
  protocol: 'ftp' | 'sftp'
  host: string
  port: number
  username: string
  password: string
  /** 认证方式与私钥信息（缺省为密码认证） */
  authMethod?: RemoteAuthMethod
  privateKey?: string
  keyPassphrase?: string
  direction: 'download' | 'upload'
  /** 下载目标本地目录 / 上传目标远端目录 */
  target: string
  items: RemoteTransferItem[]
  /** 传输成功后删除源（剪切 / 移动） */
  removeSources?: boolean
}

export interface RemoteTransferResult {
  ok: boolean
  errors: string[]
  transferred: number
  bytes: number
}

export interface RemoteTransferProgress {
  direction: 'download' | 'upload'
  done: number
  total: number
  current: string
}

// ---- SSH 终端 ----

export interface SshConnectRequest {
  /** 前端生成，用于事件路由 */
  sessionId: string
  host: string
  port: number
  username: string
  authMethod?: RemoteAuthMethod
  password?: string
  privateKey?: string
  keyPassphrase?: string
  cols?: number
  rows?: number
}

export interface SshSessionInfo {
  sessionId: string
  /** 服务器主机密钥 SHA256 指纹（展示给用户核对） */
  fingerprint: string
}

/** 终端输出：`data` 为 base64 编码的原始字节。 */
export interface SshDataEvent {
  sessionId: string
  data: string
}

export interface SshExitEvent {
  sessionId: string
  message: string
  code: number | null
}

export interface TabState {
  id: string
  path: string
  /** 标签页类型：普通目录 / 搜索结果 / 远程（FTP/SFTP）目录 / SSH 终端 */
  kind?: 'files' | 'search' | 'remote' | 'terminal'
  title?: string
  searchQuery?: string
  /** 远程标签的连接信息（`kind === 'remote'` 时有效）；`path` 为远程 POSIX 路径 */
  remote?: RemoteLocation
  /** SSH 终端标签的会话 id（`kind === 'terminal'` 时有效），路由到 `stores/terminal.ts` */
  terminalId?: string
  entries: FileEntry[]
  loading: boolean
  error: string | null
  /** 当前目录内的本地过滤词 */
  filter: string
  /** 当前目录内的类型筛选（searchFilters 分类 key；null/缺省 = 全部） */
  typeFilter?: string | null
  sortKey: 'name' | 'path' | 'size' | 'modified' | 'ext'
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

/** 意见反馈提交请求（版本 / commit 由前端注入）。 */
export interface FeedbackRequest {
  /** 反馈类型：Bug / 功能建议 / 其它 */
  kind: string
  title: string
  description: string
  appVersion: string
  commit: string
}

/** 反馈提交结果：新建 issue 的编号与网页链接。 */
export interface FeedbackCreated {
  number: string
  url: string
}

/** GitHub CLI 的可用性与登录状态。 */
export interface FeedbackAuth {
  cliFound: boolean
  loggedIn: boolean
  user?: string | null
  message: string
}

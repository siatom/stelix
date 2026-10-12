import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import type {
  AiConfig,
  AiContext,
  AiMessage,
  AiReply,
  AiToolCall,
  DirListing,
  DriveInfo,
  FsOp,
  FsOpResult,
  IndexStatus,
  ScheduledTask,
  SearchHit,
  SearchQuery,
  TaskAction,
  TaskPlanReply,
  TaskPreview,
  ToolOutcome,
} from '../types'

/** 后端 command 封装（与 src-tauri/src/lib.rs 注册表一一对应） */
export const api = {
  listDir: (path: string) => invoke<DirListing>('list_dir', { path }),
  getRoots: () => invoke<DriveInfo[]>('get_roots'),
  listRemote: (request: { protocol: 'ftp' | 'sftp'; host: string; port: number; username: string; password: string; path: string }) =>
    invoke<string[]>('list_remote', { request }),
  discoverSmbNeighbors: () => invoke<{ server: string; shares: string[] }[]>('discover_smb_neighbors'),
  listSmbShares: (server: string) => invoke<string[]>('list_smb_shares', { server }),
  listWslDistros: () => invoke<string[]>('list_wsl_distros'),
  saveRemotePassword: (hostId: string, password: string) => invoke<void>('save_remote_password', { hostId, password }),
  getRemotePassword: (hostId: string) => invoke<string | null>('get_remote_password', { hostId }),
  deleteRemotePassword: (hostId: string) => invoke<void>('delete_remote_password', { hostId }),
  fsOperation: (op: FsOp) => invoke<FsOpResult>('fs_operation', { op }),
  setFileClipboard: (paths: string[], moveFiles: boolean) =>
    invoke<void>('set_file_clipboard', { paths, moveFiles }),
  getFileClipboard: () => invoke<{ paths: string[]; moveFiles: boolean } | null>('get_file_clipboard'),
  /** 取文件/文件夹的 Windows 系统图标（base64 PNG data URL）；失败返回 null。 */
  getFileIcon: (path: string, isDir: boolean) =>
    invoke<string | null>('get_file_icon', { path, isDir }),
  pathIsFile: (path: string) => invoke<boolean>('path_is_file', { path }),
  openInTerminal: (path: string) => invoke<void>('open_in_terminal', { path }),
  showItemProperties: (path: string) => invoke<void>('show_item_properties', { path }),

  startIndex: (roots: string[]) => invoke<void>('start_index', { roots }),
  indexStatus: () => invoke<IndexStatus>('index_status'),
  searchFiles: (query: SearchQuery) => invoke<SearchHit[]>('search_files', { query }),

  aiChat: (messages: AiMessage[], context: AiContext) =>
    invoke<AiReply>('ai_chat', { messages, context }),
  aiExecuteTool: (call: AiToolCall, approved: boolean) =>
    invoke<ToolOutcome>('ai_execute_tool', { call, approved }),
  aiConfigGet: () => invoke<AiConfig>('ai_config_get'),
  aiConfigSet: (config: AiConfig) => invoke<void>('ai_config_set', { config }),
  /** 用大模型把自然语言需求规划为一条定时任务草稿（离线时走规则解析兜底）。 */
  aiPlanTask: (prompt: string, context: AiContext) =>
    invoke<TaskPlanReply>('ai_plan_task', { prompt, context }),

  taskList: () => invoke<ScheduledTask[]>('task_list'),
  taskSave: (task: ScheduledTask) => invoke<ScheduledTask>('task_save', { task }),
  taskDelete: (id: string) => invoke<void>('task_delete', { id }),
  taskSetEnabled: (id: string, enabled: boolean) =>
    invoke<void>('task_set_enabled', { id, enabled }),
  taskRunNow: (id: string) => invoke<string>('task_run_now', { id }),
  /** 预演（dry-run）：统计将命中的文件，不产生任何副作用。 */
  taskPreview: (action: TaskAction) => invoke<TaskPreview>('task_preview', { action }),

  /** 全量设置需要监听的目录集合（幂等；后端只接受本地盘符目录）。 */
  watchDirsSet: (paths: string[]) => invoke<void>('watch_dirs_set', { paths }),

  /** 请求一批目录的递归内部大小（只上报详情视图当前可见的目录）；结果经 `fs:dir-size` 事件回填。 */
  dirSizeRequest: (paths: string[]) => invoke<void>('dir_size_request', { paths }),
  /** 使一批目录的大小缓存失效（目录内容变化时调用），下次请求会重新遍历。 */
  dirSizeInvalidate: (paths: string[]) => invoke<void>('dir_size_invalidate', { paths }),

  autostartGet: () => invoke<boolean>('autostart_get'),
  autostartSet: (enabled: boolean) => invoke<void>('autostart_set', { enabled }),
}

/** 后端事件订阅 */
export const events = {
  onIndexProgress: (cb: (p: unknown) => void) => listen('index:progress', (e) => cb(e.payload)),
  onIndexDone: (cb: (p: unknown) => void) => listen('index:done', (e) => cb(e.payload)),
  onSearchResult: (cb: (p: unknown) => void) => listen('search:result', (e) => cb(e.payload)),
  onAiToken: (cb: (p: string) => void) => listen('ai:token', (e) => cb(e.payload as string)),
  onFsProgress: (cb: (p: string) => void) => listen('fs:progress', (e) => cb(e.payload as string)),
  /** 定时任务状态变化（新增 / 运行 / 启用状态改变）。 */
  onTaskUpdated: (cb: (id: string) => void) => listen('task:updated', (e) => cb(e.payload as string)),
  /** 目录内容变更（后端 250ms 防抖后上报受影响的目录列表）。 */
  onFsChanged: (cb: (p: { dirs: string[] }) => void) =>
    listen('fs:changed', (e) => cb(e.payload as { dirs: string[] })),
  /** 目录大小统计结果（详情视图按需计算）。 */
  onDirSize: (cb: (p: { path: string; size: number }) => void) =>
    listen('fs:dir-size', (e) => cb(e.payload as { path: string; size: number })),
}

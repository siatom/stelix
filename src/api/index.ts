import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import type {
  AiApprovalDecision,
  AiAuditEntry,
  AiConfig,
  AiConversation,
  AiContext,
  AiMessage,
  AiModel,
  AiProvider,
  AiReply,
  AiToolCall,
  CloudAccountInfo,
  ConflictItem,
  DirListing,
  DriveInfo,
  FsOp,
  FsOpResult,
  FsProgress,
  FeedbackAuth,
  FeedbackCreated,
  FeedbackRequest,
  IndexStatus,
  RemoteProtocol,
  RemoteTransferProgress,
  RemoteTransferRequest,
  RemoteTransferResult,
  ScheduledTask,
  SearchHit,
  SearchQuery,
  SshConnectRequest,
  SshSessionInfo,
  SshDataEvent,
  SyncExecuteRequest,
  SyncPlan,
  SyncRequest,
  SyncResult,
  SshExitEvent,
  TaskAction,
  TaskPlanReply,
  TaskPreview,
  ToolOutcome,
  TrashEntry,
  TrashOpResult,
} from '../types'

/** 后端 command 封装（与 src-tauri/src/lib.rs 注册表一一对应） */
export const api = {
  listDir: (path: string) => invoke<DirListing>('list_dir', { path }),
  getRoots: () => invoke<DriveInfo[]>('get_roots'),
  /** 列出 FTP / SFTP 目录，条目带类型 / 大小 / 修改时间。 */
  listRemoteDir: (request: {
    protocol: RemoteProtocol
    host: string
    port: number
    username: string
    password: string
    authMethod?: 'password' | 'key'
    privateKey?: string
    keyPassphrase?: string
    accountId?: string
    path: string
  }) => invoke<DirListing>('list_remote_dir', { request }),
  /** 在本地与 FTP / SFTP 之间上传或下载（目录递归）；进度见 `events.onRemoteTransfer`。 */
  remoteTransfer: (request: RemoteTransferRequest) =>
    invoke<RemoteTransferResult>('remote_transfer', { request }),
  discoverSmbNeighbors: () => invoke<{ server: string; shares: string[] }[]>('discover_smb_neighbors'),
  listSmbShares: (server: string) => invoke<string[]>('list_smb_shares', { server }),
  listWslDistros: () => invoke<string[]>('list_wsl_distros'),
  saveRemotePassword: (hostId: string, password: string) => invoke<void>('save_remote_password', { hostId, password }),
  getRemotePassword: (hostId: string) => invoke<string | null>('get_remote_password', { hostId }),
  deleteRemotePassword: (hostId: string) => invoke<void>('delete_remote_password', { hostId }),
  saveRemoteKeyPassphrase: (hostId: string, passphrase: string) =>
    invoke<void>('save_remote_key_passphrase', { hostId, passphrase }),
  getRemoteKeyPassphrase: (hostId: string) =>
    invoke<string | null>('get_remote_key_passphrase', { hostId }),
  deleteRemoteKeyPassphrase: (hostId: string) =>
    invoke<void>('delete_remote_key_passphrase', { hostId }),

  /** 云存储登录：百度走浏览器 OAuth，迅雷用账号密码或 refresh token；令牌存入系统凭据库。 */
  cloudBaiduLogin: (request: {
    accountId: string
    clientId: string
    clientSecret: string
    label?: string
  }) => invoke<CloudAccountInfo>('cloud_baidu_login', { request }),
  cloudXunleiLogin: (request: {
    accountId: string
    username: string
    password: string
    label?: string
  }) => invoke<CloudAccountInfo>('cloud_xunlei_login', { request }),
  cloudXunleiTokenLogin: (request: {
    accountId: string
    refreshToken: string
    label?: string
  }) => invoke<CloudAccountInfo>('cloud_xunlei_token_login', { request }),
  cloudLogout: (accountId: string) => invoke<void>('cloud_logout', { accountId }),

  /** 建立 SSH 终端会话（密码 / 私钥认证）；输出经 `ssh:data`、结束经 `ssh:exit` 事件回传。 */
  sshConnect: (request: SshConnectRequest) =>
    invoke<SshSessionInfo>('ssh_connect', { request }),
  sshWrite: (sessionId: string, data: string) =>
    invoke<void>('ssh_write', { sessionId, data }),
  sshResize: (sessionId: string, cols: number, rows: number) =>
    invoke<void>('ssh_resize', { sessionId, cols, rows }),
  sshClose: (sessionId: string) => invoke<void>('ssh_close', { sessionId }),
  /** 执行文件操作；传 opId 时可通过 fsCancel 中途取消，进度经 fs:progress 回传。 */
  fsOperation: (op: FsOp, opId?: string) =>
    invoke<FsOpResult>('fs_operation', { op, opId: opId ?? null }),
  /** 取消一个正在执行的文件操作（按 opId 匹配）。 */
  fsCancel: (opId: string) => invoke<void>('fs_cancel', { opId }),
  /** 预检同名冲突：返回目标位置已存在的条目，供 UI 选择替换 / 跳过 / 保留两者。 */
  fsCheckConflicts: (op: FsOp) => invoke<ConflictItem[]>('fs_check_conflicts', { op }),
  /** 提权执行一次因权限不足失败的文件操作（Windows UAC）。 */
  fsOperationElevated: (op: FsOp) => invoke<FsOpResult>('fs_operation_elevated', { op }),
  /** 扫描左右两个文件夹的差异（只读，不修改文件）。 */
  syncScan: (request: SyncRequest) => invoke<SyncPlan>('sync_scan', { request }),
  /** 执行同步；传 opId 时可用 fsCancel 取消，进度经 fs:progress 回传。 */
  syncExecute: (request: SyncExecuteRequest, opId?: string) =>
    invoke<SyncResult>('sync_execute', { request, opId: opId ?? null }),
  /** 列出系统回收站条目（按删除时间从新到旧）。 */
  trashList: () => invoke<TrashEntry[]>('trash_list'),
  /** 把回收站条目还原到原位置；同名冲突时逐项返回错误。 */
  trashRestore: (ids: string[]) => invoke<TrashOpResult>('trash_restore', { ids }),
  /** 永久删除回收站中的指定条目。 */
  trashPurge: (ids: string[]) => invoke<TrashOpResult>('trash_purge', { ids }),
  /** 清空回收站。 */
  trashEmpty: () => invoke<TrashOpResult>('trash_empty'),
  setFileClipboard: (paths: string[], moveFiles: boolean) =>
    invoke<void>('set_file_clipboard', { paths, moveFiles }),
  getFileClipboard: () => invoke<{ paths: string[]; moveFiles: boolean } | null>('get_file_clipboard'),
  /**
   * 取文件/文件夹的 Windows 系统图标（base64 PNG data URL）；失败返回 null。
   * `emptyTrash` 固定返回空桶图标；`trashState` 跟随回收站当前是否有内容。
   */
  getFileIcon: (
    path: string,
    isDir: boolean,
    usePathIcon = false,
    emptyTrash = false,
    trashState = false,
  ) => invoke<string | null>('get_file_icon', { path, isDir, usePathIcon, emptyTrash, trashState }),
  pathIsFile: (path: string) => invoke<boolean>('path_is_file', { path }),
  /** 展开地址栏输入中的 Windows 环境变量（`%APPDATA%`）与 `~` 主目录。 */
  expandPath: (path: string) => invoke<string>('expand_path', { path }),
  openInTerminal: (path: string) => invoke<void>('open_in_terminal', { path }),
  showItemProperties: (path: string) => invoke<void>('show_item_properties', { path }),
  /** 弹出系统原生「打开方式」窗口（仅本地文件）。 */
  showOpenWith: (path: string) => invoke<void>('show_open_with', { path }),
  /**
   * 弹出 Windows Shell 菜单，并在末尾追加应用自定义操作。
   * 返回被选中的应用项 id（如 duplicate / copy-to）；选中系统项或取消时返回 null。
   */
  showNativeShellMenu: (
    path: string,
    anchor?: { x: number; y: number; scale: number } | null,
    items?: { id: string; label: string }[] | null,
  ) =>
    invoke<string | null>('show_native_shell_menu', {
      path,
      anchor: anchor ?? null,
      items: items ?? null,
    }),

  startIndex: (roots: string[]) => invoke<void>('start_index', { roots }),
  setIndexRoots: (roots: string[]) => invoke<void>('set_index_roots', { roots }),
  retryEverythingIndex: () => invoke<void>('retry_everything_index'),
  indexStatus: () => invoke<IndexStatus>('index_status'),
  searchFiles: (query: SearchQuery) => invoke<SearchHit[]>('search_files', { query }),

  aiChat: (messages: AiMessage[], context: AiContext, requestId: string) =>
    invoke<AiReply>('ai_chat', { messages, context, requestId }),
  /** 请求停止某个正在流式生成的回复（按 requestId 匹配）。 */
  aiChatCancel: (id: string) => invoke<void>('ai_chat_cancel', { id }),
  /** 用户逐项确认后继续一个挂起的 Agent run。 */
  aiAgentResume: (runId: string, requestId: string, decisions: AiApprovalDecision[]) =>
    invoke<AiReply>('ai_agent_resume', { runId, requestId, decisions }),
  /** 放弃一个等待审批的 run。 */
  aiAgentCancel: (runId: string) => invoke<void>('ai_agent_cancel', { runId }),
  /** 读取最近的写操作审计记录。 */
  aiAgentAudit: (limit: number) => invoke<AiAuditEntry[]>('ai_agent_audit', { limit }),
  /** 拉取某个 Provider 可用的模型列表。 */
  aiListModels: (provider: AiProvider) => invoke<AiModel[]>('ai_list_models', { provider }),
  aiExecuteTool: (call: AiToolCall, approved: boolean) =>
    invoke<ToolOutcome>('ai_execute_tool', { call, approved }),
  aiConfigGet: () => invoke<AiConfig>('ai_config_get'),
  aiConfigSet: (config: AiConfig) => invoke<void>('ai_config_set', { config }),
  aiTestConnection: (config: AiConfig) => invoke<string>('ai_test_connection', { config }),
  aiConversationsList: () => invoke<AiConversation[]>('ai_conversations_list'),
  aiConversationSave: (conversation: AiConversation) => invoke<void>('ai_conversation_save', { conversation }),
  aiConversationDelete: (id: string) => invoke<void>('ai_conversation_delete', { id }),
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

  /** 检查本机 GitHub CLI 是否可用并已登录（意见反馈入口）。 */
  feedbackAuthStatus: () => invoke<FeedbackAuth>('feedback_auth_status'),
  /** 通过本机 GitHub CLI 把反馈提交为 GitHub issue，返回编号与链接。 */
  feedbackSubmit: (request: FeedbackRequest) => invoke<FeedbackCreated>('feedback_submit', { request }),
}

/** 后端事件订阅 */
export const events = {
  onIndexProgress: (cb: (p: unknown) => void) => listen('index:progress', (e) => cb(e.payload)),
  onIndexDone: (cb: (p: unknown) => void) => listen('index:done', (e) => cb(e.payload)),
  onSearchResult: (cb: (p: unknown) => void) => listen('search:result', (e) => cb(e.payload)),
  /** 文件操作进度（复制 / 移动 / 删除的扫描与执行进度，按 opId 归属）。 */
  onFsProgress: (cb: (p: FsProgress) => void) =>
    listen('fs:progress', (e) => cb(e.payload as FsProgress)),
  /** 定时任务状态变化（新增 / 运行 / 启用状态改变）。 */
  onTaskUpdated: (cb: (id: string) => void) => listen('task:updated', (e) => cb(e.payload as string)),
  /** 目录内容变更（后端 250ms 防抖后上报受影响的目录列表）。 */
  onFsChanged: (cb: (p: { dirs: string[] }) => void) =>
    listen('fs:changed', (e) => cb(e.payload as { dirs: string[] })),
  /** 目录大小统计结果（详情视图按需计算）。 */
  onDirSize: (cb: (p: { path: string; size: number }) => void) =>
    listen('fs:dir-size', (e) => cb(e.payload as { path: string; size: number })),
  /** AI 流式文本增量。 */
  onAiToken: (cb: (p: { requestId: string; delta: string }) => void) =>
    listen('ai:token', (e) => cb(e.payload as { requestId: string; delta: string })),
  /** AI 工具执行步骤提示。 */
  onAiStep: (cb: (p: { requestId: string; step: string }) => void) =>
    listen('ai:step', (e) => cb(e.payload as { requestId: string; step: string })),
  /** 远程传输进度（每开始一个顶层条目回调一次）。 */
  onRemoteTransfer: (cb: (p: RemoteTransferProgress) => void) =>
    listen('remote:transfer', (e) => cb(e.payload as RemoteTransferProgress)),
  /** SSH 终端输出（base64 字节）。 */
  onSshData: (cb: (p: SshDataEvent) => void) =>
    listen('ssh:data', (e) => cb(e.payload as SshDataEvent)),
  /** SSH 会话结束（远程 shell 退出 / 网络断开 / 主动关闭）。 */
  onSshExit: (cb: (p: SshExitEvent) => void) =>
    listen('ssh:exit', (e) => cb(e.payload as SshExitEvent)),
}

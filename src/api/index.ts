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
  SearchHit,
  SearchQuery,
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
}

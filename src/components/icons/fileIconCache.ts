/**
 * 系统文件图标缓存。
 *
 * 按「图标种类」缓存（与后端 `src-tauri/src/shell_icon.rs` 的 `cache_key` 同规则）：
 * - 目录 → `dir`
 * - 磁盘根目录 → `drive:<path>`
 * - 系统位置 → `system:<path>`
 * - 空回收站 → `system:empty-recycle-bin`
 * - 跟随状态的回收站 → `system:recycle-bin:v<版本>`
 * - 普通文件 → `e:<ext>`
 * - 自带图标资源的文件（exe/lnk/ico/…）→ `p:<小写路径>`
 *
 * 同一 key 的所有组件共享同一个 `Ref`，因此同一扩展名只会发起一次 IPC，
 * 且异步结果返回后自动触发视图更新。
 *
 * ⚠️ 扩展名必须按 Windows Shell 规则从**文件名**推导：`.env` / `.gitignore`
 * 等 dotfile 同样以点后部分作为扩展名。若把它们都当作「无扩展名」落入同一个
 * `e:`，先加载的 dotfile 图标会覆盖其余文件——例如 `.gitignore` 可能显示
 * `.env` 的图标（看起来像文件夹）。
 */
import { ref, type Ref } from 'vue'
import { api } from '../../api'

/** 自带图标资源、需要按「每个文件」单独提取的扩展名（与后端保持一致）。 */
const PER_FILE_EXTS = new Set([
  'exe', 'lnk', 'ico', 'msi', 'scr', 'cpl', 'url', 'cur', 'ani', 'appx',
])

const refs = new Map<string, Ref<string | null>>()

/**
 * 回收站状态版本号。回收站内容变化（删除/还原/清空等）后递增，
 * 使「跟随回收站状态」的图标换用新 key 重新取图。
 */
const trashVersion = ref(0)

/** 通知回收站图标重新加载（回收站内容变化后调用）。 */
export function refreshTrashIcon() {
  trashVersion.value += 1
}

/** 取文件名最后一段（与后端 `extension_of` 的输入保持一致）。 */
function baseName(path: string): string {
  return path.slice(Math.max(path.lastIndexOf('\\'), path.lastIndexOf('/')) + 1)
}

/** Windows Shell 规则的扩展名：文件名最后一个点之后的部分（小写）；无扩展名返回空串。 */
function iconExtension(path: string): string {
  const name = baseName(path)
  const dot = name.lastIndexOf('.')
  return dot >= 0 && dot + 1 < name.length ? name.slice(dot + 1).toLowerCase() : ''
}

/** 计算缓存 key。 */
export function iconKey(
  path: string,
  isDir: boolean,
  isDrive = false,
  usePathIcon = false,
  emptyTrash = false,
  trashState = false,
): string {
  // 空回收站使用固定的空桶图标，与回收站当前的系统图标分开缓存。
  if (emptyTrash) return 'system:empty-recycle-bin'
  // 跟随回收站状态：按版本号换 key，内容变化后重新请求。
  if (trashState) return `system:recycle-bin:v${trashVersion.value}`
  if (isDrive) return `drive:${path.toLowerCase()}`
  if (usePathIcon) return `system:${path.toLowerCase()}`
  if (isDir) return 'dir'
  const ext = iconExtension(path)
  return PER_FILE_EXTS.has(ext) ? `p:${path.toLowerCase()}` : `e:${ext}`
}

/**
 * 取指定条目的系统图标（`data:image/png;base64,...`）。
 * 加载中 / 失败时保持 `null`，由调用方回退到内置矢量图标。
 */
export function fileIconRef(
  path: string,
  isDir: boolean,
  isDrive = false,
  usePathIcon = false,
  emptyTrash = false,
  trashState = false,
): Ref<string | null> {
  const key = iconKey(path, isDir, isDrive, usePathIcon, emptyTrash, trashState)
  const existing = refs.get(key)
  if (existing) return existing

  const url = ref<string | null>(null)
  refs.set(key, url)
  api
    .getFileIcon(path, isDir, usePathIcon, emptyTrash, trashState)
    .then((value) => {
      if (value) url.value = value
    })
    .catch(() => {
      // 失败保持 null → 回退矢量图标
    })
  return url
}

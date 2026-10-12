/**
 * 系统文件图标缓存。
 *
 * 按「图标种类」缓存（与后端 `src-tauri/src/shell_icon.rs` 的 `cache_key` 同规则）：
 * - 目录 → `dir`
 * - 磁盘根目录 → `drive:<path>`
 * - 系统位置 → `system:<path>`
 * - 普通文件 → `e:<ext>`
 * - 自带图标资源的文件（exe/lnk/ico/…）→ `p:<小写路径>`
 *
 * 同一 key 的所有组件共享同一个 `Ref`，因此同一扩展名只会发起一次 IPC，
 * 且异步结果返回后自动触发视图更新。
 */
import { ref, type Ref } from 'vue'
import { api } from '../../api'

/** 自带图标资源、需要按「每个文件」单独提取的扩展名（与后端保持一致）。 */
const PER_FILE_EXTS = new Set([
  'exe', 'lnk', 'ico', 'msi', 'scr', 'cpl', 'url', 'cur', 'ani', 'appx',
])

const refs = new Map<string, Ref<string | null>>()

/** 计算缓存 key。 */
export function iconKey(path: string, isDir: boolean, ext: string | null, isDrive = false, usePathIcon = false): string {
  if (isDrive) return `drive:${path.toLowerCase()}`
  if (usePathIcon) return `system:${path.toLowerCase()}`
  if (isDir) return 'dir'
  const e = (ext ?? '').replace(/^\./, '').toLowerCase()
  return PER_FILE_EXTS.has(e) ? `p:${path.toLowerCase()}` : `e:${e}`
}

/**
 * 取指定条目的系统图标（`data:image/png;base64,...`）。
 * 加载中 / 失败时保持 `null`，由调用方回退到内置矢量图标。
 */
export function fileIconRef(
  path: string,
  isDir: boolean,
  ext: string | null,
  isDrive = false,
  usePathIcon = false,
): Ref<string | null> {
  const key = iconKey(path, isDir, ext, isDrive, usePathIcon)
  const existing = refs.get(key)
  if (existing) return existing

  const url = ref<string | null>(null)
  refs.set(key, url)
  api
    .getFileIcon(path, isDir, usePathIcon)
    .then((value) => {
      if (value) url.value = value
    })
    .catch(() => {
      // 失败保持 null → 回退矢量图标
    })
  return url
}

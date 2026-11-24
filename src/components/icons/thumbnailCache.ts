/**
 * 图标视图图片缩略图缓存。
 *
 * 只有本地常见位图格式才请求缩略图：
 * - 结果缓存在「路径 + 修改时间 + 文件大小 + 目标边长」上，文件改名或内容变化后
 *   自动换成新 key；
 * - 缓存按最近使用顺序限制条目数，长时间浏览大量图片不会无限增长；
 * - 远程（UNC / WSL）路径不请求，继续由调用方显示系统类型图标。
 *
 * 组件只在单元格进入视口附近时才调用 `requestThumbnail`，因此请求量随可见区域
 * 增长，而不是随目录规模线性增长。
 */
import { api } from '../../api'

/** 与后端 `thumbnail.rs` 的 `SUPPORTED_EXTS` 保持一致。 */
const THUMBNAIL_EXTS = new Set(['png', 'jpg', 'jpeg', 'webp', 'gif', 'bmp'])

/** 缓存条目上限：一张缩略图约十几 KB，400 条约几 MB。 */
const MAX_CACHE_ENTRIES = 400

const cache = new Map<string, string>()
const inflight = new Map<string, Promise<string | null>>()

function normalizedExt(ext: string | null): string {
  return (ext ?? '').replace(/^\./, '').toLowerCase()
}

/** 是否值得生成缩略图：图片、非目录、非远程路径。 */
export function isThumbnailCandidate(
  path: string,
  ext: string | null,
  isDir: boolean,
): boolean {
  if (isDir || !path) return false
  if (!THUMBNAIL_EXTS.has(normalizedExt(ext))) return false
  // UNC 覆盖 SMB 共享与 WSL（\\wsl$ / \\wsl.localhost）；FTP / SFTP 标签由调用方过滤。
  return !path.startsWith('\\\\') && !path.startsWith('//')
}

function cacheKey(path: string, modified: number, size: number, edge: number): string {
  return `${path}\u0000${modified}\u0000${size}\u0000${edge}`
}

/** 写入并刷新 LRU 顺序，超出上限时淘汰最久未使用的条目。 */
function remember(key: string, url: string) {
  cache.delete(key)
  cache.set(key, url)
  while (cache.size > MAX_CACHE_ENTRIES) {
    const oldest = cache.keys().next().value
    if (oldest === undefined) break
    cache.delete(oldest)
  }
}

/**
 * 取缩略图（`data:image/png;base64,...`）；失败返回 `null`。
 *
 * 命中缓存同步返回（仍是 Promise，调用方无分支）；同一 key 的并发请求共享
 * 同一次 IPC，不会重复解码。
 */
export function requestThumbnail(
  path: string,
  modified: number,
  size: number,
  edge: number,
): Promise<string | null> {
  const key = cacheKey(path, modified, size, edge)
  const hit = cache.get(key)
  if (hit) {
    remember(key, hit)
    return Promise.resolve(hit)
  }
  const pending = inflight.get(key)
  if (pending) return pending

  const task = api
    .getThumbnail(path, edge)
    .then((url) => {
      if (url) remember(key, url)
      return url
    })
    .catch(() => null)
    .finally(() => {
      inflight.delete(key)
    })
  inflight.set(key, task)
  return task
}

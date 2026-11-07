//! 目录大小统计：为「详情视图」提供文件夹的递归内部大小。
//!
//! 直接同步遍历会阻塞列表渲染，因此这里把遍历放到**有界并发的后台线程池**里，
//! 只按需（前端仅上报当前可见的目录）计算，结果带 TTL 缓存并通过 `fs:dir-size`
//! 事件回填前端。
//!
//! 速度优化要点：
//! - **仅可见行**：前端只请求详情视图里当前可见的文件夹，通常十几行；
//! - **有界并发**：固定线程池（`WORKERS`），避免同时压满磁盘；
//! - **缓存 + 去重**：结果带 TTL 缓存，同一目录在途时不会重复遍历；
//! - **跳过重解析点**：junction / symlink 不进入，避免环与重复计数；
//! - **可取消**：滚出视口的目录在遍历中途被取消，不再浪费 I/O。

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use rayon::ThreadPool;
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use walkdir::WalkDir;

/// 结果事件名，与前端 `events.onDirSize` 对应。
pub const DIR_SIZE_EVENT: &str = "fs:dir-size";

/// 缓存有效期：过期后再次请求会重新遍历（作为兜底，正常情况下由 `invalidate` 主动失效）。
const CACHE_TTL: Duration = Duration::from_secs(120);
/// 缓存条目上限；淘汰最久未使用的结果，限制历史浏览路径占用。
const CACHE_CAPACITY: usize = 1024;
/// 后台并发遍历上限，避免同时压满磁盘。
const WORKERS: usize = 4;
/// 遍历中每处理这么多条目检查一次「是否仍被需要」，用于取消滚出视口的目录。
const CANCEL_CHECK_EVERY: usize = 512;

/// `fs:dir-size` 事件负载。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DirSizePayload {
    path: String,
    size: u64,
}

struct Cached {
    size: u64,
    at: Instant,
    last_used: Instant,
}

struct SizeCache {
    entries: HashMap<String, Cached>,
    capacity: usize,
}

impl SizeCache {
    fn new(capacity: usize) -> Self {
        Self {
            entries: HashMap::new(),
            capacity,
        }
    }

    // 每批请求（包括空集合）、写入及失效时维护；命中只做单项查询。
    fn maintain(&mut self, now: Instant) {
        self.entries
            .retain(|_, value| now.saturating_duration_since(value.at) < CACHE_TTL);
    }

    fn get(&mut self, path: &str, now: Instant) -> Option<u64> {
        let value = self.entries.get_mut(path)?;
        if now.saturating_duration_since(value.at) >= CACHE_TTL {
            self.entries.remove(path);
            return None;
        }
        value.last_used = now;
        Some(value.size)
    }

    fn insert(&mut self, path: String, size: u64, now: Instant) {
        self.maintain(now);
        if self.capacity == 0 {
            return;
        }
        if !self.entries.contains_key(&path) && self.entries.len() >= self.capacity {
            if let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, value)| value.last_used)
                .map(|(key, _)| key.clone())
            {
                self.entries.remove(&oldest);
            }
        }
        self.entries.insert(
            path,
            Cached {
                size,
                at: now,
                last_used: now,
            },
        );
    }

    fn invalidate(&mut self, paths: &[String], now: Instant) {
        self.maintain(now);
        self.entries
            .retain(|key, _| !paths.iter().any(|path| is_related(key, path)));
    }
}

struct Inner {
    cache: Mutex<SizeCache>,
    /// 正在遍历中的路径（去重，避免同一目录重复入队）。
    pending: Mutex<HashSet<String>>,
    /// 前端当前「需要」的目录集合；不在其中的在途遍历会被取消。
    wanted: Mutex<HashSet<String>>,
    pool: ThreadPool,
}

enum SizeRequest {
    Cached(u64),
    Pending,
    Compute,
}

impl Inner {
    fn lookup_or_queue(&self, path: &str, now: Instant) -> SizeRequest {
        // 与缓存回填一起检查在途状态，避免完成与新请求交错时重复遍历。
        let mut pending = self.pending.lock();
        if let Some(size) = self.cache.lock().get(path, now) {
            return SizeRequest::Cached(size);
        }
        if pending.insert(path.to_owned()) {
            SizeRequest::Compute
        } else {
            SizeRequest::Pending
        }
    }
}

/// 目录大小统计服务，全局单例（存于 `AppState`）。
pub struct DirSizeService {
    inner: Arc<Inner>,
}

impl DirSizeService {
    pub fn new() -> Self {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(WORKERS)
            .thread_name(|i| format!("dir-size-{i}"))
            .build()
            .expect("初始化目录大小线程池失败");
        Self {
            inner: Arc::new(Inner {
                cache: Mutex::new(SizeCache::new(CACHE_CAPACITY)),
                pending: Mutex::new(HashSet::new()),
                wanted: Mutex::new(HashSet::new()),
                pool,
            }),
        }
    }

    /// 请求一批目录的大小。
    ///
    /// `paths` 是前端当前**全部**需要计算的目录（各窗格可见行的并集）：
    /// - 不在其中的在途遍历会被取消；
    /// - 已缓存且未过期的直接回填（重发事件，保证前端能拿到值）；
    /// - 其余入队计算。
    pub fn request(&self, app: &AppHandle, paths: Vec<String>) {
        let wanted: HashSet<String> = paths.into_iter().filter(|p| !p.trim().is_empty()).collect();
        *self.inner.wanted.lock() = wanted.clone();

        let now = Instant::now();
        self.inner.cache.lock().maintain(now);
        for path in wanted {
            match self.inner.lookup_or_queue(&path, now) {
                SizeRequest::Cached(size) => {
                    let _ = app.emit(DIR_SIZE_EVENT, DirSizePayload { path, size });
                    continue;
                }
                SizeRequest::Pending => continue,
                SizeRequest::Compute => {}
            }

            let inner = Arc::clone(&self.inner);
            let app = app.clone();
            self.inner.pool.spawn(move || {
                let cancelled = || !inner.wanted.lock().contains(&path);
                let size = compute_size(&path, &cancelled);
                // 先写入结果再释放在途标记，避免同一路径在两者之间重复入队。
                if let Some(size) = size {
                    inner
                        .cache
                        .lock()
                        .insert(path.clone(), size, Instant::now());
                    inner.pending.lock().remove(&path);
                    let _ = app.emit(DIR_SIZE_EVENT, DirSizePayload { path, size });
                } else {
                    inner.pending.lock().remove(&path);
                }
            });
        }
    }

    /// 使一批目录的大小缓存失效（目录内容变化时调用），下次请求会重新遍历。
    ///
    /// 变更目录 `D` 会牵连三类缓存项：`D` 自身、`D` 的**祖先**（`D` 的内容变了，
    /// 祖先的总和随之变化），以及 `D` 的**后代**——因为 `notify` 只监听被打开目录的
    /// 直接子项，深层变更会以「被监听目录自身」的路径上报（子目录 mtime 变化被视作
    /// 该子项的修改），此时其后代目录的大小同样需要重算。
    pub fn invalidate(&self, paths: &[String]) {
        self.inner.cache.lock().invalidate(paths, Instant::now());
    }
}

/// 去掉尾部路径分隔符并统一小写（Windows 路径大小写不敏感）。
fn normalize(p: &str) -> String {
    p.trim_end_matches(['\\', '/']).to_lowercase()
}

/// `a` 与 `b` 是否同一路径，或一方位于另一方之下（含分隔符边界）。大小写不敏感。
fn is_related(a: &str, b: &str) -> bool {
    let a = normalize(a);
    let b = normalize(b);
    if a.is_empty() || b.is_empty() {
        return false;
    }
    under(&a, &b) || under(&b, &a)
}

/// `child` 是否等于 `ancestor`，或位于其之下。
fn under(child: &str, ancestor: &str) -> bool {
    if child == ancestor {
        return true;
    }
    match child.strip_prefix(ancestor) {
        Some(rest) => rest.starts_with('\\') || rest.starts_with('/'),
        None => false,
    }
}

impl Default for DirSizeService {
    fn default() -> Self {
        Self::new()
    }
}

/// 递归累加目录内所有文件大小；返回 `None` 表示中途被取消（目录已不再被需要）。
fn compute_size(root: &str, cancelled: &dyn Fn() -> bool) -> Option<u64> {
    let path = Path::new(root);
    if !path.is_dir() {
        return Some(0);
    }
    let mut total: u64 = 0;
    let mut seen: usize = 0;
    let walker = WalkDir::new(path)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !(e.file_type().is_dir() && is_reparse_point(e)));
    for entry in walker.filter_map(|e| e.ok()) {
        seen += 1;
        if seen % CANCEL_CHECK_EVERY == 0 && cancelled() {
            return None;
        }
        if entry.file_type().is_file() {
            if let Ok(meta) = entry.metadata() {
                total = total.saturating_add(meta.len());
            }
        }
    }
    Some(total)
}

/// 是否为 Windows 重解析点（junction / symlink）；非 Windows 恒 `false`。
fn is_reparse_point(entry: &walkdir::DirEntry) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        return entry
            .metadata()
            .map(|m| m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
            .unwrap_or(false);
    }
    #[cfg(not(windows))]
    {
        let _ = entry;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn maintenance_removes_unvisited_expired_entries() {
        let now = Instant::now();
        let mut cache = SizeCache::new(2);
        cache.insert("old".into(), 10, now);
        cache.insert("fresh".into(), 20, now + Duration::from_secs(1));
        cache.maintain(now + CACHE_TTL);
        assert!(!cache.entries.contains_key("old"));
        assert_eq!(cache.get("fresh", now + CACHE_TTL), Some(20));
        cache.maintain(now + CACHE_TTL + Duration::from_secs(1));
        assert!(cache.entries.is_empty());
    }

    #[test]
    fn capacity_evicts_least_recently_used_without_extending_ttl() {
        let now = Instant::now();
        let mut cache = SizeCache::new(2);
        cache.insert("a".into(), 10, now);
        cache.insert("b".into(), 20, now + Duration::from_secs(1));
        assert_eq!(cache.get("a", now + Duration::from_secs(2)), Some(10));
        cache.insert("c".into(), 30, now + Duration::from_secs(3));
        assert_eq!(cache.entries.len(), 2);
        assert_eq!(cache.get("b", now + Duration::from_secs(3)), None);
        assert_eq!(cache.get("a", now + CACHE_TTL), None);
        for i in 4..100 {
            cache.insert(format!("path-{i}"), i, now + Duration::from_secs(i));
            assert!(cache.entries.len() <= 2);
        }
    }

    #[test]
    fn insertion_reclaims_expired_entries_before_evicting_live_entries() {
        let now = Instant::now();
        let mut cache = SizeCache::new(2);
        cache.insert("expired".into(), 1, now);
        cache.insert("live".into(), 2, now + Duration::from_secs(1));
        cache.insert("new".into(), 3, now + CACHE_TTL);
        assert!(!cache.entries.contains_key("expired"));
        assert_eq!(cache.get("live", now + CACHE_TTL), Some(2));
        assert_eq!(cache.get("new", now + CACHE_TTL), Some(3));
        let mut disabled = SizeCache::new(0);
        disabled.insert("ignored".into(), 1, now);
        assert!(disabled.entries.is_empty());
    }

    #[test]
    fn invalidation_removes_related_entries_and_preserves_other_paths() {
        let now = Instant::now();
        let mut cache = SizeCache::new(8);
        for path in ["d:\\a", "d:\\a\\b", "d:\\a\\b\\c", "d:\\ab"] {
            cache.insert(path.into(), 1, now);
        }
        cache.invalidate(&["d:\\a\\b".into()], now);
        assert_eq!(cache.entries.len(), 1);
        assert_eq!(cache.get("d:\\ab", now), Some(1));
    }

    #[test]
    fn requests_deduplicate_pending_work_and_recompute_after_expiry_or_eviction() {
        let service = DirSizeService::new();
        let inner = &service.inner;
        *inner.cache.lock() = SizeCache::new(1);
        let now = Instant::now();
        assert!(matches!(
            inner.lookup_or_queue("a", now),
            SizeRequest::Compute
        ));
        assert!(matches!(
            inner.lookup_or_queue("a", now),
            SizeRequest::Pending
        ));
        // 模拟后台遍历完成：先缓存结果，再释放在途标记。
        inner.cache.lock().insert("a".into(), 10, now);
        inner.pending.lock().remove("a");
        assert!(matches!(
            inner.lookup_or_queue("a", now),
            SizeRequest::Cached(10)
        ));
        inner
            .cache
            .lock()
            .insert("b".into(), 20, now + Duration::from_secs(1));
        assert!(matches!(
            inner.lookup_or_queue("a", now + Duration::from_secs(2)),
            SizeRequest::Compute
        ));
        assert!(matches!(
            inner.lookup_or_queue("b", now + CACHE_TTL + Duration::from_secs(1)),
            SizeRequest::Compute
        ));
    }

    #[test]
    fn compute_size_sums_files_recursively() {
        let dir = std::env::temp_dir().join(format!("stelix-dir-size-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("sub/deep")).unwrap();
        fs::write(dir.join("a.txt"), vec![0u8; 100]).unwrap();
        fs::write(dir.join("sub/b.txt"), vec![0u8; 200]).unwrap();
        fs::write(dir.join("sub/deep/c.txt"), vec![0u8; 300]).unwrap();

        let size = compute_size(dir.to_string_lossy().as_ref(), &|| false);
        assert_eq!(size, Some(600));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn compute_size_can_be_cancelled() {
        let dir =
            std::env::temp_dir().join(format!("stelix-dir-size-cancel-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for i in 0..CANCEL_CHECK_EVERY + 8 {
            fs::write(dir.join(format!("f{i}.txt")), b"x").unwrap();
        }

        let size = compute_size(dir.to_string_lossy().as_ref(), &|| true);
        assert_eq!(size, None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn compute_size_of_non_directory_is_zero() {
        let file =
            std::env::temp_dir().join(format!("stelix-dir-size-file-{}", std::process::id()));
        fs::write(&file, b"hello").unwrap();
        assert_eq!(
            compute_size(file.to_string_lossy().as_ref(), &|| false),
            Some(0)
        );
        let _ = fs::remove_file(&file);
    }

    #[test]
    fn path_relation_covers_self_ancestors_and_descendants() {
        assert!(is_related("d:\\a", "d:\\a"));
        assert!(is_related("d:\\a\\b", "d:\\a")); // 后代
        assert!(is_related("d:\\a", "d:\\a\\b")); // 祖先
        assert!(is_related("D:\\A\\B", "d:\\a")); // 大小写不敏感
        assert!(!is_related("d:\\ab", "d:\\a")); // 前缀但非路径边界
        assert!(!is_related("d:\\a", "e:\\a"));
        assert!(!is_related("", "d:\\a"));
    }
}

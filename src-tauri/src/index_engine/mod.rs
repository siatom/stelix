//! 索引引擎 —— 由 **WS-A (feat/index-core)** 实现。
//!
//! 契约（Phase 0 冻结）：
//! - `IndexStore::new()` / `start_scan` / `status` / `search` / `snapshot`
//! - 扫描期间通过 `app.emit(events::INDEX_PROGRESS, IndexProgress)` 上报进度，
//!   完成后 emit `events::INDEX_DONE`（payload = `IndexStatus`）。
//!
//! # 索引数据源：目录遍历
//!
//! 索引器通过 [`IndexSource`] trait 抽象数据源，当前唯一实现是 [`WalkDirSource`]
//! （walkdir 遍历 + rayon 并行取元数据）。索引只在启动与手动重扫时重建，
//! 运行期不再监听文件系统变更 —— 搜索结果的时效性由重新扫描保证。
//! 所有写索引的动作都发生在 `on_batch` 回调里，替换数据源即可复用
//! 增量更新、进度节流、状态维护等全部逻辑。

use std::cmp::Ordering as CmpOrdering;
use std::collections::{BinaryHeap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use dashmap::DashMap;
use parking_lot::{Mutex, RwLock};
use rayon::prelude::*;
use tauri::{AppHandle, Emitter};
use walkdir::WalkDir;

use crate::app_core::events;
use crate::search_engine;
use crate::types::{FileEntry, IndexProgress, IndexStatus, SearchHit, SearchQuery};

/// 每批写入 DashMap 的条目数
const BATCH_SIZE: usize = 2000;
/// 进度上报：距上次上报超过该毫秒数则上报
const PROGRESS_INTERVAL_MS: u128 = 500;
/// 进度上报：距上次上报新增该条目数则上报
const PROGRESS_INTERVAL_ITEMS: usize = 5000;

/// 索引数据源抽象：当前实现为目录遍历（[`WalkDirSource`]）。
pub trait IndexSource: Send + Sync {
    /// 扫描给定根，产出条目（可分批回调）
    fn scan(&self, roots: &[String], on_batch: &mut dyn FnMut(Vec<FileEntry>));
}

/// 目录遍历数据源（walkdir + rayon 并行取元数据）。
pub struct WalkDirSource {
    /// 每批产出的条目数
    pub batch_size: usize,
}

impl WalkDirSource {
    pub fn new(batch_size: usize) -> Self {
        Self {
            batch_size: batch_size.max(1),
        }
    }
}

impl Default for WalkDirSource {
    fn default() -> Self {
        Self::new(BATCH_SIZE)
    }
}

/// 索引时跳过的目录名（大小写不敏感）：系统目录与缓存 / 构建产物。
///
/// 这些目录往往占了文件总数的绝大多数，却极少需要被搜索。跳过它们能让默认
/// 索引量从百万级降到十万级 —— 内存与扫描时间同步下降。
const SKIPPED_DIR_NAMES: &[&str] = &[
    "appdata",
    "node_modules",
    ".git",
    "target",
    "dist",
    ".next",
    ".nuxt",
    ".turbo",
    ".cache",
    ".pnpm-store",
    ".pnpm",
    ".cargo",
    ".rustup",
    ".nuget",
    ".gradle",
    ".m2",
    ".venv",
    "venv",
    "__pycache__",
    "windows",
    "programdata",
    "$recycle.bin",
    "system volume information",
    "recovery",
    "perflogs",
];

/// macOS 专属跳过项：用户库缓存 / 废纸篓 / 系统索引目录。
#[cfg(target_os = "macos")]
const MACOS_SKIPPED_DIR_NAMES: &[&str] = &[
    "library",
    ".trash",
    ".spotlight-v100",
    ".fseventsd",
    ".documentrevisions-v100",
];

#[cfg(target_os = "macos")]
fn platform_skip_dir(name: &str) -> bool {
    MACOS_SKIPPED_DIR_NAMES.contains(&name)
}

#[cfg(not(target_os = "macos"))]
fn platform_skip_dir(_name: &str) -> bool {
    false
}

/// 是否跳过该目录：只按目录名判断（同名文件不受影响），根目录本身永不跳过。
fn skip_dir(entry: &walkdir::DirEntry) -> bool {
    if entry.depth() == 0 || !entry.file_type().is_dir() {
        return false;
    }
    let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
    SKIPPED_DIR_NAMES.contains(&name.as_str()) || platform_skip_dir(&name)
}

impl IndexSource for WalkDirSource {
    fn scan(&self, roots: &[String], on_batch: &mut dyn FnMut(Vec<FileEntry>)) {
        for root in roots {
            // min_depth(1)：跳过根目录自身；错误（如无权限）直接忽略，不 panic。
            let mut buf: Vec<PathBuf> = Vec::with_capacity(self.batch_size);
            let walker = WalkDir::new(root)
                .min_depth(1)
                .into_iter()
                .filter_entry(|entry| !skip_dir(entry));
            for item in walker {
                match item {
                    Ok(e) => {
                        buf.push(e.into_path());
                        if buf.len() >= self.batch_size {
                            let entries = build_entries(&buf);
                            if !entries.is_empty() {
                                on_batch(entries);
                            }
                            buf.clear();
                        }
                    }
                    Err(_) => continue,
                }
            }
            if !buf.is_empty() {
                let entries = build_entries(&buf);
                if !entries.is_empty() {
                    on_batch(entries);
                }
            }
        }
    }
}

/// 用 rayon 并行读取一批路径的元数据并构造条目；读取失败的条目被跳过。
fn build_entries(paths: &[PathBuf]) -> Vec<FileEntry> {
    paths
        .par_iter()
        .filter_map(|p| match std::fs::symlink_metadata(p) {
            Ok(meta) => Some(crate::fs_core::to_entry(p, &meta)),
            Err(_) => None,
        })
        .collect()
}

/// 内存索引存储
pub struct IndexStore {
    /// 路径 -> 条目（内部用 Arc 便于 move 进后台线程）
    entries: Arc<DashMap<String, FileEntry>>,
    status: Arc<RwLock<IndexStatus>>,
    /// 已连接的 Everything IPC 客户端。未运行或 IPC 不可用时保持为 None，
    /// 下次检索会再次尝试连接，避免把临时故障永久缓存。
    #[cfg(windows)]
    everything_client: Mutex<Option<everything_ipc::wm::EverythingClient>>,
}

impl IndexStore {
    pub fn new() -> Self {
        Self {
            entries: Arc::new(DashMap::new()),
            status: Arc::new(RwLock::new(IndexStatus::default())),
            #[cfg(windows)]
            everything_client: Mutex::new(None),
        }
    }

    /// 更新备用内存索引的根目录，但不启动扫描。
    pub fn set_roots(&self, roots: Vec<String>) {
        self.status.write().roots = roots;
    }

    /// 是否能连接到本机运行中的 Everything IPC。
    ///
    /// 连接成功时，搜索直接使用 Everything 的实时索引，无需再扫描配置目录。
    #[cfg(windows)]
    pub fn everything_available(&self) -> bool {
        let mut client = self.everything_client.lock();
        if client.is_none() {
            *client = everything_ipc::wm::EverythingClient::new().ok();
        }
        client.is_some()
    }

    /// 等待 Everything 在系统启动后完成初始化；超时后仅更新状态，不自动扫描磁盘。
    #[cfg(windows)]
    pub fn wait_for_everything(self: Arc<Self>, app: AppHandle, roots: Vec<String>) {
        {
            let mut status = self.status.write();
            if status.scanning || status.waiting_for_everything {
                return;
            }
            status.roots = roots;
            status.scanning = false;
            status.scanned = 0;
            status.using_everything = false;
            status.waiting_for_everything = true;
        }
        std::thread::spawn(move || {
            for _ in 0..30 {
                if self.everything_available() {
                    let status = {
                        let mut status = self.status.write();
                        status.using_everything = true;
                        status.waiting_for_everything = false;
                        status.clone()
                    };
                    let _ = app.emit(events::INDEX_DONE, status);
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            let status = {
                let mut status = self.status.write();
                status.waiting_for_everything = false;
                status.clone()
            };
            let _ = app.emit(events::INDEX_DONE, status);
        });
    }

    /// 启动后台扫描（非阻塞）。用 `rayon + walkdir` 并行遍历，分批写入 DashMap。
    ///
    /// - 增量：不清空已有索引，仅更新；扫描结束后按 root 清理本轮未再出现的旧条目。
    /// - 节流：每 500ms 或每 5000 条 emit 一次 `index:progress`。
    /// - 结束：emit `index:done`（payload = `IndexStatus`）。
    ///
    /// 实际扫描逻辑与 [`IndexStore::scan_blocking`] 共用 [`scan_core`]，避免两份实现漂移。
    pub fn start_scan(&self, app: AppHandle, roots: Vec<String>) {
        // 已在扫描则忽略重复触发，避免两个后台线程互相清理条目。
        if self.status.read().scanning {
            return;
        }

        {
            let mut st = self.status.write();
            st.roots = roots.clone();
            st.scanning = true;
            st.scanned = 0;
            st.using_everything = false;
            st.waiting_for_everything = false;
        }

        let _ = app.emit(
            events::INDEX_PROGRESS,
            IndexProgress {
                scanned: 0,
                total: self.entries.len(),
                current: String::new(),
                done: false,
            },
        );

        let entries = Arc::clone(&self.entries);
        let status = Arc::clone(&self.status);

        std::thread::spawn(move || {
            // 进度节流状态（仅本线程持有）
            let mut last_emit = Instant::now();
            let mut last_count = 0usize;

            let total_scanned = scan_core(&entries, &roots, |done_count, current| {
                let should_emit = last_emit.elapsed().as_millis() >= PROGRESS_INTERVAL_MS
                    || done_count - last_count >= PROGRESS_INTERVAL_ITEMS;
                if should_emit {
                    status.write().scanned = done_count;
                    let _ = app.emit(
                        events::INDEX_PROGRESS,
                        IndexProgress {
                            scanned: done_count,
                            total: entries.len(),
                            current: current.to_string(),
                            done: false,
                        },
                    );
                    last_emit = Instant::now();
                    last_count = done_count;
                }
            });

            let final_status = {
                let mut st = status.write();
                st.scanning = false;
                st.scanned = total_scanned;
                st.total = entries.len();
                IndexStatus {
                    total: entries.len(),
                    ..st.clone()
                }
            };
            let _ = app.emit(events::INDEX_DONE, final_status);
        });
    }

    /// 同步扫描：把 roots 下的条目写入索引，返回本次写入的条目数。
    ///
    /// 不依赖 `AppHandle`，供测试与 `start_scan` 内部复用；
    /// 与 `start_scan` 走同一套过滤/写入/prune 逻辑（[`scan_core`]）。
    ///
    /// 生产路径目前只走 `start_scan`（异步 + 进度事件），本方法主要服务于
    /// 端到端测试（见 `end_to_end_index_and_search`），并作为 Phase 2
    /// 命令行/预索引等同步场景的入口保留。
    #[allow(dead_code)]
    pub fn scan_blocking(&self, roots: &[String]) -> usize {
        scan_core(&self.entries, roots, |_, _| {})
    }

    pub fn status(&self) -> IndexStatus {
        let st = self.status.read();
        IndexStatus {
            total: self.entries.len(),
            ..st.clone()
        }
    }

    /// 检索内存索引：过滤 + 打分 + 排序 + 截断到 `query.limit`。
    ///
    /// 直接并行遍历内存条目，不维护 n-gram 倒排表 —— 几十万条目下单次遍历
    /// 只需几十毫秒，省下的倒排内存远比那点延迟值钱。
    pub fn search(&self, query: &SearchQuery) -> Vec<SearchHit> {
        self.search_with_events(None, query)
    }

    /// 只走 Everything 索引检索；未运行 / IPC 不可用时返回 `None`。
    ///
    /// 与 `search_with_events` 的区别：这里**不会**回退到 Stelix 自己的内存索引。
    /// 窗格筛选需要「结果完整」而不是「有结果就行」——内存索引只覆盖配置的根目录，
    /// 拿它当任意文件夹的筛选数据源会静默漏掉文件。
    #[cfg(windows)]
    pub fn search_everything(&self, query: &SearchQuery) -> Option<Vec<SearchHit>> {
        match everything_search(&self.everything_client, query) {
            EverythingSearch::Hits(hits) => Some(hits),
            EverythingSearch::Unavailable => {
                self.mark_everything_unavailable(None);
                None
            }
        }
    }

    #[cfg(not(windows))]
    pub fn search_everything(&self, _query: &SearchQuery) -> Option<Vec<SearchHit>> {
        None
    }

    /// 检索索引；调用方提供 AppHandle 时，Everything 断开会同步更新前端状态。
    #[cfg_attr(not(windows), allow(unused_variables))]
    pub fn search_with_events(&self, app: Option<&AppHandle>, query: &SearchQuery) -> Vec<SearchHit> {
        #[cfg(windows)]
        match everything_search(&self.everything_client, query) {
            EverythingSearch::Hits(hits) => return hits,
            EverythingSearch::Unavailable => self.mark_everything_unavailable(app),
        }
        let search_started = Instant::now();
        let limit = if query.limit == 0 {
            search_engine::DEFAULT_LIMIT
        } else {
            query.limit
        };
        if limit == 0 {
            return Vec::new();
        }
        let terms: Vec<String> = query
            .text
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        let normalized_query = query.text.trim().to_lowercase();
        let candidates = self.entries.len();

        // 每个线程各自维护一个 top-K 堆，最后归并，全程无共享锁。
        let best = self
            .entries
            .par_iter()
            .fold(
                || BinaryHeap::with_capacity(limit.saturating_add(1)),
                |mut best, item| {
                    let entry = item.value();
                    let name_lower = entry.name.to_lowercase();
                    if !search_engine::matches_with_terms(entry, query, &name_lower, &terms) {
                        return best;
                    }
                    let score = search_engine::score_with_normalized_query(
                        query,
                        &name_lower,
                        &normalized_query,
                    );
                    // 堆已满时先用 (score, name) 与当前最差命中比较：不够好就直接跳过。
                    // 宽泛查询（几十万命中、limit 500）若先克隆 FileEntry 再比较，会为每条命中
                    // 分配 path/name/ext 三份字符串，产生大量无谓的堆分配。
                    push_ranked(&mut best, limit, score, &name_lower, entry);
                    best
                },
            )
            .reduce(
                || BinaryHeap::with_capacity(limit.saturating_add(1)),
                |mut left, right| {
                    for ranked in right {
                        push_ranked(
                            &mut left,
                            limit,
                            ranked.hit.score,
                            &ranked.name_lower,
                            &ranked.hit.entry,
                        );
                    }
                    left
                },
            );

        let mut hits: Vec<SearchHit> = best.into_iter().map(|ranked| ranked.hit).collect();
        hits.sort_by(|a, b| {
            b.score.cmp(&a.score).then_with(|| {
                a.entry
                    .name
                    .to_lowercase()
                    .cmp(&b.entry.name.to_lowercase())
            })
        });
        let elapsed = search_started.elapsed();
        if elapsed.as_millis() >= 25 {
            eprintln!(
                "[search-perf] candidates={} results={} elapsed_ms={}",
                candidates,
                hits.len(),
                elapsed.as_millis()
            );
        }
        hits
    }

    #[cfg(windows)]
    fn mark_everything_unavailable(&self, app: Option<&AppHandle>) {
        let status = {
            let mut status = self.status.write();
            if !status.using_everything {
                return;
            }
            status.using_everything = false;
            status.waiting_for_everything = false;
            status.clone()
        };
        if let Some(app) = app {
            let _ = app.emit(events::INDEX_DONE, status);
        }
    }

    /// 供 AI 工具调用复用的内部条目快照
    pub fn snapshot(&self) -> Arc<Vec<FileEntry>> {
        Arc::new(self.entries.iter().map(|e| e.value().clone()).collect())
    }
}

/// 通过 Everything 官方 Windows Message IPC 查询本机索引。
///
/// Everything 不要求安装在固定路径，也不依赖 SDK DLL；客户端会查找正在运行的
/// IPC 窗口。服务未运行、版本不支持或 IPC 暂不可用时返回 None，由调用方回退。
#[cfg(windows)]
enum EverythingSearch {
    Hits(Vec<SearchHit>),
    Unavailable,
}

#[cfg(windows)]
fn everything_search(
    client: &Mutex<Option<everything_ipc::wm::EverythingClient>>,
    query: &SearchQuery,
) -> EverythingSearch {
    use everything_ipc::wm::{EverythingClient, RequestFlags};

    let mut client = client.lock();
    if client.is_none() {
        *client = EverythingClient::new().ok();
    }
    let request_flags = RequestFlags::FileName
        | RequestFlags::Path
        | RequestFlags::Size
        | RequestFlags::DateModified
        | RequestFlags::Attributes;
    // 把结构化条件尽量下推给 Everything：路径范围、类型、扩展名、大小与修改时间
    // 都能在索引内部完成过滤，避免先取回海量候选再在本地丢弃。
    // 文件名关键词仍然逐个加引号，确保按字面量匹配（不做 Everything 语法解析）。
    let search = everything_query_string(query);
    let result = {
        let Some(client) = client.as_ref() else {
            return EverythingSearch::Unavailable;
        };
        client
            .query_wait(&search)
            .request_flags(request_flags)
            .max_results(query.limit.max(1) as u32)
            .call()
    };
    let list = match result {
        Ok(list) => list,
        Err(_) => {
            *client = None;
            return EverythingSearch::Unavailable;
        }
    };

    let mut hits = list
        .iter()
        .filter_map(|item| everything_item_to_hit(item, query))
        .collect::<Vec<_>>();
    hits.sort_by(|a, b| {
        b.score.cmp(&a.score).then_with(|| {
            a.entry
                .name
                .to_lowercase()
                .cmp(&b.entry.name.to_lowercase())
        })
    });
    EverythingSearch::Hits(hits)
}

#[cfg(windows)]
fn everything_query_string(query: &SearchQuery) -> String {
    let mut parts: Vec<String> = Vec::new();

    // 路径范围：补尾部分隔符，让 `path:` 只命中该目录及其子目录。
    if let Some(scope) = query.scope.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let mut path = scope.replace('/', "\\");
        if !path.ends_with('\\') {
            path.push('\\');
        }
        parts.push(format!("path:\"{path}\""));
    }

    match query.is_dir {
        Some(true) => parts.push("folder:".into()),
        Some(false) => parts.push("file:".into()),
        None => {}
    }

    let exts: Vec<String> = query
        .ext
        .iter()
        .map(|ext| ext.trim().trim_start_matches('.').to_lowercase())
        .filter(|ext| !ext.is_empty() && ext.chars().all(|c| c.is_ascii_alphanumeric()))
        .collect();
    if !exts.is_empty() {
        parts.push(format!("ext:{}", exts.join(";")));
    }

    if let Some(min) = query.min_size {
        parts.push(format!("size:>={min}"));
    }
    if let Some(max) = query.max_size {
        parts.push(format!("size:<={max}"));
    }
    if let Some(after) = query.modified_after {
        if let Some(date) = everything_date(after) {
            parts.push(format!("dm:>={date}"));
        }
    }

    // 路径范围存在（窗格筛选）时关键词按字面量匹配：用户输入 `ext:pdf` 时也应搜索
    // 这个字符串本身，而不是被 Everything 当成语法。Windows 文件名不允许双引号，
    // 直接剔除即可。全局搜索保持原先的原始输入，继续支持 Everything 通配符。
    if query.scope.is_some() {
        for term in query.text.split_whitespace() {
            let cleaned = term.replace('"', "");
            if !cleaned.is_empty() {
                parts.push(format!("\"{cleaned}\""));
            }
        }
    } else if !query.text.trim().is_empty() {
        parts.push(query.text.trim().to_string());
    }

    parts.join(" ")
}

#[cfg(windows)]
fn everything_date(unix_millis: i64) -> Option<String> {
    let utc = chrono::DateTime::from_timestamp_millis(unix_millis)?;
    Some(
        utc.with_timezone(&chrono::Local)
            .format("%Y-%m-%d")
            .to_string(),
    )
}

#[cfg(windows)]
fn everything_item_to_hit(
    item: everything_ipc::wm::QueryItem<'_>,
    query: &SearchQuery,
) -> Option<SearchHit> {
    use everything_ipc::wm::RequestFlags;

    let name = item.get_string(RequestFlags::FileName)?;
    let dir = item.get_string(RequestFlags::Path)?;
    let path = if dir.ends_with(['\\', '/']) {
        format!("{dir}{name}")
    } else {
        format!("{dir}\\{name}")
    };
    let attrs = item.get_u32(RequestFlags::Attributes).unwrap_or_default();
    let is_dir = attrs & 0x10 != 0;
    let modified = item
        .get_time(RequestFlags::DateModified)
        .map(filetime_to_unix_millis)
        .unwrap_or_default();
    let entry = FileEntry {
        id: crate::fs_core::path_id(&path),
        ext: (!is_dir).then_some(&name).and_then(|name| {
            std::path::Path::new(name)
                .extension()
                .map(|ext| ext.to_string_lossy().to_lowercase())
        }),
        name,
        path,
        is_dir,
        size: if is_dir {
            0
        } else {
            item.get_size(RequestFlags::Size).unwrap_or_default()
        },
        modified,
        hidden: attrs & 0x2 != 0,
        system: attrs & 0x4 != 0,
    };
    search_engine::matches(&entry, query).then(|| SearchHit {
        score: search_engine::score(&entry, query),
        entry,
    })
}

/// Windows FILETIME（1601 起的 100ns）转 Unix 毫秒。
#[cfg(windows)]
fn filetime_to_unix_millis(filetime: windows::Win32::Foundation::FILETIME) -> i64 {
    const WINDOWS_TO_UNIX_EPOCH_100NS: u64 = 116_444_736_000_000_000;
    let ticks = (u64::from(filetime.dwHighDateTime) << 32) | u64::from(filetime.dwLowDateTime);
    ticks
        .saturating_sub(WINDOWS_TO_UNIX_EPOCH_100NS)
        .saturating_div(10_000) as i64
}

/// 把一条命中塞进 top-K 堆：堆未满直接进，满了先与最差命中比 (score, name)。
fn push_ranked(
    best: &mut BinaryHeap<RankedHit>,
    limit: usize,
    score: i32,
    name_lower: &str,
    entry: &FileEntry,
) {
    if best.len() >= limit {
        let better = best.peek().is_some_and(|worst| {
            score > worst.hit.score
                || (score == worst.hit.score && name_lower < worst.name_lower.as_str())
        });
        if !better {
            return;
        }
        best.pop();
    }
    best.push(RankedHit {
        hit: SearchHit {
            score,
            entry: entry.clone(),
        },
        name_lower: name_lower.to_string(),
    });
}
/// BinaryHeap keeps the worst currently selected hit at its root.
struct RankedHit {
    hit: SearchHit,
    name_lower: String,
}

impl PartialEq for RankedHit {
    fn eq(&self, other: &Self) -> bool {
        self.hit.score == other.hit.score && self.name_lower == other.name_lower
    }
}
impl Eq for RankedHit {}
impl PartialOrd for RankedHit {
    fn partial_cmp(&self, other: &Self) -> Option<CmpOrdering> {
        Some(self.cmp(other))
    }
}
impl Ord for RankedHit {
    fn cmp(&self, other: &Self) -> CmpOrdering {
        other
            .hit
            .score
            .cmp(&self.hit.score)
            .then_with(|| self.name_lower.cmp(&other.name_lower))
    }
}

/// 扫描核心：遍历 roots，分批写入索引，结束后做增量 prune。
///
/// - 每写入一批调用一次 `on_progress(已处理条目数, 当前路径)`（供 start_scan 节流上报）。
/// - 返回本次写入的条目总数。
///
/// `start_scan`（后台线程 + 事件上报）与 `scan_blocking`（同步、无 AppHandle）共用此函数，
/// 保证两条路径的过滤/写入/prune 行为完全一致。
fn scan_core(
    entries: &DashMap<String, FileEntry>,
    roots: &[String],
    mut on_progress: impl FnMut(usize, &str),
) -> usize {
    let source: Box<dyn IndexSource> = Box::new(WalkDirSource::default());
    let seen: Mutex<HashSet<u64>> = Mutex::new(HashSet::new());
    let mut count: usize = 0;
    let scan_started = Instant::now();
    let mut write_time = std::time::Duration::ZERO;

    {
        let mut on_batch = |batch: Vec<FileEntry>| {
            let current = batch.last().map(|e| e.path.clone()).unwrap_or_default();
            let n = batch.len();

            {
                let write_started = Instant::now();
                let mut seen = seen.lock();
                for e in &batch {
                    seen.insert(e.id);
                }
                drop(seen);
                for e in batch {
                    entries.insert(e.path.clone(), e);
                }
                write_time += write_started.elapsed();
            }

            count += n;
            on_progress(count, &current);
        };
        source.scan(roots, &mut on_batch);
    }

    // 增量清理：删除位于 root 之下、但本轮未再出现的旧条目（已删除/改名的文件）
    let prune_started = Instant::now();
    let seen = seen.into_inner();
    prune_stale(entries, roots, &seen);
    eprintln!(
        "[index-perf] entries={} scan_total_ms={} write_ms={} prune_ms={}",
        count,
        scan_started.elapsed().as_millis(),
        write_time.as_millis(),
        prune_started.elapsed().as_millis()
    );

    count
}

/// 删除位于 roots 之下、但不在 `seen` 中的旧条目（增量更新）。
fn prune_stale(entries: &DashMap<String, FileEntry>, roots: &[String], seen: &HashSet<u64>) {
    let lowers: Vec<String> = roots
        .iter()
        .map(|r| r.trim_end_matches(['\\', '/']).to_string())
        .collect();

    entries.retain(|path, entry| {
        let path = path.trim_end_matches(['\\', '/']);
        let under_root = lowers.iter().any(|root| {
            !root.is_empty()
                && path.len() >= root.len()
                && path
                    .get(..root.len())
                    .is_some_and(|head| head.eq_ignore_ascii_case(root))
                && (path.len() == root.len()
                    || path
                        .as_bytes()
                        .get(root.len())
                        .is_some_and(|c| *c == b'\\' || *c == b'/'))
        });
        if under_root {
            seen.contains(&entry.id)
        } else {
            true
        }
    });
}

impl Default for IndexStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn walk_dir_source_collects_files_and_dirs() {
        let base = std::env::temp_dir().join(format!("stelix-idx-test-{}", std::process::id()));
        let sub = base.join("sub");
        fs::create_dir_all(&sub).unwrap();
        fs::write(base.join("a.txt"), b"hello").unwrap();
        fs::write(sub.join("b.pdf"), b"world").unwrap();

        let source = WalkDirSource::new(1);
        let mut collected: Vec<FileEntry> = Vec::new();
        source.scan(&[base.to_string_lossy().to_string()], &mut |batch| {
            collected.extend(batch);
        });

        let names: Vec<String> = collected.iter().map(|e| e.name.clone()).collect();
        assert!(names.contains(&"a.txt".to_string()));
        assert!(names.contains(&"b.pdf".to_string()));
        assert!(names.contains(&"sub".to_string()));
        assert!(collected.iter().any(|e| e.is_dir && e.name == "sub"));

        fs::remove_dir_all(&base).ok();
    }

    /// 端到端：真实扫描填充 IndexStore 后，用 search 能查到 —— 覆盖"索引 → 检索"完整链路。
    #[test]
    fn end_to_end_index_and_search() {
        let base = std::env::temp_dir().join(format!(
            "stelix-e2e-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let sub = base.join("sub");
        fs::create_dir_all(&sub).unwrap();
        fs::write(base.join("报告.pdf"), b"small pdf").unwrap();
        fs::write(base.join("大文件.bin"), vec![0u8; 3 * 1024 * 1024]).unwrap();
        fs::write(base.join("notes.txt"), b"notes").unwrap();
        fs::write(sub.join("图片.png"), b"png").unwrap();

        let store = IndexStore::new();
        let written = store.scan_blocking(&[base.to_string_lossy().to_string()]);

        // 递归生效：报告.pdf / 大文件.bin / notes.txt / sub / sub/图片.png 共 5 条
        assert_eq!(written, 5, "scan_blocking 应写入 5 条");
        assert_eq!(
            store.status().total,
            5,
            "status().total 应覆盖全部条目（含 sub 与 图片.png）"
        );

        // ext:pdf 恰好命中 报告.pdf
        let hits = store.search(&search_engine::parse_query("ext:pdf"));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].entry.name, "报告.pdf");

        // size:>1mb 命中 大文件.bin
        let hits = store.search(&search_engine::parse_query("size:>1mb"));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].entry.name, "大文件.bin");

        // 中文文件名子串命中 sub/图片.png
        let hits = store.search(&search_engine::parse_query("图片"));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].entry.name, "图片.png");
        assert!(
            hits[0].entry.path.ends_with("sub\\图片.png")
                || hits[0].entry.path.ends_with("sub/图片.png"),
            "应命中子目录中的 图片.png，实际路径：{}",
            hits[0].entry.path
        );

        // 空查询不应过滤掉所有条目
        assert!(
            !store.search(&SearchQuery::default()).is_empty(),
            "空查询应返回非空结果"
        );

        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn search_filters_and_sorts() {
        let store = IndexStore::new();
        store.entries.insert(
            "C:\\x\\report.pdf".into(),
            FileEntry {
                id: 1,
                name: "report.pdf".into(),
                path: "C:\\x\\report.pdf".into(),
                is_dir: false,
                size: 10,
                modified: 0,
                ext: Some("pdf".into()),
                hidden: false,
                system: false,
            },
        );
        store.entries.insert(
            "C:\\x\\my-report.docx".into(),
            FileEntry {
                id: 2,
                name: "my-report.docx".into(),
                path: "C:\\x\\my-report.docx".into(),
                is_dir: false,
                size: 20,
                modified: 0,
                ext: Some("docx".into()),
                hidden: false,
                system: false,
            },
        );

        let q = search_engine::parse_query("report ext:pdf");
        let hits = store.search(&q);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].entry.name, "report.pdf");
        assert!(hits[0].score > 0);
    }

    /// 堆满后只保留 limit 条最优命中（同分按名称升序），覆盖「先比较后克隆」的剪枝路径。
    #[test]
    fn search_keeps_only_top_limited_hits() {
        let store = IndexStore::new();
        for (id, name) in ["file.txt", "file-1.txt", "file-2.txt", "my-file.txt"]
            .iter()
            .enumerate()
        {
            let path = format!("C:\\x\\{name}");
            store.entries.insert(
                path.clone(),
                FileEntry {
                    id: id as u64 + 1,
                    name: (*name).into(),
                    path,
                    is_dir: false,
                    size: 1,
                    modified: 0,
                    ext: Some("txt".into()),
                    hidden: false,
                    system: false,
                },
            );
        }

        let mut query = search_engine::parse_query("file");
        query.limit = 2;
        let hits = store.search(&query);
        assert_eq!(hits.len(), 2, "应截断到 limit");
        assert_eq!(hits[0].entry.name, "file.txt", "主干名命中评分最高");
        assert_eq!(hits[1].entry.name, "file-1.txt", "同分按名称升序");
    }

    #[test]
    fn snapshot_returns_all() {
        let store = IndexStore::new();
        store.entries.insert(
            "C:\\x\\a".into(),
            FileEntry {
                id: 1,
                name: "a".into(),
                path: "C:\\x\\a".into(),
                is_dir: false,
                size: 1,
                modified: 0,
                ext: None,
                hidden: false,
                system: false,
            },
        );
        assert_eq!(store.snapshot().len(), 1);
        assert_eq!(store.status().total, 1);
    }

    #[cfg(windows)]
    #[test]
    fn everything_query_pushes_scope_and_filters() {
        let query = SearchQuery {
            text: "report 2024".into(),
            scope: Some(r"C:\Users\Ann".into()),
            is_dir: Some(false),
            ext: vec!["pdf".into()],
            min_size: Some(1024),
            max_size: Some(2048),
            modified_after: Some(0),
            limit: 10,
        };
        let text = everything_query_string(&query);
        assert!(text.contains(r#"path:"C:\Users\Ann\""#), "{text}");
        assert!(text.contains("file:"), "{text}");
        assert!(text.contains("ext:pdf"), "{text}");
        assert!(text.contains("size:>=1024"), "{text}");
        assert!(text.contains("size:<=2048"), "{text}");
        assert!(text.contains("dm:>="), "{text}");
        assert!(text.contains("\"report\""), "{text}");
        assert!(text.contains("\"2024\""), "{text}");
    }

    #[cfg(windows)]
    #[test]
    fn everything_query_treats_keywords_as_literals() {
        let query = SearchQuery {
            text: "ext:pdf \"quoted\"".into(),
            scope: Some(r"C:\Users\Ann".into()),
            ..Default::default()
        };
        let text = everything_query_string(&query);
        assert!(text.contains("\"ext:pdf\""), "{text}");
        assert!(text.contains("\"quoted\""), "{text}");
    }
}

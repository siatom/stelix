//! 索引引擎 —— 由 **WS-A (feat/index-core)** 实现。
//!
//! 契约（Phase 0 冻结）：
//! - `IndexStore::new()` / `start_scan` / `status` / `search` / `snapshot`
//! - 扫描期间通过 `app.emit(events::INDEX_PROGRESS, IndexProgress)` 上报进度，
//!   完成后 emit `events::INDEX_DONE`（payload = `IndexStatus`）。
//!
//! # Phase 2：接入 NTFS MFT 直读
//!
//! 索引器通过 [`IndexSource`] trait 抽象数据源，Phase 1 的目录遍历实现是
//! [`WalkDirSource`]。要接入 MFT：
//! 1. 新增一个类型 `MftSource` 实现 [`IndexSource`]：在 `scan` 内读取
//!    `$MFT`（`windows-sys` 的 `DeviceIoControl` + `FSCTL_ENUM_USN_DATA`），
//!    解析出 `FileEntry` 后按批调用 `on_batch`。**无需改动本文件对外签名。**
//! 2. 在 [`IndexStore::start_scan`] 里把 `WalkDirSource::default()` 换成
//!    `MftSource::new()`（可按卷自动选择：NTFS 走 MFT，其余走 walkdir）。
//! 由于所有写索引的动作都发生在 `on_batch` 回调里，替换数据源即可复用
//! 增量更新、进度节流、状态维护等全部逻辑。

use std::collections::HashSet;
use std::path::PathBuf;
#[cfg(windows)]
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use dashmap::DashMap;
use parking_lot::{Mutex, RwLock};
use rayon::prelude::*;
use tauri::{AppHandle, Emitter};
use walkdir::WalkDir;

use crate::app_core::events;
use crate::persistence::{self, IndexCache, IndexJournal};
use crate::search_engine;
use crate::types::{FileEntry, IndexProgress, IndexStatus, SearchHit, SearchQuery};

#[cfg(windows)]
mod mft;

/// 每批写入 DashMap 的条目数
const BATCH_SIZE: usize = 2000;
/// 进度上报：距上次上报超过该毫秒数则上报
const PROGRESS_INTERVAL_MS: u128 = 500;
/// 进度上报：距上次上报新增该条目数则上报
const PROGRESS_INTERVAL_ITEMS: usize = 5000;

/// 索引数据源抽象：Phase 1 用目录遍历实现，Phase 2 增加 NTFS MFT 实现。
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

impl IndexSource for WalkDirSource {
    fn scan(&self, roots: &[String], on_batch: &mut dyn FnMut(Vec<FileEntry>)) {
        for root in roots {
            // min_depth(1)：跳过根目录自身；错误（如无权限）直接忽略，不 panic。
            let mut buf: Vec<PathBuf> = Vec::with_capacity(self.batch_size);
            for item in WalkDir::new(root).min_depth(1).into_iter() {
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
    #[cfg(windows)]
    service_generation: Arc<AtomicU64>,
}

impl IndexStore {
    pub fn new() -> Self {
        Self {
            entries: Arc::new(DashMap::new()),
            status: Arc::new(RwLock::new(IndexStatus::default())),
            #[cfg(windows)]
            service_generation: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Restore the durable index and replay the NTFS journal before falling back
    /// to a full MFT enumeration.
    pub fn start_service(&self, app: AppHandle, roots: Vec<String>) {
        #[cfg(windows)]
        if let Some(cache) = persistence::load_index_cache() {
            if same_roots(&cache.roots, &roots) {
                self.entries.clear();
                for entry in cache.entries {
                    self.entries.insert(entry.path.clone(), entry);
                }
                {
                    let mut status = self.status.write();
                    status.roots = roots.clone();
                    status.total = self.entries.len();
                    status.scanned = 0;
                    status.scanning = true;
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
                let resumed = mft::resume_journals(&roots, &cache.journals, &self.entries, true);
                if let Some(checkpoints) = resumed {
                    {
                        let mut status = self.status.write();
                        status.roots = roots.clone();
                        status.total = self.entries.len();
                        status.scanned = self.entries.len();
                        status.scanning = false;
                    }
                    persist_index(&roots, &self.entries, &checkpoints);
                    let _ = app.emit(events::INDEX_DONE, self.status());
                    let generation = self.service_generation.fetch_add(1, Ordering::SeqCst) + 1;
                    let entries = Arc::clone(&self.entries);
                    let service_generation = Arc::clone(&self.service_generation);
                    std::thread::spawn(move || {
                        mft::watch_roots(
                            roots,
                            entries,
                            generation,
                            service_generation,
                            checkpoints,
                        );
                    });
                    return;
                }
            }
            self.entries.clear();
        }
        self.start_scan(app, roots);
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

        #[cfg(windows)]
        let generation = self.service_generation.fetch_add(1, Ordering::SeqCst) + 1;

        #[cfg(windows)]
        let initial_journals = mft::capture_journals(&roots);

        {
            let mut st = self.status.write();
            st.roots = roots.clone();
            st.scanning = true;
            st.scanned = 0;
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
        #[cfg(windows)]
        let service_generation = Arc::clone(&self.service_generation);

        std::thread::spawn(move || {
            #[cfg(windows)]
            let mut checkpoints = None;
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

            #[cfg(windows)]
            {
                if let Some(initial) = initial_journals {
                    checkpoints = mft::resume_journals(&roots, &initial, &entries, false);
                }
                if let Some(ref checkpoints) = checkpoints {
                    persist_index(&roots, &entries, checkpoints);
                }
            }

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

            #[cfg(windows)]
            mft::watch_roots(
                roots,
                entries,
                generation,
                service_generation,
                checkpoints.unwrap_or_default(),
            );
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
    pub fn search(&self, query: &SearchQuery) -> Vec<SearchHit> {
        let mut hits: Vec<SearchHit> = self
            .entries
            .iter()
            .filter_map(|e| {
                let entry = e.value();
                if search_engine::matches(entry, query) {
                    Some(SearchHit {
                        entry: entry.clone(),
                        score: search_engine::score(entry, query),
                    })
                } else {
                    None
                }
            })
            .collect();

        // 分数降序；同分按名称不区分大小写升序，保证结果稳定。
        hits.sort_by(|a, b| {
            b.score.cmp(&a.score).then_with(|| {
                a.entry
                    .name
                    .to_lowercase()
                    .cmp(&b.entry.name.to_lowercase())
            })
        });

        let limit = if query.limit == 0 {
            search_engine::DEFAULT_LIMIT
        } else {
            query.limit
        };
        if hits.len() > limit {
            hits.truncate(limit);
        }
        hits
    }

    /// 供 AI 工具调用复用的内部条目快照
    pub fn snapshot(&self) -> Arc<Vec<FileEntry>> {
        Arc::new(self.entries.iter().map(|e| e.value().clone()).collect())
    }
}

#[cfg(windows)]
fn same_roots(left: &[String], right: &[String]) -> bool {
    let normalize = |roots: &[String]| {
        let mut roots: Vec<String> = roots
            .iter()
            .map(|root| root.trim_end_matches(['\\', '/']).to_lowercase())
            .collect();
        roots.sort();
        roots
    };
    normalize(left) == normalize(right)
}

#[cfg(windows)]
fn persist_index(
    roots: &[String],
    entries: &DashMap<String, FileEntry>,
    journals: &[IndexJournal],
) {
    let cache = IndexCache {
        roots: roots.to_vec(),
        entries: entries.iter().map(|entry| entry.value().clone()).collect(),
        journals: journals.to_vec(),
    };
    let _ = persistence::save_index_cache(&cache);
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
    #[cfg(windows)]
    let source: Box<dyn IndexSource> = Box::new(mft::MftOrWalkSource::default());
    #[cfg(not(windows))]
    let source: Box<dyn IndexSource> = Box::new(WalkDirSource::default());
    let seen: Mutex<HashSet<u64>> = Mutex::new(HashSet::new());
    let mut count: usize = 0;

    {
        let mut on_batch = |batch: Vec<FileEntry>| {
            let current = batch.last().map(|e| e.path.clone()).unwrap_or_default();
            let n = batch.len();

            {
                let mut seen = seen.lock();
                for e in &batch {
                    seen.insert(e.id);
                }
            }
            for e in batch {
                entries.insert(e.path.clone(), e);
            }

            count += n;
            on_progress(count, &current);
        };
        source.scan(roots, &mut on_batch);
    }

    // 增量清理：删除位于 root 之下、但本轮未再出现的旧条目（已删除/改名的文件）
    let seen = seen.into_inner();
    prune_stale(entries, roots, &seen);

    count
}

/// 删除位于 roots 之下、但不在 `seen` 中的旧条目（增量更新）。
fn prune_stale(entries: &DashMap<String, FileEntry>, roots: &[String], seen: &HashSet<u64>) {
    let lowers: Vec<String> = roots
        .iter()
        .map(|r| r.trim_end_matches(['\\', '/']).to_lowercase())
        .collect();

    entries.retain(|path, entry| {
        let p = path.to_lowercase();
        let under_root = lowers
            .iter()
            .any(|r| !r.is_empty() && p.starts_with(r.as_str()));
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
        let base = std::env::temp_dir().join(format!("file-povit-idx-test-{}", std::process::id()));
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
            "file-povit-e2e-{}-{}",
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
}

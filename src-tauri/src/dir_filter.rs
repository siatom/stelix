//! 目录内的本地筛选（支持递归）。
//!
//! 有 Everything 索引时优先在索引内完成递归筛选（毫秒级）；没有索引时回退到
//! 实时目录遍历。两条路径复用同一套 `search_engine` 匹配语义，保证结果一致。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use once_cell::sync::Lazy;
use parking_lot::Mutex;
use walkdir::WalkDir;

use crate::fs_core::{io_path, to_entry};
use crate::index_engine::IndexStore;
use crate::search_engine;
use crate::types::{DirFilterResult, SearchQuery};

/// 单次筛选最多返回的命中数。
pub const MAX_FILTER_RESULTS: usize = 5000;
/// 防御性上限：目录树过大时停止扫描并按截断处理，避免长时间占用 IO。
const MAX_SCAN_ENTRIES: usize = 300_000;
/// 取消时的统一错误文案（前端会丢弃过期请求，不展示为失败）。
const CANCELLED: &str = "筛选已取消";
/// 同一标签的新筛选会取消上一次仍在执行的遍历，避免反复输入时堆积后台扫描。
static FILTER_CANCELS: Lazy<Mutex<HashMap<String, Arc<AtomicBool>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// 在 `path` 内按查询条件筛选条目（仅供测试 / 无索引场景直接调用）。
///
/// - `recursive = false`：只看直接子项；
/// - `recursive = true`：遍历整个子树（不跟随 symlink / junction）。
#[cfg(test)]
#[allow(dead_code)]
pub fn filter_dir(
    path: &str,
    query: &SearchQuery,
    recursive: bool,
) -> Result<DirFilterResult, String> {
    filter_dir_walk(path, query, recursive, None)
}

/// 带索引的筛选入口：本地固定磁盘优先查询 Everything，失败时回退目录遍历。
///
/// `cancel_key` 一般传标签 id；同一 key 的新请求会取消旧请求。
pub fn filter_dir_with_index(
    index: &IndexStore,
    path: &str,
    query: &SearchQuery,
    recursive: bool,
    cancel_key: Option<&str>,
) -> Result<DirFilterResult, String> {
    let started = Instant::now();
    let token = begin_filter(cancel_key);
    let mut source = "scan";
    let result = (|| {
        if token.load(Ordering::Relaxed) {
            return Err(CANCELLED.to_string());
        }
        let limit = result_limit(query);
        if recursive && is_local_drive_path(path) {
            // 多取一条用于判断是否被截断；其余条件交给 Everything 在索引内过滤。
            let scoped = SearchQuery {
                scope: Some(path.to_string()),
                limit: limit.saturating_add(1),
                ..query.clone()
            };
            if let Some(mut hits) = index.search_everything(&scoped) {
                if token.load(Ordering::Relaxed) {
                    return Err(CANCELLED.to_string());
                }
                source = "everything";
                let truncated = hits.len() > limit;
                hits.truncate(limit);
                return Ok(DirFilterResult {
                    entries: hits.into_iter().map(|hit| hit.entry).collect(),
                    truncated,
                });
            }
        }
        filter_dir_walk(path, query, recursive, Some(&token))
    })();
    let cancelled = token.load(Ordering::Relaxed);
    finish_filter(cancel_key, &token);
    if cancelled {
        return Err(CANCELLED.to_string());
    }
    let elapsed = started.elapsed();
    if elapsed.as_millis() >= 25 {
        let count = result
            .as_ref()
            .map(|value| value.entries.len())
            .unwrap_or(0);
        eprintln!(
            "[filter-perf] source={source} entries={count} elapsed_ms={}",
            elapsed.as_millis()
        );
    }
    result
}

/// 取消某个标签正在执行的递归筛选。
pub fn cancel_filter(cancel_key: &str) {
    if let Some(token) = FILTER_CANCELS.lock().remove(cancel_key) {
        token.store(true, Ordering::Relaxed);
    }
}

fn result_limit(query: &SearchQuery) -> usize {
    if query.limit == 0 {
        search_engine::DEFAULT_LIMIT
    } else {
        query.limit.min(MAX_FILTER_RESULTS)
    }
}

fn begin_filter(cancel_key: Option<&str>) -> Arc<AtomicBool> {
    let token = Arc::new(AtomicBool::new(false));
    if let Some(key) = cancel_key.filter(|key| !key.is_empty()) {
        if let Some(previous) = FILTER_CANCELS.lock().insert(key.to_string(), token.clone()) {
            previous.store(true, Ordering::Relaxed);
        }
    }
    token
}

fn finish_filter(cancel_key: Option<&str>, token: &Arc<AtomicBool>) {
    if let Some(key) = cancel_key.filter(|key| !key.is_empty()) {
        let mut cancels = FILTER_CANCELS.lock();
        if cancels
            .get(key)
            .is_some_and(|current| Arc::ptr_eq(current, token))
        {
            cancels.remove(key);
        }
    }
}

/// Windows 盘符绝对路径（Everything 只索引本地文件系统，UNC / 远程路径直接遍历）。
fn is_local_drive_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
}

fn filter_dir_walk(
    path: &str,
    query: &SearchQuery,
    recursive: bool,
    cancel: Option<&AtomicBool>,
) -> Result<DirFilterResult, String> {
    let root = PathBuf::from(path);
    let io_root = io_path(&root);
    let meta =
        std::fs::metadata(&io_root).map_err(|error| format!("无法读取目录 {path}: {error}"))?;
    if !meta.is_dir() {
        return Err(format!("不是目录：{path}"));
    }

    let limit = result_limit(query);
    let mut walker = WalkDir::new(&io_root).min_depth(1).follow_links(false);
    if !recursive {
        walker = walker.max_depth(1);
    }

    let mut entries = Vec::new();
    let mut truncated = false;
    let mut scanned = 0usize;
    let terms: Vec<String> = query
        .text
        .split_whitespace()
        .map(str::to_lowercase)
        .collect();
    for item in walker
        .into_iter()
        .filter_entry(|entry| !is_reparse_dir(entry))
    {
        let entry = match item {
            Ok(entry) => entry,
            // 无权限 / 目录被并发删除：跳过该分支，继续扫描其余部分。
            Err(_) => continue,
        };
        scanned += 1;
        if scanned > MAX_SCAN_ENTRIES {
            truncated = true;
            break;
        }
        if scanned % 2048 == 0 {
            if let Some(token) = cancel {
                if token.load(Ordering::Relaxed) {
                    return Err(CANCELLED.to_string());
                }
            }
        }
        // 先用文件名 / 扩展名 / 类型做零成本预筛；只有可能命中时才读取元数据。
        // 关键词筛选在大目录上能因此省掉绝大多数 stat 调用。
        if !passes_prefilter(&entry, query, &terms) {
            continue;
        }
        let meta = match entry.metadata() {
            Ok(meta) => meta,
            Err(_) => continue,
        };
        let rel = entry
            .path()
            .strip_prefix(&io_root)
            .unwrap_or_else(|_| entry.path());
        // 用原始根路径拼回展示路径，避免 `\\?\` 前缀漏到界面。
        let display = root.join(rel);
        let file_entry = to_entry(&display, &meta);
        let name_lower = file_entry.name.to_lowercase();
        if !search_engine::matches_with_terms(&file_entry, query, &name_lower, &terms) {
            continue;
        }
        if entries.len() >= limit {
            truncated = true;
            break;
        }
        entries.push(file_entry);
    }

    Ok(DirFilterResult { entries, truncated })
}

/// 读取元数据前的轻量预筛：只使用 walkdir 已经拿到的文件名 / 扩展名 / 文件类型。
fn passes_prefilter(entry: &walkdir::DirEntry, query: &SearchQuery, terms: &[String]) -> bool {
    let is_dir = entry.file_type().is_dir();
    if let Some(want_dir) = query.is_dir {
        if is_dir != want_dir {
            return false;
        }
    }
    if !query.ext.is_empty() {
        if is_dir {
            return false;
        }
        let name = entry.file_name().to_string_lossy();
        let Some(ext) = Path::new(name.as_ref()).extension() else {
            return false;
        };
        let ext = ext.to_string_lossy();
        if !query.ext.iter().any(|want| want.eq_ignore_ascii_case(&ext)) {
            return false;
        }
    }
    if !terms.is_empty() {
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if !terms.iter().all(|term| name.contains(term)) {
            return false;
        }
    }
    true
}

/// walkdir 的过滤：目录型重解析点（symlink / junction）不下钻，避免循环。
fn is_reparse_dir(entry: &walkdir::DirEntry) -> bool {
    if entry.depth() == 0 || !entry.file_type().is_dir() {
        return false;
    }
    if entry.path_is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        entry
            .metadata()
            .map(|meta| meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
            .unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::FileEntry;
    use std::fs;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("stelix-dir-filter-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn names(entries: &[FileEntry]) -> Vec<String> {
        entries.iter().map(|entry| entry.name.clone()).collect()
    }

    #[test]
    fn recursive_finds_nested_matches() {
        let dir = temp_dir("recursive");
        fs::create_dir_all(dir.join("sub/deep")).unwrap();
        fs::write(dir.join("report.txt"), b"a").unwrap();
        fs::write(dir.join("sub/report.md"), b"b").unwrap();
        fs::write(dir.join("sub/deep/report.pdf"), b"c").unwrap();
        fs::write(dir.join("sub/other.txt"), b"d").unwrap();

        let query = SearchQuery {
            text: "report".into(),
            limit: 100,
            ..Default::default()
        };
        let result = filter_dir(&dir.to_string_lossy(), &query, true).unwrap();
        let mut got = names(&result.entries);
        got.sort();
        assert_eq!(got, vec!["report.md", "report.pdf", "report.txt"]);
        assert!(!result.truncated);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn non_recursive_only_returns_direct_children() {
        let dir = temp_dir("non-recursive");
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("report.txt"), b"a").unwrap();
        fs::write(dir.join("sub/report.md"), b"b").unwrap();

        let query = SearchQuery {
            text: "report".into(),
            limit: 100,
            ..Default::default()
        };
        let result = filter_dir(&dir.to_string_lossy(), &query, false).unwrap();
        assert_eq!(names(&result.entries), vec!["report.txt"]);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn extension_and_size_filters_are_applied() {
        let dir = temp_dir("criteria");
        fs::write(dir.join("small.txt"), b"1").unwrap();
        fs::write(dir.join("large.txt"), vec![b'x'; 2048]).unwrap();
        fs::write(dir.join("large.pdf"), vec![b'y'; 2048]).unwrap();

        let query = SearchQuery {
            ext: vec!["txt".into()],
            min_size: Some(1024),
            limit: 100,
            ..Default::default()
        };
        let result = filter_dir(&dir.to_string_lossy(), &query, false).unwrap();
        assert_eq!(names(&result.entries), vec!["large.txt"]);

        let _ = fs::remove_dir_all(&dir);
    }
}

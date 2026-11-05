//! 目录内的本地筛选（支持递归）。
//!
//! 与全局检索不同，这里直接遍历指定目录，不依赖索引是否覆盖该位置；
//! 条件复用 `search_engine::matches`，保证「筛选」与「搜索」语义一致。

use std::path::PathBuf;

use walkdir::WalkDir;

use crate::fs_core::{io_path, to_entry};
use crate::search_engine;
use crate::types::{DirFilterResult, SearchQuery};

/// 单次筛选最多返回的命中数。
pub const MAX_FILTER_RESULTS: usize = 5000;
/// 防御性上限：目录树过大时停止扫描并按截断处理，避免长时间占用 IO。
const MAX_SCAN_ENTRIES: usize = 300_000;

/// 在 `path` 内按查询条件筛选条目。
///
/// - `recursive = false`：只看直接子项；
/// - `recursive = true`：遍历整个子树（不跟随 symlink / junction）。
pub fn filter_dir(
    path: &str,
    query: &SearchQuery,
    recursive: bool,
) -> Result<DirFilterResult, String> {
    let root = PathBuf::from(path);
    let io_root = io_path(&root);
    let meta =
        std::fs::metadata(&io_root).map_err(|error| format!("无法读取目录 {path}: {error}"))?;
    if !meta.is_dir() {
        return Err(format!("不是目录：{path}"));
    }

    let limit = if query.limit == 0 {
        search_engine::DEFAULT_LIMIT
    } else {
        query.limit.min(MAX_FILTER_RESULTS)
    };

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

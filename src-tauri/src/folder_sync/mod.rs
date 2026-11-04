//! 文件夹同步：两侧目录树并行扫描 → 差异计划 → 并行执行。
//!
//! 设计目标是「业务最快」：
//! - 每侧只遍历一遍目录树，元数据来自目录枚举缓存（不读文件内容）；
//! - 相同项（大小 + 修改时间）直接跳过，只传差异；
//! - 扫描按顶层子树并行（rayon），复制按文件并行；
//! - 复制走 `std::fs::copy`（Windows 底层 CopyFileExW）；
//! - 可选内容校验只在用户显式开启时读取文件。

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

use rayon::prelude::*;
use walkdir::WalkDir;

use crate::fs_core::{self, OpContext};
use crate::types::{
    SyncAction, SyncCompare, SyncDirection, SyncEntry, SyncExecuteRequest, SyncExecItem, SyncMode,
    SyncPlan, SyncRequest, SyncResult, SyncStats,
};

/// 修改时间容差：FAT / 网络盘的时间精度是 2s，避免把「同一文件」误判为不同。
const MTIME_TOLERANCE_MS: i64 = 2000;
/// 内容校验的读块大小。
const HASH_CHUNK: usize = 64 * 1024;

#[derive(Debug, Clone, Copy)]
struct Meta {
    is_dir: bool,
    size: u64,
    mtime: i64,
}

#[derive(Debug, Clone)]
struct TreeEntry {
    /// 原始大小写的相对路径（用于拼真实路径与展示）
    rel: String,
    meta: Meta,
}

/// key = 小写相对路径（Windows 大小写不敏感）。
type Tree = HashMap<String, TreeEntry>;

// ---------------------------------------------------------------- 扫描

/// 扫描两侧目录并生成差异计划。
pub fn scan(req: &SyncRequest) -> Result<SyncPlan, String> {
    let left = PathBuf::from(req.left.trim());
    let right = PathBuf::from(req.right.trim());
    validate_roots(&left, &right)?;

    // 左右两侧同时扫描；每一侧内部再按顶层子树并行。
    let (lt, rt) = rayon::join(|| collect_tree(&left), || collect_tree(&right));
    let left_tree = lt?;
    let right_tree = rt?;
    Ok(build_plan(&left, &right, &left_tree, &right_tree, req))
}

/// 校验左右根目录：都存在、都是目录、不相同、不互相包含。
fn validate_roots(left: &Path, right: &Path) -> Result<(), String> {
    if left.as_os_str().is_empty() || right.as_os_str().is_empty() {
        return Err("左右目录都不能为空".into());
    }
    let l = canonical(left)?;
    let r = canonical(right)?;
    if !l.is_dir() {
        return Err(format!("左侧不是文件夹：{}", left.display()));
    }
    if !r.is_dir() {
        return Err(format!("右侧不是文件夹：{}", right.display()));
    }
    let ls = l.to_string_lossy().to_lowercase();
    let rs = r.to_string_lossy().to_lowercase();
    if ls == rs {
        return Err("左右两侧是同一个文件夹，无法同步".into());
    }
    let sep = std::path::MAIN_SEPARATOR;
    if ls.starts_with(&format!("{rs}{sep}")) || rs.starts_with(&format!("{ls}{sep}")) {
        return Err("两个文件夹存在包含关系，无法同步".into());
    }
    Ok(())
}

fn canonical(p: &Path) -> Result<PathBuf, String> {
    fs::canonicalize(fs_core::io_path(p)).map_err(|error| format!("无法访问 {}: {error}", p.display()))
}

/// 收集一侧目录树（跳过符号链接 / 重解析点）。
fn collect_tree(root: &Path) -> Result<Tree, String> {
    let io_root = fs_core::io_path(root);
    let read = fs::read_dir(&io_root).map_err(|error| format!("无法读取目录 {}: {error}", root.display()))?;

    let mut tree: Tree = Tree::new();
    let mut top_dirs: Vec<(String, PathBuf)> = Vec::new();

    for entry in read.flatten() {
        let file_type = match entry.file_type() {
            Ok(value) => value,
            Err(_) => continue,
        };
        if file_type.is_symlink() {
            continue;
        }
        let meta = match entry.metadata() {
            Ok(value) => value,
            Err(_) => continue,
        };
        if is_reparse_meta(&meta) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if meta.is_dir() {
            tree.insert(
                name.to_lowercase(),
                TreeEntry {
                    rel: name.clone(),
                    meta: meta_of(&meta),
                },
            );
            top_dirs.push((name, entry.path()));
        } else if meta.is_file() {
            tree.insert(
                name.to_lowercase(),
                TreeEntry {
                    rel: name,
                    meta: meta_of(&meta),
                },
            );
        }
    }

    // 顶层子树并行遍历：宽目录（大量平级子目录）能拿到接近线性的加速。
    let subtrees: Vec<Tree> = top_dirs
        .par_iter()
        .map(|(_, path)| walk_subtree(&io_root, path))
        .collect();
    for subtree in subtrees {
        tree.extend(subtree);
    }
    Ok(tree)
}

/// 遍历一棵子树（start 是已带 verbatim 前缀的绝对路径）。
fn walk_subtree(io_root: &Path, start: &Path) -> Tree {
    let mut out: Tree = Tree::new();
    let walker = WalkDir::new(start)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| entry.depth() == 0 || !is_reparse_entry(entry));

    for entry in walker.flatten() {
        if entry.depth() == 0 {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(io_root) else {
            continue;
        };
        let rel = rel.to_string_lossy().into_owned();
        let Ok(meta) = entry.metadata() else { continue };
        if is_reparse_meta(&meta) {
            continue;
        }
        out.insert(
            rel.to_lowercase(),
            TreeEntry {
                rel,
                meta: meta_of(&meta),
            },
        );
    }
    out
}

fn meta_of(meta: &fs::Metadata) -> Meta {
    let mtime = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0);
    Meta {
        is_dir: meta.is_dir(),
        size: if meta.is_dir() { 0 } else { meta.len() },
        mtime,
    }
}

fn is_reparse_meta(meta: &fs::Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn is_reparse_entry(entry: &walkdir::DirEntry) -> bool {
    if entry.path_is_symlink() {
        return true;
    }
    entry.metadata().map(|meta| is_reparse_meta(&meta)).unwrap_or(false)
}

// ---------------------------------------------------------------- 差异计划

fn build_plan(
    left: &Path,
    right: &Path,
    left_tree: &Tree,
    right_tree: &Tree,
    req: &SyncRequest,
) -> SyncPlan {
    let source_is_left = matches!(req.direction, SyncDirection::LeftToRight);
    let copy_action = if source_is_left {
        SyncAction::CopyToRight
    } else {
        SyncAction::CopyToLeft
    };
    let delete_action = if source_is_left {
        SyncAction::DeleteRight
    } else {
        SyncAction::DeleteLeft
    };

    let mut entries: Vec<SyncEntry> = Vec::new();
    let mut equal = 0usize;

    for (key, l) in left_tree {
        match right_tree.get(key) {
            None => {
                let action = match req.mode {
                    SyncMode::TwoWay => Some(SyncAction::CopyToRight),
                    SyncMode::OneWay => source_is_left.then_some(copy_action),
                    SyncMode::Mirror => Some(if source_is_left { copy_action } else { delete_action }),
                };
                if let Some(action) = action {
                    entries.push(make_entry(l, action));
                }
            }
            Some(r) => {
                if same_entry(l, r, left, right, req.compare) {
                    equal += 1;
                    continue;
                }
                let action = match req.mode {
                    SyncMode::TwoWay => {
                        let diff = l.meta.mtime - r.meta.mtime;
                        if diff.abs() <= MTIME_TOLERANCE_MS {
                            // 时间相同但内容不同：交给用户决定方向，不擅自覆盖。
                            SyncAction::Conflict
                        } else if diff > 0 {
                            SyncAction::CopyToRight
                        } else {
                            SyncAction::CopyToLeft
                        }
                    }
                    _ => copy_action,
                };
                entries.push(make_entry(l, action));
            }
        }
    }

    for (key, r) in right_tree {
        if left_tree.contains_key(key) {
            continue;
        }
        let action = match req.mode {
            SyncMode::TwoWay => Some(SyncAction::CopyToLeft),
            SyncMode::OneWay => (!source_is_left).then_some(copy_action),
            SyncMode::Mirror => Some(if source_is_left { delete_action } else { copy_action }),
        };
        if let Some(action) = action {
            entries.push(make_entry(r, action));
        }
    }

    // 镜像删除目录时，只保留最上层删除项：子项会随目录一起消失，避免重复删除报错。
    let delete_prefixes: Vec<String> = entries
        .iter()
        .filter(|entry| entry.is_dir && entry.action.is_delete())
        .map(|entry| format!("{}{}", entry.path.to_lowercase(), std::path::MAIN_SEPARATOR))
        .collect();
    if !delete_prefixes.is_empty() {
        entries.retain(|entry| {
            let path = entry.path.to_lowercase();
            !delete_prefixes
                .iter()
                .any(|prefix| path.starts_with(prefix.as_str()))
        });
    }

    entries.sort_by(|a, b| a.path.to_lowercase().cmp(&b.path.to_lowercase()));

    let mut stats = SyncStats {
        equal,
        ..Default::default()
    };
    for entry in &entries {
        if entry.action.is_copy() {
            stats.copy += 1;
            if !entry.is_dir {
                stats.bytes += entry.size;
            }
        } else if entry.action.is_delete() {
            stats.delete += 1;
        } else if entry.action == SyncAction::Conflict {
            stats.conflict += 1;
        }
    }

    SyncPlan { entries, stats }
}

fn make_entry(entry: &TreeEntry, action: SyncAction) -> SyncEntry {
    SyncEntry {
        path: entry.rel.clone(),
        is_dir: entry.meta.is_dir,
        size: entry.meta.size,
        mtime: entry.meta.mtime,
        action,
    }
}

/// 判断两侧同一相对路径的条目是否相同。
fn same_entry(
    l: &TreeEntry,
    r: &TreeEntry,
    left: &Path,
    right: &Path,
    compare: SyncCompare,
) -> bool {
    if l.meta.is_dir != r.meta.is_dir {
        return false;
    }
    if l.meta.is_dir {
        // 目录只看是否存在（时间戳对目录没有意义）。
        return true;
    }
    if l.meta.size != r.meta.size {
        return false;
    }
    match compare {
        SyncCompare::Meta => (l.meta.mtime - r.meta.mtime).abs() <= MTIME_TOLERANCE_MS,
        SyncCompare::Hash => same_content(&left.join(&l.rel), &right.join(&r.rel)),
    }
}

/// 分块比较两个文件内容；任一读取失败都按「不同」处理。
fn same_content(a: &Path, b: &Path) -> bool {
    let (Ok(mut fa), Ok(mut fb)) = (
        fs::File::open(fs_core::io_path(a)),
        fs::File::open(fs_core::io_path(b)),
    ) else {
        return false;
    };
    let mut buf_a = vec![0u8; HASH_CHUNK];
    let mut buf_b = vec![0u8; HASH_CHUNK];
    loop {
        let read_a = match fa.read(&mut buf_a) {
            Ok(value) => value,
            Err(_) => return false,
        };
        let read_b = match fb.read(&mut buf_b) {
            Ok(value) => value,
            Err(_) => return false,
        };
        if read_a != read_b {
            return false;
        }
        if read_a == 0 {
            return true;
        }
        if buf_a[..read_a] != buf_b[..read_b] {
            return false;
        }
    }
}

// ---------------------------------------------------------------- 执行

/// 执行一批同步动作（前端已把扫描结果连同动作发回，这里不再重扫）。
pub fn execute(req: &SyncExecuteRequest, ctx: &OpContext) -> SyncResult {
    let left = PathBuf::from(req.left.trim());
    let right = PathBuf::from(req.right.trim());
    let mut result = SyncResult {
        ok: true,
        ..Default::default()
    };

    let mut dirs: Vec<&SyncExecItem> = Vec::new();
    let mut files: Vec<&SyncExecItem> = Vec::new();
    let mut deletes: Vec<&SyncExecItem> = Vec::new();
    for item in &req.items {
        if item.action.is_delete() {
            deletes.push(item);
        } else if item.action.is_copy() {
            if item.is_dir {
                dirs.push(item);
            } else {
                files.push(item);
            }
        }
    }

    let bytes_total: u64 = files.iter().map(|item| item.size).sum();
    ctx.set_total(files.len() as u64, bytes_total);
    ctx.report("run", "", true);

    // 1. 目录先建（父目录先于子目录）。
    dirs.sort_by_key(|item| item.path.matches(std::path::MAIN_SEPARATOR).count());
    for item in &dirs {
        if ctx.is_cancelled() {
            result.cancelled = true;
            break;
        }
        let dest = dest_path(&left, &right, item);
        match ensure_dir(&dest) {
            Ok(()) => {
                result.copied += 1;
                ctx.add_file(0);
                ctx.tick(&item.path);
            }
            Err(error) => record_error(&mut result, &item.path, &error),
        }
    }

    // 2. 文件并行复制。
    if !result.cancelled {
        let errors: Mutex<Vec<String>> = Mutex::new(Vec::new());
        let denied: Mutex<Vec<String>> = Mutex::new(Vec::new());
        let copied = AtomicUsize::new(0);
        let copied_bytes = AtomicU64::new(0);
        let cancelled = AtomicBool::new(false);
        let parents: Mutex<HashSet<PathBuf>> = Mutex::new(HashSet::new());

        files.par_iter().for_each(|item| {
            if cancelled.load(Ordering::Relaxed) || ctx.is_cancelled() {
                cancelled.store(true, Ordering::Relaxed);
                return;
            }
            let src = source_path(&left, &right, item);
            let dest = dest_path(&left, &right, item);
            if let Some(parent) = dest.parent() {
                let missing = parents
                    .lock()
                    .map(|set| !set.contains(parent))
                    .unwrap_or(true);
                if missing {
                    let _ = fs::create_dir_all(fs_core::io_path(parent));
                    if let Ok(mut set) = parents.lock() {
                        set.insert(parent.to_path_buf());
                    }
                }
            }
            match copy_one(&src, &dest) {
                Ok(bytes) => {
                    copied.fetch_add(1, Ordering::Relaxed);
                    copied_bytes.fetch_add(bytes, Ordering::Relaxed);
                    ctx.add_file(bytes);
                    ctx.tick(&item.path);
                }
                Err(error) => {
                    if error.raw_os_error() == Some(5) {
                        if let Ok(mut list) = denied.lock() {
                            list.push(item.path.clone());
                        }
                    }
                    if let Ok(mut list) = errors.lock() {
                        list.push(format!("复制失败 {}: {error}", item.path));
                    }
                }
            }
        });

        result.copied += copied.load(Ordering::Relaxed);
        result.bytes += copied_bytes.load(Ordering::Relaxed);
        result.cancelled |= cancelled.load(Ordering::Relaxed);
        if let Ok(mut list) = errors.into_inner() {
            result.errors.append(&mut list);
        }
        if let Ok(mut list) = denied.into_inner() {
            result.denied.append(&mut list);
        }
    }

    // 3. 删除（深的先删）。
    if !result.cancelled && !deletes.is_empty() {
        deletes.sort_by_key(|item| {
            std::cmp::Reverse(item.path.matches(std::path::MAIN_SEPARATOR).count())
        });
        let targets: Vec<PathBuf> = deletes
            .iter()
            .map(|item| dest_path(&left, &right, item))
            .filter(|path| fs_core::io_path(path).exists())
            .collect();

        if req.permanent {
            for path in &targets {
                if ctx.is_cancelled() {
                    result.cancelled = true;
                    break;
                }
                match remove_path(path) {
                    Ok(()) => {
                        result.deleted += 1;
                        ctx.add_file(0);
                        ctx.tick(&path.to_string_lossy());
                    }
                    Err(error) => record_error(&mut result, &path.to_string_lossy(), &error),
                }
            }
        } else if !targets.is_empty() {
            // 回收站：一次 IFileOperation 批量投递，比逐项快一个数量级。
            match trash::delete_all(&targets) {
                Ok(()) => {
                    result.deleted += targets.len();
                    ctx.add_file(0);
                    ctx.report("run", "", true);
                }
                Err(_) => {
                    // 批量失败时逐项重试，才能定位到具体失败的路径。
                    for path in &targets {
                        if ctx.is_cancelled() {
                            result.cancelled = true;
                            break;
                        }
                        match fs_core::trash_path(path) {
                            Ok(()) => {
                                result.deleted += 1;
                                ctx.add_file(0);
                                ctx.tick(&path.to_string_lossy());
                            }
                            Err(error) => {
                                result
                                    .errors
                                    .push(format!("删除失败 {}: {error}", path.display()));
                            }
                        }
                    }
                }
            }
        }
    }

    result.ok = result.errors.is_empty() && !result.cancelled;
    ctx.report("done", "", true);
    result
}

fn source_path(left: &Path, right: &Path, item: &SyncExecItem) -> PathBuf {
    if matches!(item.action, SyncAction::CopyToRight) {
        left.join(&item.path)
    } else {
        right.join(&item.path)
    }
}

/// 动作作用的「目标侧」路径（复制是写入目标，删除是删目标侧的多余项）。
fn dest_path(left: &Path, right: &Path, item: &SyncExecItem) -> PathBuf {
    match item.action {
        SyncAction::CopyToLeft | SyncAction::DeleteLeft => left.join(&item.path),
        _ => right.join(&item.path),
    }
}

/// 确保目标目录存在；如果同路径上是文件则先删掉。
fn ensure_dir(dir: &Path) -> std::io::Result<()> {
    let io = fs_core::io_path(dir);
    if io.is_dir() {
        return Ok(());
    }
    if io.exists() {
        fs::remove_file(&io)?;
    }
    fs::create_dir_all(&io)
}

/// 复制一个文件；目标已存在时覆盖（目录则先删）。
fn copy_one(src: &Path, dest: &Path) -> std::io::Result<u64> {
    let src_io = fs_core::io_path(src);
    let dest_io = fs_core::io_path(dest);
    if let Ok(meta) = fs::symlink_metadata(&src_io) {
        if is_reparse_meta(&meta) {
            return Ok(0);
        }
    }
    if let Ok(meta) = fs::metadata(&dest_io) {
        if meta.is_dir() {
            fs::remove_dir_all(&dest_io)?;
        } else if meta.permissions().readonly() {
            // CopyFileEx 遇到只读目标会失败，先去掉只读位。
            let mut permissions = meta.permissions();
            permissions.set_readonly(false);
            let _ = fs::set_permissions(&dest_io, permissions);
        }
    }
    fs::copy(&src_io, &dest_io)
}

/// 永久删除一条路径（文件或目录）。
fn remove_path(path: &Path) -> std::io::Result<()> {
    let io = fs_core::io_path(path);
    if io.is_dir() {
        fs::remove_dir_all(&io)
    } else {
        fs::remove_file(&io)
    }
}

fn record_error(result: &mut SyncResult, path: &str, error: &std::io::Error) {
    if error.raw_os_error() == Some(5) {
        result.denied.push(path.to_string());
    }
    result.errors.push(format!("操作失败 {path}: {error}"));
}

// ---------------------------------------------------------------- 测试

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::Duration;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "stelix-sync-test-{name}-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut file = fs::File::create(path).unwrap();
        file.write_all(body.as_bytes()).unwrap();
    }

    fn set_mtime(path: &Path, secs: u64) {
        let file = fs::OpenOptions::new().write(true).open(path).unwrap();
        file.set_modified(UNIX_EPOCH + Duration::from_secs(secs))
            .unwrap();
    }

    fn request(left: &Path, right: &Path, mode: SyncMode, direction: SyncDirection) -> SyncRequest {
        SyncRequest {
            left: left.to_string_lossy().into_owned(),
            right: right.to_string_lossy().into_owned(),
            mode,
            direction,
            compare: SyncCompare::Meta,
        }
    }

    fn actions(plan: &SyncPlan) -> HashMap<&str, SyncAction> {
        plan.entries
            .iter()
            .map(|entry| (entry.path.as_str(), entry.action))
            .collect()
    }

    #[test]
    fn one_way_detects_missing_and_newer() {
        let base = tmp("oneway");
        let left = base.join("left");
        let right = base.join("right");
        write(&left.join("new.txt"), "new");
        write(&left.join("same.txt"), "same");
        write(&right.join("same.txt"), "same");
        write(&left.join("newer.txt"), "left-version");
        write(&right.join("newer.txt"), "old");
        write(&right.join("only-right.txt"), "right");
        set_mtime(&left.join("same.txt"), 1_700_000_000);
        set_mtime(&right.join("same.txt"), 1_700_000_000);
        set_mtime(&left.join("newer.txt"), 1_700_001_000);
        set_mtime(&right.join("newer.txt"), 1_700_000_000);

        let plan = scan(&request(
            &left,
            &right,
            SyncMode::OneWay,
            SyncDirection::LeftToRight,
        ))
        .unwrap();
        let map = actions(&plan);

        assert_eq!(map.get("new.txt"), Some(&SyncAction::CopyToRight));
        assert_eq!(map.get("newer.txt"), Some(&SyncAction::CopyToRight));
        assert!(!map.contains_key("same.txt"));
        // 单向不删目标多余项，也不把它列进计划。
        assert!(!map.contains_key("only-right.txt"));
        assert_eq!(plan.stats.copy, 2);
        assert_eq!(plan.stats.equal, 1);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn mirror_marks_target_extras_for_delete() {
        let base = tmp("mirror");
        let left = base.join("left");
        let right = base.join("right");
        write(&left.join("keep.txt"), "keep");
        write(&right.join("keep.txt"), "keep");
        write(&right.join("extra.txt"), "extra");
        write(&right.join("extra_dir").join("child.txt"), "child");
        set_mtime(&left.join("keep.txt"), 1_700_000_000);
        set_mtime(&right.join("keep.txt"), 1_700_000_000);

        let plan = scan(&request(
            &left,
            &right,
            SyncMode::Mirror,
            SyncDirection::LeftToRight,
        ))
        .unwrap();
        let map = actions(&plan);

        assert_eq!(map.get("extra.txt"), Some(&SyncAction::DeleteRight));
        assert_eq!(map.get("extra_dir"), Some(&SyncAction::DeleteRight));
        // 子项随目录一起删除，不应重复出现在计划里。
        assert!(!plan.entries.iter().any(|entry| entry.path.contains("child.txt")));
        assert_eq!(plan.stats.delete, 2);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn two_way_picks_newer_and_flags_conflicts() {
        let base = tmp("twoway");
        let left = base.join("left");
        let right = base.join("right");
        write(&left.join("newer-left.txt"), "left");
        write(&right.join("newer-left.txt"), "right");
        write(&left.join("newer-right.txt"), "left");
        write(&right.join("newer-right.txt"), "right");
        // 大小不同但修改时间相同 → 无法判断哪边更新，标记为冲突。
        write(&left.join("conflict.txt"), "aaaa");
        write(&right.join("conflict.txt"), "bb");
        write(&left.join("only-left.txt"), "left");
        write(&right.join("only-right.txt"), "right");

        set_mtime(&left.join("newer-left.txt"), 1_700_001_000);
        set_mtime(&right.join("newer-left.txt"), 1_700_000_000);
        set_mtime(&left.join("newer-right.txt"), 1_700_000_000);
        set_mtime(&right.join("newer-right.txt"), 1_700_001_000);
        set_mtime(&left.join("conflict.txt"), 1_700_000_000);
        set_mtime(&right.join("conflict.txt"), 1_700_000_000);

        let plan = scan(&request(
            &left,
            &right,
            SyncMode::TwoWay,
            SyncDirection::LeftToRight,
        ))
        .unwrap();
        let map = actions(&plan);

        assert_eq!(map.get("newer-left.txt"), Some(&SyncAction::CopyToRight));
        assert_eq!(map.get("newer-right.txt"), Some(&SyncAction::CopyToLeft));
        assert_eq!(map.get("conflict.txt"), Some(&SyncAction::Conflict));
        assert_eq!(map.get("only-left.txt"), Some(&SyncAction::CopyToRight));
        assert_eq!(map.get("only-right.txt"), Some(&SyncAction::CopyToLeft));
        assert_eq!(plan.stats.conflict, 1);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn hash_compare_detects_same_size_same_time_difference() {
        let base = tmp("hash");
        let left = base.join("left");
        let right = base.join("right");
        write(&left.join("same-size.txt"), "aaaa");
        write(&right.join("same-size.txt"), "bbbb");
        set_mtime(&left.join("same-size.txt"), 1_700_000_000);
        set_mtime(&right.join("same-size.txt"), 1_700_000_000);

        let mut meta_req = request(&left, &right, SyncMode::OneWay, SyncDirection::LeftToRight);
        let meta_plan = scan(&meta_req).unwrap();
        assert!(meta_plan.entries.is_empty(), "元数据相同应判为相同");

        meta_req.compare = SyncCompare::Hash;
        let hash_plan = scan(&meta_req).unwrap();
        assert_eq!(hash_plan.entries.len(), 1);
        assert_eq!(hash_plan.entries[0].action, SyncAction::CopyToRight);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn execute_copies_and_mirrors_deletes() {
        let base = tmp("exec");
        let left = base.join("left");
        let right = base.join("right");
        write(&left.join("a.txt"), "left-a");
        write(&left.join("sub").join("b.txt"), "left-b");
        write(&right.join("a.txt"), "old-a");
        write(&right.join("extra.txt"), "extra");

        let plan = scan(&request(
            &left,
            &right,
            SyncMode::Mirror,
            SyncDirection::LeftToRight,
        ))
        .unwrap();
        let items: Vec<SyncExecItem> = plan
            .entries
            .iter()
            .map(|entry| SyncExecItem {
                path: entry.path.clone(),
                is_dir: entry.is_dir,
                size: entry.size,
                action: entry.action,
            })
            .collect();

        let result = execute(
            &SyncExecuteRequest {
                left: left.to_string_lossy().into_owned(),
                right: right.to_string_lossy().into_owned(),
                permanent: true,
                items,
            },
            &OpContext::noop(),
        );

        assert!(result.ok, "{:?}", result.errors);
        assert_eq!(fs::read_to_string(right.join("a.txt")).unwrap(), "left-a");
        assert_eq!(
            fs::read_to_string(right.join("sub").join("b.txt")).unwrap(),
            "left-b"
        );
        assert!(!right.join("extra.txt").exists());
        assert_eq!(result.deleted, 1);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn rejects_same_and_nested_roots() {
        let base = tmp("validate");
        let left = base.join("left");
        let nested = left.join("nested");
        fs::create_dir_all(&nested).unwrap();

        assert!(scan(&request(&left, &left, SyncMode::OneWay, SyncDirection::LeftToRight)).is_err());
        assert!(scan(&request(&left, &nested, SyncMode::Mirror, SyncDirection::LeftToRight)).is_err());
        assert!(scan(&request(
            &base.join("missing"),
            &left,
            SyncMode::OneWay,
            SyncDirection::LeftToRight
        ))
        .is_err());
        let _ = fs::remove_dir_all(&base);
    }
}

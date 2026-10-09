//! 文件系统核心操作 —— 由 **WS-B (feat/fs-ai)** 实现。
//!
//! 契约（Phase 0 冻结）：`list_dir` / `get_roots` / `execute`。
//! 本文件在保留原有行为的基础上，为 `execute` 增加了路径安全校验、
//! 重名自动改名、跨盘 Move 回退（复制 + 删除）等增强。
//!
//! ⚠️ MVP 取舍：删除操作**未接入回收站**（`trash` crate 未在冻结的
//! Cargo.toml 中，且 WS-B 不得修改依赖清单），因此 Delete 走 `std::fs`
//! 直接物理删除。后续 Phase 可通过实现 `IndexSource`/注入 `TrashBackend`
//! trait 的方式接入回收站，而不改动本模块对外签名。

use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::types::{DirListing, DriveInfo, FileEntry, FsOp, FsOpKind, FsOpResult};

/// 由路径生成稳定 id
pub fn path_id(path: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    path.to_lowercase().hash(&mut h);
    h.finish()
}

/// 构造 FileEntry
pub fn to_entry(path: &Path, meta: &fs::Metadata) -> FileEntry {
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string());
    let path_str = path.to_string_lossy().to_string();
    let ext = if meta.is_dir() {
        None
    } else {
        path.extension().map(|e| e.to_string_lossy().to_lowercase())
    };
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    FileEntry {
        id: path_id(&path_str),
        name,
        path: path_str,
        is_dir: meta.is_dir(),
        size: if meta.is_dir() { 0 } else { meta.len() },
        modified,
        ext,
    }
}

/// 列目录：目录在前，同类按名称排序
pub fn list_dir(path: &str) -> Result<DirListing, String> {
    let dir = PathBuf::from(path);
    let rd = fs::read_dir(&dir).map_err(|e| format!("无法读取目录 {path}: {e}"))?;
    let mut entries = Vec::new();
    for item in rd.flatten() {
        let p = item.path();
        match item.metadata() {
            Ok(meta) => entries.push(to_entry(&p, &meta)),
            Err(_) => continue,
        }
    }
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(DirListing {
        path: dir.to_string_lossy().to_string(),
        entries,
    })
}

/// 递归收集某目录下的条目（供 AI 离线检索在索引为空时兜底使用）。
///
/// - `max_depth`：最大递归深度（`1` 表示只取直接子项，`0` 视为 `1`）
/// - `limit`：最多返回的条目数
///
/// 这是一个**新增的辅助函数**，不改动任何已有签名；索引可用时 AI 会优先走索引。
pub fn collect_entries(root: &str, max_depth: usize, limit: usize) -> Vec<FileEntry> {
    let depth = max_depth.max(1);
    let mut out = Vec::with_capacity(limit.min(4096));
    let walker = walkdir::WalkDir::new(root)
        .max_depth(depth)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !e.file_name().to_string_lossy().starts_with('.'));
    for entry in walker.flatten() {
        if entry.depth() == 0 {
            continue;
        }
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        out.push(to_entry(entry.path(), &meta));
        if out.len() >= limit {
            break;
        }
    }
    out
}

/// 枚举本机驱动器
pub fn get_roots() -> Vec<DriveInfo> {
    let mut out = Vec::new();
    let mask = unsafe { windows_sys::Win32::Storage::FileSystem::GetLogicalDrives() };
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = (b'A' + i as u8) as char;
        let mount = format!("{letter}:\\");
        if !Path::new(&mount).exists() {
            continue;
        }
        let (total, free) = drive_space(&mount);
        out.push(DriveInfo {
            name: format!("本地磁盘 ({letter}:)"),
            mount,
            total,
            free,
        });
    }
    out
}

fn drive_space(root: &str) -> (u64, u64) {
    let wide: Vec<u16> = root.encode_utf16().chain(std::iter::once(0)).collect();
    let mut free_to_caller: u64 = 0;
    let mut total: u64 = 0;
    let mut total_free: u64 = 0;
    let ok = unsafe {
        windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free_to_caller as *mut u64,
            &mut total as *mut u64,
            &mut total_free as *mut u64,
        )
    };
    if ok != 0 {
        (total, total_free)
    } else {
        (0, 0)
    }
}

// ---------------------------------------------------------------- 文件操作

/// 执行文件操作。
///
/// 增强点（相对 Phase 0 骨架）：
/// 1. **路径安全校验**：拒绝空路径、拒绝删除/移动驱动器根（如 `C:\`）、
///    拒绝把目录移动/复制到它自身的子目录。
/// 2. **重名自动改名**：Copy/Move 目标已存在同名项时，自动使用 `name (1).ext`。
/// 3. **跨盘 Move 回退**：`rename` 失败（跨盘）时回退为「复制 + 删除」。
/// 4. **删除**：MVP 直接物理删除（见文件头说明，未接入回收站）。
///
/// 注意：`execute` 的签名不含 `AppHandle`，因此这里**不 emit** 任何进度事件，
/// 以保持冻结契约不变。
pub fn execute(op: &FsOp) -> FsOpResult {
    let mut errors: Vec<String> = Vec::new();
    match op.op {
        FsOpKind::Mkdir => match op.target.as_deref() {
            None => errors.push("Mkdir 缺少 target".into()),
            Some(t) if t.trim().is_empty() => errors.push("Mkdir 目标路径为空".into()),
            Some(t) => {
                if let Err(e) = fs::create_dir_all(t) {
                    errors.push(format!("新建目录失败 {t}: {e}"));
                }
            }
        },
        FsOpKind::Delete => {
            if op.sources.is_empty() {
                errors.push("Delete 缺少 sources".into());
            }
            for src in &op.sources {
                if src.trim().is_empty() {
                    errors.push("删除源路径为空".into());
                    continue;
                }
                let p = Path::new(src);
                if is_drive_root(p) {
                    errors.push(format!("已拒绝删除驱动器根目录：{src}"));
                    continue;
                }
                if !p.exists() {
                    errors.push(format!("删除失败 {src}: 路径不存在"));
                    continue;
                }
                // MVP：直接物理删除（未接入回收站，见文件头）。
                if let Err(e) = remove_path(p) {
                    errors.push(format!("删除失败 {src}: {e}"));
                }
            }
        }
        FsOpKind::Rename => {
            let Some(target) = op.target.as_deref().filter(|t| !t.trim().is_empty()) else {
                return FsOpResult {
                    ok: false,
                    errors: vec!["Rename 缺少 target".into()],
                };
            };
            if op.sources.is_empty() {
                errors.push("Rename 缺少 sources".into());
            }
            let tp = Path::new(target);
            for src in &op.sources {
                if src.trim().is_empty() {
                    errors.push("重命名源路径为空".into());
                    continue;
                }
                let sp = Path::new(src);
                if is_drive_root(sp) {
                    errors.push(format!("已拒绝重命名驱动器根目录：{src}"));
                    continue;
                }
                // 目标是已存在目录时，把源移入该目录并自动改名；否则按原样改名。
                let dest = if tp.is_dir() { unique_dest(tp, sp) } else { tp.to_path_buf() };
                if let Some(parent) = dest.parent() {
                    if !parent.as_os_str().is_empty() && !parent.exists() {
                        if let Err(e) = fs::create_dir_all(parent) {
                            errors.push(format!("创建目标目录失败 {}: {e}", parent.display()));
                            continue;
                        }
                    }
                }
                if let Err(e) = fs::rename(sp, &dest) {
                    errors.push(format!("重命名失败 {src} -> {}: {e}", dest.display()));
                }
            }
        }
        FsOpKind::Copy | FsOpKind::Move => {
            let Some(target_dir) = op.target.as_deref().filter(|t| !t.trim().is_empty()) else {
                return FsOpResult {
                    ok: false,
                    errors: vec!["Copy/Move 缺少 target".into()],
                };
            };
            if op.sources.is_empty() {
                errors.push("Copy/Move 缺少 sources".into());
            }
            let tdir = Path::new(target_dir);
            for src in &op.sources {
                if src.trim().is_empty() {
                    errors.push("源路径为空".into());
                    continue;
                }
                let sp = Path::new(src);
                if is_drive_root(sp) {
                    errors.push(format!("已拒绝操作驱动器根目录：{src}"));
                    continue;
                }
                if !sp.exists() {
                    errors.push(format!("操作失败 {src}: 路径不存在"));
                    continue;
                }
                // 防止把目录移动/复制到它自身的子目录（会造成递归或数据丢失）。
                if sp.is_dir() && is_subpath(tdir, sp) {
                    errors.push(format!("已拒绝把目录放入其自身子目录：{src} -> {target_dir}"));
                    continue;
                }
                if let Err(e) = fs::create_dir_all(tdir) {
                    errors.push(format!("创建目标目录失败 {target_dir}: {e}"));
                    continue;
                }
                let dest = unique_dest(tdir, sp);
                let r = if op.op == FsOpKind::Copy {
                    copy_recursive(sp, &dest)
                } else {
                    move_path(sp, &dest)
                };
                if let Err(e) = r {
                    errors.push(format!("操作失败 {src}: {e}"));
                }
            }
        }
    }
    FsOpResult {
        ok: errors.is_empty(),
        errors,
    }
}

// ---------------------------------------------------------------- 内部工具

/// 是否为驱动器根（`C:\` 或 `C:` 之类），这类路径禁止删除 / 移动 / 重命名。
fn is_drive_root(p: &Path) -> bool {
    let comps: Vec<Component> = p.components().collect();
    match comps.as_slice() {
        [Component::Prefix(_)] => true,
        [Component::Prefix(_), Component::RootDir] => true,
        [Component::RootDir] => true,
        _ => false,
    }
}

/// `child` 是否位于 `ancestor` 之下（对两者做规范化，目标不存在时退回原路径）。
fn is_subpath(child: &Path, ancestor: &Path) -> bool {
    let c = canonical_or_self(child);
    let a = canonical_or_self(ancestor);
    c != a && c.starts_with(&a)
}

fn canonical_or_self(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

/// 计算目标路径；若同名已存在则自动改为 `name (1).ext` / `name (1)`。
fn unique_dest(dir: &Path, src: &Path) -> PathBuf {
    let name = src
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let dest = dir.join(&name);
    if !dest.exists() {
        return dest;
    }
    let (stem, ext) = if src.is_dir() {
        (name.clone(), None)
    } else {
        let p = Path::new(&name);
        (
            p.file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| name.clone()),
            p.extension().map(|e| e.to_string_lossy().to_string()),
        )
    };
    for i in 1..100_000u32 {
        let cand = match &ext {
            Some(e) => format!("{stem} ({i}).{e}"),
            None => format!("{stem} ({i})"),
        };
        let d = dir.join(&cand);
        if !d.exists() {
            return d;
        }
    }
    dest
}

/// 移动：优先 `rename`；失败（常见于跨盘）时回退为「复制 + 删除」。
fn move_path(src: &Path, dest: &Path) -> std::io::Result<()> {
    match fs::rename(src, dest) {
        Ok(_) => Ok(()),
        Err(_) => {
            copy_recursive(src, dest)?;
            remove_path(src)
        }
    }
}

fn remove_path(p: &Path) -> std::io::Result<()> {
    if p.is_dir() {
        fs::remove_dir_all(p)
    } else {
        fs::remove_file(p)
    }
}

fn copy_recursive(src: &Path, dest: &Path) -> std::io::Result<()> {
    if src.is_dir() {
        fs::create_dir_all(dest)?;
        for item in fs::read_dir(src)? {
            let item = item?;
            copy_recursive(&item.path(), &dest.join(item.file_name()))?;
        }
        Ok(())
    } else {
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(src, dest).map(|_| ())
    }
}

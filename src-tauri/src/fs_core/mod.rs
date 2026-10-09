//! 文件系统核心操作 —— 由 **WS-B (feat/fs-ai)** 实现。
//!
//! 契约（Phase 0 冻结）：`list_dir` / `get_roots` / `execute`。

use std::fs;
use std::path::{Path, PathBuf};
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

/// 执行文件操作。TODO(WS-B): 增加回收站删除、覆盖策略、进度上报、批量原子性。
pub fn execute(op: &FsOp) -> FsOpResult {
    let mut errors = Vec::new();
    match op.op {
        FsOpKind::Mkdir => {
            if let Some(target) = &op.target {
                if let Err(e) = fs::create_dir_all(target) {
                    errors.push(format!("新建目录失败 {target}: {e}"));
                }
            } else {
                errors.push("Mkdir 缺少 target".into());
            }
        }
        FsOpKind::Delete => {
            for src in &op.sources {
                let p = Path::new(src);
                let r = if p.is_dir() {
                    fs::remove_dir_all(p)
                } else {
                    fs::remove_file(p)
                };
                if let Err(e) = r {
                    errors.push(format!("删除失败 {src}: {e}"));
                }
            }
        }
        FsOpKind::Rename => {
            let Some(target) = &op.target else {
                return FsOpResult {
                    ok: false,
                    errors: vec!["Rename 缺少 target".into()],
                };
            };
            for src in &op.sources {
                if let Err(e) = fs::rename(src, target) {
                    errors.push(format!("重命名失败 {src} -> {target}: {e}"));
                }
            }
        }
        FsOpKind::Copy | FsOpKind::Move => {
            let Some(target_dir) = &op.target else {
                return FsOpResult {
                    ok: false,
                    errors: vec!["Copy/Move 缺少 target".into()],
                };
            };
            for src in &op.sources {
                let name = Path::new(src)
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                let dest = Path::new(target_dir).join(name);
                let r = if op.op == FsOpKind::Copy {
                    copy_recursive(Path::new(src), &dest)
                } else {
                    fs::rename(src, &dest)
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

//! 文件系统核心操作 —— 由 **WS-B (feat/fs-ai)** 实现。
//!
//! 契约（Phase 0 冻结）：`list_dir` / `get_roots` / `execute`。
//! 本文件在保留原有行为的基础上，为 `execute` 增加了路径安全校验、
//! 重名自动改名、跨盘 Move 回退（复制 + 删除）等增强。
//!
//! **删除默认进入系统回收站**（Windows 回收站 / macOS 废纸篓 / Linux Trash），
//! 可通过系统回收站还原；`FsOp::permanent = true` 时跳过回收站永久删除。
//! Move 的跨盘回退等内部清理仍直接物理删除。

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

/// 读取 Windows 文件属性中的「隐藏 / 系统」标志。
///
/// 非 Windows 平台恒返回 `(false, false)`，保持契约字段可用。
fn file_flags(meta: &fs::Metadata) -> (bool, bool) {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
        const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;
        let attrs = meta.file_attributes();
        (
            attrs & FILE_ATTRIBUTE_HIDDEN != 0,
            attrs & FILE_ATTRIBUTE_SYSTEM != 0,
        )
    }
    #[cfg(not(windows))]
    {
        let _ = meta;
        (false, false)
    }
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
    let (hidden, system) = file_flags(meta);
    FileEntry {
        id: path_id(&path_str),
        name,
        path: path_str,
        is_dir: meta.is_dir(),
        size: if meta.is_dir() { 0 } else { meta.len() },
        modified,
        ext,
        hidden,
        system,
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

/// 展开用户在地址栏输入的路径文本。
///
/// 支持 Windows 命令行的常见写法，让 `%APPDATA%`、`~\Downloads` 这类输入
/// 能直接导航：
/// - 环境变量：`%APPDATA%`、`%USERPROFILE%\Downloads`（未知变量保持原样）；
/// - 主目录简写：`~`、`~\Downloads`、`~/Downloads`；
/// - 资源管理器「复制为路径」带上的成对引号；
/// - 统一路径分隔符为 `\`（Windows）。
///
/// 不做 `canonicalize`：目标可能尚不存在，且调用方需要保留用户输入的大小写。
pub fn expand_path(input: &str) -> String {
    let value = strip_quotes(input.trim());
    if value.is_empty() {
        return value;
    }
    expand_home(expand_env_vars(&value)).replace('/', "\\")
}

/// 去掉首尾成对的单 / 双引号（资源管理器复制路径会带上）。
fn strip_quotes(value: &str) -> String {
    let bytes = value.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return value[1..value.len() - 1].trim().to_string();
        }
    }
    value.to_string()
}

/// 展开环境变量；变量值里可能再引用变量，因此迭代多轮直到稳定。
fn expand_env_vars(input: &str) -> String {
    let mut current = input.to_string();
    for _ in 0..4 {
        let next = expand_env_vars_once(&current);
        if next == current {
            break;
        }
        current = next;
    }
    current
}

fn expand_env_vars_once(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            // `%%` 或没有闭合的 `%` 都按字面量保留。
            Some(end) if end > 0 => {
                let name = &after[..end];
                match std::env::var(name) {
                    Ok(value) if !value.is_empty() => out.push_str(&value),
                    // 未定义的变量保持原样，由后续「无法读取目录」提示暴露给用户。
                    _ => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[end + 1..];
            }
            _ => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// 把开头的 `~` 替换为用户主目录（`USERPROFILE`，回退 `HOME`）。
fn expand_home(input: String) -> String {
    let rest = match input.strip_prefix('~') {
        Some(rest) => rest,
        None => return input,
    };
    if !(rest.is_empty() || rest.starts_with('\\') || rest.starts_with('/')) {
        return input;
    }
    let home = std::env::var("USERPROFILE")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| std::env::var("HOME").ok().filter(|value| !value.is_empty()));
    match home {
        Some(home) => format!("{}{rest}", home.trim_end_matches(['\\', '/'])),
        None => input,
    }
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
/// 4. **删除**：默认移到系统回收站；`permanent = true` 时直接物理删除。
///
/// 注意：`execute` 的签名不含 `AppHandle`，因此这里**不 emit** 任何进度事件，
/// 以保持冻结契约不变。
pub fn execute(op: &FsOp) -> FsOpResult {
    let mut errors: Vec<String> = Vec::new();
    // 因「拒绝访问」失败的路径：前端据此发起 UAC 提权重试。
    let mut denied: Vec<String> = Vec::new();
    // 成功条目的逆向操作：前端据此提供「撤销上一步」。
    let mut undo: Vec<FsOp> = Vec::new();
    match op.op {
        FsOpKind::Mkdir => {
            // target 为单目录创建；sources 承载「批量新建」的其余目标（也供提权子进程
            // 一次重试多个被拒绝的目录）。单个目标失败不影响同一批里的其它目标。
            if op.target.is_none() && op.sources.is_empty() {
                errors.push("Mkdir 缺少 target".into());
            }
            let targets = op
                .target
                .iter()
                .chain(op.sources.iter())
                .map(String::as_str);
            for t in targets {
                if t.trim().is_empty() {
                    errors.push("Mkdir 目标路径为空".into());
                    continue;
                }
                let p = Path::new(t);
                if is_drive_root(p) {
                    errors.push(format!("已拒绝在驱动器根新建目录：{t}"));
                } else if p.exists() {
                    // 同名已存在：显式报错，避免 create_dir_all 静默成功而无任何反馈。
                    errors.push(format!("已存在同名项：{t}"));
                } else if let Err(e) = fs::create_dir_all(t) {
                    if is_access_denied(&e) {
                        denied.push(t.to_string());
                    }
                    errors.push(format!("新建目录失败 {t}: {e}"));
                } else {
                    undo.push(delete_undo(t));
                }
            }
        }
        FsOpKind::CreateFile => match op.target.as_deref() {
            None => errors.push("CreateFile 缺少 target".into()),
            Some(t) if t.trim().is_empty() => errors.push("新建文件路径为空".into()),
            Some(t) => {
                let p = Path::new(t);
                if is_drive_root(p) {
                    errors.push(format!("已拒绝在驱动器根新建文件：{t}"));
                } else if p.exists() {
                    errors.push(format!("已存在同名项：{t}"));
                } else {
                    if let Some(parent) = p.parent() {
                        if !parent.as_os_str().is_empty() && !parent.exists() {
                            if let Err(e) = fs::create_dir_all(parent) {
                                if is_access_denied(&e) {
                                    denied.push(t.to_string());
                                }
                                errors.push(format!("创建目标目录失败 {}: {e}", parent.display()));
                                return FsOpResult {
                                    ok: false,
                                    errors,
                                    denied,
                                    undo,
                                };
                            }
                        }
                    }
                    // create_new：目标已存在时返回错误（前面已判存在，这里是兜底）。
                    if let Err(e) = fs::OpenOptions::new().create_new(true).write(true).open(p) {
                        if is_access_denied(&e) {
                            denied.push(t.to_string());
                        }
                        errors.push(format!("新建文件失败 {t}: {e}"));
                    } else {
                        undo.push(delete_undo(t));
                    }
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
                if op.permanent {
                    // 显式永久删除：跳过回收站，直接物理删除。
                    if let Err(e) = remove_path(p) {
                        if is_access_denied(&e) {
                            denied.push(src.clone());
                        }
                        errors.push(format!("永久删除失败 {src}: {e}"));
                    }
                } else if let Err(e) = trash_path(p) {
                    // 默认移到系统回收站：用户可在回收站中还原误删项。
                    if is_trash_access_denied(&e) {
                        denied.push(src.clone());
                    }
                    errors.push(format!("移到回收站失败 {src}: {e}"));
                }
            }
        }
        FsOpKind::Rename => {
            let Some(target) = op.target.as_deref().filter(|t| !t.trim().is_empty()) else {
                return FsOpResult {
                    ok: false,
                    errors: vec!["Rename 缺少 target".into()],
                    denied,
                    undo,
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
                let dest = if tp.is_dir() {
                    unique_dest(tp, sp)
                } else {
                    tp.to_path_buf()
                };
                if let Some(parent) = dest.parent() {
                    if !parent.as_os_str().is_empty() && !parent.exists() {
                        if let Err(e) = fs::create_dir_all(parent) {
                            if is_access_denied(&e) {
                                denied.push(src.clone());
                            }
                            errors.push(format!("创建目标目录失败 {}: {e}", parent.display()));
                            continue;
                        }
                    }
                }
                if let Err(e) = fs::rename(sp, &dest) {
                    if is_access_denied(&e) {
                        denied.push(src.clone());
                    }
                    errors.push(format!("重命名失败 {src} -> {}: {e}", dest.display()));
                } else {
                    undo.push(rename_undo(&dest, src));
                }
            }
        }
        FsOpKind::Copy | FsOpKind::Move => {
            let Some(target_dir) = op.target.as_deref().filter(|t| !t.trim().is_empty()) else {
                return FsOpResult {
                    ok: false,
                    errors: vec!["Copy/Move 缺少 target".into()],
                    denied,
                    undo,
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
                    errors.push(format!(
                        "已拒绝把目录放入其自身子目录：{src} -> {target_dir}"
                    ));
                    continue;
                }
                if let Err(e) = fs::create_dir_all(tdir) {
                    if is_access_denied(&e) {
                        denied.push(src.clone());
                    }
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
                    if is_access_denied(&e) {
                        denied.push(src.clone());
                    }
                    errors.push(format!("操作失败 {src}: {e}"));
                } else if op.op == FsOpKind::Copy {
                    undo.push(delete_undo(&dest.to_string_lossy()));
                } else if let Some(parent) = sp.parent() {
                    if !parent.as_os_str().is_empty() {
                        undo.push(move_undo(&dest, &parent.to_string_lossy()));
                    }
                }
            }
        }
        FsOpKind::Duplicate => {
            if op.sources.is_empty() {
                errors.push("Duplicate 缺少 sources".into());
            }
            for src in &op.sources {
                if src.trim().is_empty() {
                    errors.push("复制副本源路径为空".into());
                    continue;
                }
                let sp = Path::new(src);
                if is_drive_root(sp) {
                    errors.push(format!("已拒绝复制驱动器根目录：{src}"));
                    continue;
                }
                if !sp.exists() {
                    errors.push(format!("复制副本失败 {src}: 路径不存在"));
                    continue;
                }
                let dest = unique_duplicate_dest(sp);
                if let Err(e) = copy_recursive(sp, &dest) {
                    if is_access_denied(&e) {
                        denied.push(src.clone());
                    }
                    errors.push(format!("复制副本失败 {src}: {e}"));
                } else {
                    undo.push(delete_undo(&dest.to_string_lossy()));
                }
            }
        }
    }
    FsOpResult {
        ok: errors.is_empty(),
        errors,
        denied,
        undo,
    }
}

/// 删除某条路径（移到回收站）——用于撤销「新建」与「复制」。
fn delete_undo(path: &str) -> FsOp {
    FsOp {
        op: FsOpKind::Delete,
        sources: vec![path.to_string()],
        target: None,
        permanent: false,
    }
}

/// 把 `dest` 改回 `src`——用于撤销「重命名」。
fn rename_undo(dest: &Path, src: &str) -> FsOp {
    FsOp {
        op: FsOpKind::Rename,
        sources: vec![dest.to_string_lossy().into_owned()],
        target: Some(src.to_string()),
        permanent: false,
    }
}

/// 把 `dest` 移回 `parent`——用于撤销「移动」。
fn move_undo(dest: &Path, parent: &str) -> FsOp {
    FsOp {
        op: FsOpKind::Move,
        sources: vec![dest.to_string_lossy().into_owned()],
        target: Some(parent.to_string()),
        permanent: false,
    }
}

/// 是否为 Windows「拒绝访问」错误（`ERROR_ACCESS_DENIED` = `os error 5`）。
///
/// 只有此错误才值得 UAC 提权重试。其它容易混淆的错误码都**不**映射为 5，故不会被误判：
/// 文件被占用 `ERROR_SHARING_VIOLATION`(32)、目录非空 `ERROR_DIR_NOT_EMPTY`(145)、
/// 权限不足 `ERROR_PRIVILEGE_NOT_HELD`(1314)、网络拒绝 `WSAEACCES`(10013)。
fn is_access_denied(e: &std::io::Error) -> bool {
    e.raw_os_error() == Some(5)
}

// ---------------------------------------------------------------- 内部工具

/// 词法规范化路径：解析掉 `.` 与 `..`，**不访问文件系统**（目标可不存在）。
///
/// 之所以需要它：`C:\Windows\..` 的组件是 `[Prefix, RootDir, Normal, ParentDir]`，
/// 直接按组件判定会误认为「非根」而放行删除，从而递归清空整个卷。规范化后
/// `C:\Windows\..` 归一为 `C:\`，可被正确拦截。`..` 不会越过卷根
/// （`C:\Windows\..` → `C:\`），前缀 / 根 / 盘符原样保留。
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    let mut has_root = false;
    let mut depth = 0usize; // 根之后已压入的 Normal 组件数量
    for comp in p.components() {
        match comp {
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            Component::RootDir => {
                out.push(Component::RootDir.as_os_str());
                has_root = true;
                depth = 0;
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if depth > 0 {
                    out.pop();
                    depth -= 1;
                } else if !has_root {
                    // 相对路径开头的 `..` 无处可归，保留原样。
                    out.push("..");
                }
            }
            Component::Normal(seg) => {
                out.push(seg);
                depth += 1;
            }
        }
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

/// 是否为驱动器根（`C:\` 或 `C:` 之类），这类路径禁止删除 / 移动 / 重命名。
///
/// 判定前先做词法规范化，避免 `C:\Windows\..` 这类路径绕过保护。
fn is_drive_root(p: &Path) -> bool {
    let norm = normalize(p);
    let comps: Vec<Component> = norm.components().collect();
    match comps.as_slice() {
        [Component::Prefix(_)] => true,
        [Component::Prefix(_), Component::RootDir] => true,
        [Component::RootDir] => true,
        _ => false,
    }
}

/// `child` 是否位于 `ancestor` 之下（对两者做**词法**规范化后比较包含关系）。
///
/// 不用 `canonicalize`：调用点检查的是**尚未创建**的目标目录，canonicalize
/// 失败退回原路径导致 `starts_with` 失配，进而放行「移动到自身子目录」并触发
/// 无限递归。词法规范化对不存在的路径同样有效。
fn is_subpath(child: &Path, ancestor: &Path) -> bool {
    let c = normalize(child);
    let a = normalize(ancestor);
    c != a && c.starts_with(&a)
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

/// 生成同目录副本的目标名：`原名 - 副本.ext`，重复时追加 ` (2)`、` (3)`…
fn unique_duplicate_dest(src: &Path) -> PathBuf {
    let dir = src.parent().unwrap_or_else(|| Path::new(""));
    let name = src
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let (stem, ext) = if src.is_dir() {
        (name, None)
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
        let suffix = if i == 1 {
            " - 副本".to_string()
        } else {
            format!(" - 副本 ({i})")
        };
        let cand = match &ext {
            Some(e) => format!("{stem}{suffix}.{e}"),
            None => format!("{stem}{suffix}"),
        };
        let dest = dir.join(cand);
        if !dest.exists() {
            return dest;
        }
    }
    dir.join(format!("{stem} - 副本"))
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

/// 把单个路径移到系统回收站。
///
/// `trash` crate 在 Windows 上走 `IFileOperation`（回收站），
/// macOS / Linux 分别走废纸篓与 Freedesktop Trash 规范。
pub(crate) fn trash_path(p: &Path) -> Result<(), trash::Error> {
    trash::delete(p)
}

/// 回收站操作是否属于权限不足（可触发 UAC 提权重试）。
///
/// Windows 下多为 `HRESULT_FROM_WIN32(ERROR_ACCESS_DENIED)` = `0x80070005`；
/// `CouldNotAccess` 表示路径不存在或当前进程无权读取。
fn is_trash_access_denied(e: &trash::Error) -> bool {
    match e {
        trash::Error::Os { code, .. } => *code as u32 == 0x8007_0005,
        trash::Error::CouldNotAccess { .. } => true,
        _ => false,
    }
}

/// 直接物理删除（仅用于 Move 跨盘回退等内部清理，不面向用户的删除操作）。
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FsOp, FsOpKind};

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("stelix-fs-core-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn op(kind: FsOpKind, target: &Path) -> FsOp {
        FsOp {
            op: kind,
            sources: Vec::new(),
            target: Some(target.to_string_lossy().to_string()),
            permanent: false,
        }
    }

    /// 地址栏输入：`%VAR%` / `~` / 引号 / 正斜杠都要能落到真实路径。
    #[test]
    fn expand_path_handles_env_vars_home_and_quotes() {
        std::env::set_var("STELIX_TEST_EXPAND_DIR", r"D:\StelixTestDir");
        assert_eq!(
            expand_path(r"%STELIX_TEST_EXPAND_DIR%\src"),
            r"D:\StelixTestDir\src"
        );
        // 未定义的变量保持原样，避免把用户输入静默改写。
        assert_eq!(
            expand_path(r"D:\%STELIX_NO_SUCH_VAR%\a"),
            r"D:\%STELIX_NO_SUCH_VAR%\a"
        );
        // 资源管理器「复制为路径」带引号；正斜杠统一为反斜杠。
        assert_eq!(expand_path("  \"D:\\a/b\"  "), r"D:\a\b");
        assert_eq!(expand_path(""), "");
    }

    #[test]
    fn expand_path_expands_home_dir() {
        let home = std::env::var("USERPROFILE")
            .ok()
            .filter(|value| !value.is_empty())
            .or_else(|| std::env::var("HOME").ok().filter(|value| !value.is_empty()));
        let Some(home) = home else {
            return;
        };
        let home = home.replace('/', "\\");
        let home = home.trim_end_matches('\\');
        assert_eq!(expand_path("~"), home);
        assert_eq!(expand_path(r"~\Downloads"), format!(r"{home}\Downloads"));
        assert_eq!(expand_path("~/Downloads"), format!(r"{home}\Downloads"));
        // `~` 只在路径开头且后面紧跟分隔符时才是主目录。
        assert_eq!(expand_path("~foo"), "~foo");
    }

    #[test]
    fn create_file_creates_empty_file() {
        let dir = temp_dir("create-file");
        let target = dir.join("hello.txt");
        let res = execute(&op(FsOpKind::CreateFile, &target));
        assert!(res.ok, "{:?}", res.errors);
        assert!(target.is_file());
        assert_eq!(fs::metadata(&target).unwrap().len(), 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn create_file_rejects_existing_name_without_truncating() {
        let dir = temp_dir("create-file-dup");
        let target = dir.join("dup.txt");
        fs::write(&target, b"keep").unwrap();
        let res = execute(&op(FsOpKind::CreateFile, &target));
        assert!(!res.ok);
        assert_eq!(fs::read(&target).unwrap(), b"keep");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn successful_ops_return_reverse_undo_entries() {
        let dir = temp_dir("undo-ops");
        let source = dir.join("a.txt");
        fs::write(&source, b"a").unwrap();
        let renamed = dir.join("b.txt");

        let res = execute(&FsOp {
            op: FsOpKind::Rename,
            sources: vec![source.to_string_lossy().to_string()],
            target: Some(renamed.to_string_lossy().to_string()),
            permanent: false,
        });
        assert!(res.ok, "{:?}", res.errors);
        assert_eq!(res.undo.len(), 1);
        assert_eq!(res.undo[0].op, FsOpKind::Rename);
        assert_eq!(
            res.undo[0].sources,
            vec![renamed.to_string_lossy().to_string()]
        );
        assert_eq!(
            res.undo[0].target.as_deref(),
            Some(source.to_string_lossy().as_ref())
        );

        let copies = dir.join("copies");
        let res = execute(&FsOp {
            op: FsOpKind::Copy,
            sources: vec![renamed.to_string_lossy().to_string()],
            target: Some(copies.to_string_lossy().to_string()),
            permanent: false,
        });
        assert!(res.ok, "{:?}", res.errors);
        assert_eq!(res.undo.len(), 1);
        assert_eq!(res.undo[0].op, FsOpKind::Delete);
        assert_eq!(
            res.undo[0].sources,
            vec![copies.join("b.txt").to_string_lossy().to_string()]
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn duplicate_creates_copy_with_chinese_suffix() {
        let dir = temp_dir("duplicate");
        let file = dir.join("report.txt");
        fs::write(&file, b"data").unwrap();
        let res = execute(&FsOp {
            op: FsOpKind::Duplicate,
            sources: vec![file.to_string_lossy().to_string()],
            target: None,
            permanent: false,
        });
        assert!(res.ok, "{:?}", res.errors);
        assert_eq!(fs::read(dir.join("report - 副本.txt")).unwrap(), b"data");
        // 再次副本：追加序号，且原文件保持不变。
        assert!(
            execute(&FsOp {
                op: FsOpKind::Duplicate,
                sources: vec![file.to_string_lossy().to_string()],
                target: None,
                permanent: false,
            })
            .ok
        );
        assert!(dir.join("report - 副本 (2).txt").is_file());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn mkdir_reports_existing_name() {
        let dir = temp_dir("mkdir-dup");
        let target = dir.join("sub");
        fs::create_dir(&target).unwrap();
        let res = execute(&op(FsOpKind::Mkdir, &target));
        assert!(!res.ok);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn mkdir_creates_multiple_targets_from_sources() {
        let dir = temp_dir("mkdir-batch");
        let first = dir.join("first");
        let second = dir.join("second");
        let existing = dir.join("existing");
        fs::create_dir(&existing).unwrap();
        let res = execute(&FsOp {
            op: FsOpKind::Mkdir,
            sources: vec![
                first.to_string_lossy().to_string(),
                second.to_string_lossy().to_string(),
                existing.to_string_lossy().to_string(),
            ],
            target: None,
            permanent: false,
        });
        // 已存在的项报错，但不影响其它目录创建。
        assert!(!res.ok);
        assert!(first.is_dir());
        assert!(second.is_dir());
        assert_eq!(res.errors.len(), 1);
        assert!(res.errors[0].contains("已存在同名项"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn move_files_into_newly_created_folder() {
        let dir = temp_dir("move-new-folder");
        let source = dir.join("a.txt");
        fs::write(&source, b"a").unwrap();
        let target = dir.join("archive");
        assert!(execute(&op(FsOpKind::Mkdir, &target)).ok);

        let res = execute(&FsOp {
            op: FsOpKind::Move,
            sources: vec![source.to_string_lossy().to_string()],
            target: Some(target.to_string_lossy().to_string()),
            permanent: false,
        });
        assert!(res.ok, "{:?}", res.errors);
        assert!(!source.exists());
        assert!(target.join("a.txt").is_file());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_access_denied_counts_as_elevatable() {
        // 只有 ERROR_ACCESS_DENIED(5) 值得提权重试。
        assert!(is_access_denied(&std::io::Error::from_raw_os_error(5)));
        // 文件被占用 / 目录非空 / 权限不足 / 网络拒绝：都不该触发提权。
        for code in [32, 145, 1314, 10013] {
            assert!(
                !is_access_denied(&std::io::Error::from_raw_os_error(code)),
                "错误码 {code} 不应判定为可提权"
            );
        }
        // 非 OS 错误（无错误码）同样不提权。
        assert!(!is_access_denied(&std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "x"
        )));
    }

    #[test]
    fn only_trash_access_errors_are_elevatable() {
        // HRESULT_FROM_WIN32(ERROR_ACCESS_DENIED) 与「无法访问目标」都要触发提权重试。
        assert!(is_trash_access_denied(&trash::Error::Os {
            code: 0x8007_0005u32 as i32,
            description: String::new(),
        }));
        assert!(is_trash_access_denied(&trash::Error::CouldNotAccess {
            target: "C:\\tmp\\a.txt".into(),
        }));
        // 文件被占用（0x80070020）等其它 shell 错误不触发提权。
        assert!(!is_trash_access_denied(&trash::Error::Os {
            code: 0x8007_0020u32 as i32,
            description: String::new(),
        }));
        assert!(!is_trash_access_denied(&trash::Error::Unknown {
            description: String::new(),
        }));
    }

    #[test]
    fn permanent_delete_removes_file_directly() {
        let dir = temp_dir("permanent-delete");
        let file = dir.join("a.txt");
        fs::write(&file, b"x").unwrap();
        let res = execute(&FsOp {
            op: FsOpKind::Delete,
            sources: vec![file.to_string_lossy().to_string()],
            target: None,
            permanent: true,
        });
        assert!(res.ok, "{:?}", res.errors);
        assert!(!file.exists(), "永久删除后文件不应存在");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn result_serializes_denied_field() {
        let res = execute(&FsOp {
            op: FsOpKind::Delete,
            sources: vec![String::new()],
            target: None,
            permanent: false,
        });
        let json = serde_json::to_string(&res).unwrap();
        assert!(
            json.contains("\"denied\""),
            "序列化结果应含 denied 字段: {json}"
        );
        // 普通失败（源路径为空）不进入 denied。
        assert!(!res.ok);
        assert!(res.denied.is_empty());
    }

    /// BUG A：`C:\Windows\..` 归一后即为驱动器根，必须被拦截；
    /// 普通目录不能被误判为根。
    #[cfg(windows)]
    #[test]
    fn drive_root_detection_resolves_parent_dir() {
        assert!(is_drive_root(Path::new(r"C:\")));
        assert!(is_drive_root(Path::new(r"C:")));
        assert!(is_drive_root(Path::new(r"C:\Windows\..")));
        assert!(is_drive_root(Path::new(r"C:\Windows\System32\..\..")));
        // 普通目录（含解析 `..` 后仍非根的路径）不应被误判。
        assert!(!is_drive_root(Path::new(r"C:\Windows")));
        assert!(!is_drive_root(Path::new(r"C:\Windows\..\Users")));
    }

    /// BUG B：目标（`C:\a\b\c`）尚不存在时也要判定为子路径；
    /// 同级 / 无关 / 前缀相似的路径不能误判。
    #[cfg(windows)]
    #[test]
    fn subpath_detects_nonexistent_target() {
        assert!(is_subpath(Path::new(r"C:\a\b\c"), Path::new(r"C:\a")));
        assert!(is_subpath(Path::new(r"C:\a\b\c\..\d"), Path::new(r"C:\a")));
        // 自身不算子路径。
        assert!(!is_subpath(Path::new(r"C:\a"), Path::new(r"C:\a")));
        // 同级 / 无关 / 前缀相似（C:\ab 与 C:\a）不误判。
        assert!(!is_subpath(Path::new(r"C:\a\c"), Path::new(r"C:\a\b")));
        assert!(!is_subpath(Path::new(r"C:\other"), Path::new(r"C:\a")));
        assert!(!is_subpath(Path::new(r"C:\ab"), Path::new(r"C:\a")));
    }

    /// BUG B（端到端）：目标目录尚不存在时，Copy 到自身子目录必须被拒绝，
    /// 且不得创建出该目标目录（否则会无限递归拷贝）。
    #[test]
    fn copy_into_own_nonexistent_subdir_is_rejected() {
        let dir = temp_dir("copy-self-subdir");
        let src = dir.join("a");
        fs::create_dir_all(&src).unwrap();
        let target = src.join("b").join("c"); // 检查时该目录尚不存在
        let mut o = op(FsOpKind::Copy, &target);
        o.sources = vec![src.to_string_lossy().to_string()];
        let res = execute(&o);
        assert!(!res.ok, "应拒绝把目录复制到自身子目录: {:?}", res.errors);
        assert!(
            res.errors.iter().any(|e| e.contains("自身子目录")),
            "{:?}",
            res.errors
        );
        assert!(!target.exists(), "被拒绝的目标目录不应被创建");
        let _ = fs::remove_dir_all(&dir);
    }
}

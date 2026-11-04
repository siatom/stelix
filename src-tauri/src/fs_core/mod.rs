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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

use walkdir::WalkDir;

use crate::types::{
    ConflictItem, ConflictPolicy, DirListing, DriveInfo, FileEntry, FsOp, FsOpResult, FsProgress,
};

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
    // 用扩展长度路径读盘，但条目路径仍由原始目录拼接——避免 verbatim 前缀漏到界面。
    let rd = fs::read_dir(io_path(&dir)).map_err(|e| format!("无法读取目录 {path}: {e}"))?;
    let mut entries = Vec::new();
    for item in rd.flatten() {
        let p = dir.join(item.file_name());
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

// ---------------------------------------------------------------- 执行上下文

/// 供 UI 线程消费的进度回调（lib.rs 把它接到 fs:progress 事件）。
pub type ProgressSink = std::sync::Arc<dyn Fn(FsProgress) + Send + Sync>;

/// 进度上报的最小间隔，避免大目录复制时刷爆事件通道。
const PROGRESS_INTERVAL: Duration = Duration::from_millis(90);

/// 一次文件操作的上下文：取消标记 + 进度上报。
///
/// 复制 / 移动 / 删除是长时间阻塞操作，调用方（lib.rs）创建它并登记到取消表，
/// 用户点「取消」时把 cancel 置位，执行循环在下一个检查点退出。
pub struct OpContext {
    op_id: String,
    kind: &'static str,
    cancel: Arc<AtomicBool>,
    sink: Option<ProgressSink>,
    last_emit: Mutex<Instant>,
    files_done: AtomicU64,
    bytes_done: AtomicU64,
    files_total: AtomicU64,
    bytes_total: AtomicU64,
}

impl OpContext {
    pub fn new(
        op_id: String,
        kind: &'static str,
        cancel: Arc<AtomicBool>,
        sink: Option<ProgressSink>,
    ) -> Self {
        Self {
            op_id,
            kind,
            cancel,
            sink,
            last_emit: Mutex::new(Instant::now() - PROGRESS_INTERVAL),
            files_done: AtomicU64::new(0),
            bytes_done: AtomicU64::new(0),
            files_total: AtomicU64::new(0),
            bytes_total: AtomicU64::new(0),
        }
    }

    /// 不取消、不上报进度的上下文（单测 / 提权子进程 / AI 工具调用）。
    pub fn noop() -> Self {
        Self::new(String::new(), "", Arc::new(AtomicBool::new(false)), None)
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// 设置进度分母（预扫描结果）。
    pub fn set_total(&self, files: u64, bytes: u64) {
        self.files_total.store(files, Ordering::Relaxed);
        self.bytes_total.store(bytes, Ordering::Relaxed);
    }

    /// 记一个已完成的条目（文件或顶层删除项）。
    fn add_file(&self, bytes: u64) {
        self.files_done.fetch_add(1, Ordering::Relaxed);
        self.bytes_done.fetch_add(bytes, Ordering::Relaxed);
    }

    /// 节流上报执行进度。
    fn tick(&self, current: &str) {
        self.report("run", current, false);
    }

    /// 上报进度；force = true 时忽略节流（阶段切换、开始与结束）。
    pub fn report(&self, phase: &str, current: &str, force: bool) {
        let Some(sink) = &self.sink else { return };
        match self.last_emit.lock() {
            Ok(mut last) => {
                if !force && last.elapsed() < PROGRESS_INTERVAL {
                    return;
                }
                *last = Instant::now();
            }
            Err(_) => return,
        }
        sink(FsProgress {
            op_id: self.op_id.clone(),
            kind: self.kind.to_string(),
            phase: phase.to_string(),
            files_done: self.files_done.load(Ordering::Relaxed),
            files_total: self.files_total.load(Ordering::Relaxed),
            bytes_done: self.bytes_done.load(Ordering::Relaxed),
            bytes_total: self.bytes_total.load(Ordering::Relaxed),
            current: current.to_string(),
        });
    }
}

// ---------------------------------------------------------------- 长路径

/// 把路径转换成 Windows 扩展长度形式（verbatim 前缀），使超过 MAX_PATH(260) 的路径可用。
///
/// - 先做词法规范化：verbatim 前缀会关闭系统对 . / .. 的解析。
/// - 非 Windows、相对路径、已带前缀的路径原样返回。
/// - 只用于**文件系统调用**，返回值不要透给前端（否则界面里会出现前缀）。
pub fn io_path(p: &Path) -> PathBuf {
    let norm = normalize(p);
    #[cfg(windows)]
    {
        if !norm.is_absolute() {
            return norm;
        }
        let text = norm.to_string_lossy();
        if text.starts_with("\\\\?\\") {
            return norm;
        }
        if let Some(Component::Prefix(prefix)) = norm.components().next() {
            match prefix.kind() {
                std::path::Prefix::Disk(_) => return PathBuf::from(format!("\\\\?\\{text}")),
                std::path::Prefix::UNC(server, share) => {
                    return PathBuf::from(format!(
                        "\\\\?\\UNC\\{}\\{}",
                        server.to_string_lossy(),
                        share.to_string_lossy()
                    ))
                }
                _ => {}
            }
        }
        norm
    }
    #[cfg(not(windows))]
    {
        norm
    }
}

/// 目标是否已存在（支持超长路径）。
fn io_exists(p: &Path) -> bool {
    io_path(p).exists()
}

/// 是否为符号链接 / junction 等重解析点。
///
/// 复制时**必须**跳过：跟随 junction 会把目标内容重复复制一遍，指向祖先的
/// junction 更会造成无限递归。
fn is_reparse_point(meta: &fs::Metadata) -> bool {
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

/// walkdir 的过滤：目录型重解析点不下钻。
fn is_reparse_dir_entry(entry: &walkdir::DirEntry) -> bool {
    if !entry.file_type().is_dir() {
        return false;
    }
    if entry.path_is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        entry
            .metadata()
            .map(|m| is_reparse_point(&m))
            .unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        false
    }
}

// ---------------------------------------------------------------- 执行

/// 执行一次文件操作。
///
/// ctx 提供取消标记与进度上报；OpContext::noop() 表示不取消、不报进度
/// （单测 / 提权子进程 / AI 工具调用走这条）。
pub fn execute(op: &FsOp, ctx: &OpContext) -> FsOpResult {
    let mut errors: Vec<String> = Vec::new();
    // 因「拒绝访问」失败的路径：前端据此发起 UAC 提权重试。
    let mut denied: Vec<String> = Vec::new();
    // 成功条目的逆向操作：前端据此提供「撤销上一步」。
    let mut undo: Vec<FsOp> = Vec::new();
    // 主动跳过的路径（符号链接 / 冲突跳过）：不算失败。
    let mut skipped: Vec<String> = Vec::new();
    let mut cancelled = false;

    match op {
        FsOp::Mkdir { targets } => {
            if targets.is_empty() {
                errors.push("Mkdir 缺少 targets".into());
            }
            for t in targets {
                if ctx.is_cancelled() {
                    cancelled = true;
                    break;
                }
                if t.trim().is_empty() {
                    errors.push("Mkdir 目标路径为空".into());
                    continue;
                }
                let p = Path::new(t);
                if is_drive_root(p) {
                    errors.push(format!("已拒绝在驱动器根新建目录：{t}"));
                } else if io_exists(p) {
                    // 同名已存在：显式报错，避免 create_dir_all 静默成功而无任何反馈。
                    errors.push(format!("已存在同名项：{t}"));
                } else if let Err(e) = fs::create_dir_all(io_path(p)) {
                    if is_access_denied(&e) {
                        denied.push(t.clone());
                    }
                    errors.push(format!("新建目录失败 {t}: {e}"));
                } else {
                    undo.push(delete_undo(t));
                }
            }
        }
        FsOp::CreateFile { target } => {
            if target.trim().is_empty() {
                errors.push("新建文件路径为空".into());
            } else {
                let p = Path::new(target);
                if is_drive_root(p) {
                    errors.push(format!("已拒绝在驱动器根新建文件：{target}"));
                } else if io_exists(p) {
                    errors.push(format!("已存在同名项：{target}"));
                } else {
                    let mut failed = false;
                    if let Some(parent) = p.parent() {
                        if !parent.as_os_str().is_empty() && !io_exists(parent) {
                            if let Err(e) = fs::create_dir_all(io_path(parent)) {
                                if is_access_denied(&e) {
                                    denied.push(target.clone());
                                }
                                errors.push(format!("创建目标目录失败 {}: {e}", parent.display()));
                                failed = true;
                            }
                        }
                    }
                    if !failed {
                        // create_new：目标已存在时返回错误（前面已判存在，这里是兜底）。
                        if let Err(e) = fs::OpenOptions::new()
                            .create_new(true)
                            .write(true)
                            .open(io_path(p))
                        {
                            if is_access_denied(&e) {
                                denied.push(target.clone());
                            }
                            errors.push(format!("新建文件失败 {target}: {e}"));
                        } else {
                            undo.push(delete_undo(target));
                        }
                    }
                }
            }
        }
        FsOp::Delete { sources, permanent } => {
            if sources.is_empty() {
                errors.push("Delete 缺少 sources".into());
            }
            ctx.set_total(sources.len() as u64, 0);
            ctx.report("run", "", true);
            for src in sources {
                if ctx.is_cancelled() {
                    cancelled = true;
                    break;
                }
                if src.trim().is_empty() {
                    errors.push("删除源路径为空".into());
                    continue;
                }
                let p = Path::new(src);
                if is_drive_root(p) {
                    errors.push(format!("已拒绝删除驱动器根目录：{src}"));
                    continue;
                }
                if !io_exists(p) {
                    errors.push(format!("删除失败 {src}: 路径不存在"));
                    continue;
                }
                if *permanent {
                    // 显式永久删除：跳过回收站，直接物理删除。
                    if let Err(e) = remove_path(p) {
                        if is_access_denied(&e) {
                            denied.push(src.clone());
                        }
                        let hint = if is_sharing_violation(&e) {
                            locked_hint(p)
                        } else {
                            String::new()
                        };
                        errors.push(format!("永久删除失败 {src}: {e}{hint}"));
                    }
                } else if let Err(e) = trash_path(p) {
                    // 默认移到系统回收站：用户可在回收站中还原误删项。
                    if is_trash_access_denied(&e) {
                        denied.push(src.clone());
                    }
                    // trash crate 在文件被占用时只报 "Some operations were aborted"，
                    // 拿不到 ERROR_SHARING_VIOLATION，因此这里无条件查一次占用进程
                    // （查不到占用者时 locked_hint 返回空串，等于无操作）。
                    let hint = locked_hint(p);
                    errors.push(format!("移到回收站失败 {src}: {e}{hint}"));
                }
                ctx.add_file(0);
                ctx.tick(src);
            }
        }
        FsOp::Rename {
            sources,
            target,
            overwrite,
        } => {
            if target.trim().is_empty() {
                errors.push("Rename 缺少 target".into());
            } else {
                if sources.is_empty() {
                    errors.push("Rename 缺少 sources".into());
                }
                let tp = Path::new(target);
                ctx.set_total(sources.len() as u64, 0);
                ctx.report("run", "", true);
                for src in sources {
                    if ctx.is_cancelled() {
                        cancelled = true;
                        break;
                    }
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
                    let mut dest = if io_path(tp).is_dir() {
                        unique_dest(tp, sp)
                    } else {
                        tp.to_path_buf()
                    };
                    let same = normalize(&dest) == normalize(sp);
                    if !same && io_exists(&dest) {
                        if *overwrite {
                            // 覆盖目标：先删掉旧的，否则 Windows 的 rename 会失败。
                            if let Err(e) = remove_path(&dest) {
                                if is_access_denied(&e) {
                                    denied.push(src.clone());
                                }
                                errors.push(format!("覆盖目标失败 {}: {e}", dest.display()));
                                continue;
                            }
                        } else {
                            dest = unique_dest(dest.parent().unwrap_or(tp), sp);
                        }
                    }
                    if let Some(parent) = dest.parent() {
                        if !parent.as_os_str().is_empty() && !io_exists(parent) {
                            if let Err(e) = fs::create_dir_all(io_path(parent)) {
                                if is_access_denied(&e) {
                                    denied.push(src.clone());
                                }
                                errors.push(format!(
                                    "创建目标目录失败 {}: {e}",
                                    parent.display()
                                ));
                                continue;
                            }
                        }
                    }
                    if let Err(e) = fs::rename(io_path(sp), io_path(&dest)) {
                        if is_access_denied(&e) {
                            denied.push(src.clone());
                        }
                        let hint = if is_sharing_violation(&e) {
                            locked_hint(sp)
                        } else {
                            String::new()
                        };
                        errors.push(format!("重命名失败 {src} -> {}: {e}{hint}", dest.display()));
                    } else {
                        undo.push(rename_undo(&dest, src));
                    }
                    ctx.add_file(0);
                    ctx.tick(src);
                }
            }
        }
        FsOp::Copy {
            sources,
            target,
            conflict,
        }
        | FsOp::Move {
            sources,
            target,
            conflict,
        } => {
            let is_move = matches!(op, FsOp::Move { .. });
            if target.trim().is_empty() {
                errors.push("Copy/Move 缺少 target".into());
            } else {
                if sources.is_empty() {
                    errors.push("Copy/Move 缺少 sources".into());
                }
                let tdir = Path::new(target);
                // 预扫描：给进度条一个分母（同卷 Move 走 rename，无需扫描）。
                let mut files_total = 0u64;
                let mut bytes_total = 0u64;
                let need_scan = !is_move || sources.iter().any(|src| io_path(Path::new(src)).is_dir());
                if need_scan {
                    ctx.report("scan", "", true);
                    for src in sources {
                        let p = Path::new(src);
                        match fs::symlink_metadata(io_path(p)) {
                            Ok(meta) if meta.is_dir() => {
                                if !scan_size(p, ctx, &mut files_total, &mut bytes_total) {
                                    cancelled = true;
                                    break;
                                }
                            }
                            Ok(meta) => {
                                files_total += 1;
                                bytes_total += meta.len();
                            }
                            Err(_) => {}
                        }
                    }
                }
                ctx.set_total(files_total, bytes_total);
                ctx.report("run", "", true);

                if !cancelled {
                    for src in sources {
                        if ctx.is_cancelled() {
                            cancelled = true;
                            break;
                        }
                        if src.trim().is_empty() {
                            errors.push("源路径为空".into());
                            continue;
                        }
                        let sp = Path::new(src);
                        if is_drive_root(sp) {
                            errors.push(format!("已拒绝操作驱动器根目录：{src}"));
                            continue;
                        }
                        if !io_exists(sp) {
                            errors.push(format!("操作失败 {src}: 路径不存在"));
                            continue;
                        }
                        // 防止把目录移动/复制到它自身的子目录（会造成递归或数据丢失）。
                        if io_path(sp).is_dir() && is_subpath(tdir, sp) {
                            errors.push(format!(
                                "已拒绝把目录放入其自身子目录：{src} -> {target}"
                            ));
                            continue;
                        }
                        if let Err(e) = fs::create_dir_all(io_path(tdir)) {
                            if is_access_denied(&e) {
                                denied.push(src.clone());
                            }
                            errors.push(format!("创建目标目录失败 {target}: {e}"));
                            continue;
                        }

                        let name = sp.file_name().map(|s| s.to_os_string()).unwrap_or_default();
                        let plain = tdir.join(&name);
                        let dest = match conflict {
                            ConflictPolicy::KeepBoth => unique_dest(tdir, sp),
                            ConflictPolicy::Skip => {
                                if io_exists(&plain) {
                                    skipped.push(src.clone());
                                    continue;
                                }
                                plain
                            }
                            ConflictPolicy::Replace => {
                                // 目标与来源是同一条路径时不能「覆盖」——那等于删除源。
                                if normalize(&plain) != normalize(sp) && io_exists(&plain) {
                                    if let Err(e) = remove_path(&plain) {
                                        if is_access_denied(&e) {
                                            denied.push(src.clone());
                                        }
                                        errors.push(format!(
                                            "覆盖目标失败 {}: {e}",
                                            plain.display()
                                        ));
                                        continue;
                                    }
                                }
                                plain
                            }
                        };

                        let outcome = if is_move {
                            move_path(sp, &dest, ctx, &mut skipped)
                        } else {
                            copy_recursive(sp, &dest, ctx, &mut skipped).map(|_| true)
                        };

                        match outcome {
                            Ok(source_removed) => {
                                if is_move && source_removed {
                                    if let Some(parent) = sp.parent() {
                                        if !parent.as_os_str().is_empty() {
                                            undo.push(move_undo(&dest, &parent.to_string_lossy()));
                                        }
                                    }
                                } else if !matches!(conflict, ConflictPolicy::Replace) {
                                    // 覆盖模式下原内容已被销毁，无法撤销。
                                    undo.push(delete_undo(&dest.to_string_lossy()));
                                }
                            }
                            Err(CopyFailure::Cancelled) => {
                                // 取消：清掉写了一半的副本，避免留下残缺目录。
                                let _ = remove_path(&dest);
                                cancelled = true;
                                break;
                            }
                            Err(CopyFailure::Io(e)) => {
                                let _ = remove_path(&dest);
                                if is_access_denied(&e) {
                                    denied.push(src.clone());
                                }
                                let hint = if is_sharing_violation(&e) {
                                    locked_hint(sp)
                                } else {
                                    String::new()
                                };
                                errors.push(format!("操作失败 {src}: {e}{hint}"));
                            }
                        }
                        ctx.add_file(0);
                        ctx.tick(src);
                    }
                }
            }
        }
        FsOp::Duplicate { sources } => {
            if sources.is_empty() {
                errors.push("Duplicate 缺少 sources".into());
            }
            for src in sources {
                if ctx.is_cancelled() {
                    cancelled = true;
                    break;
                }
                if src.trim().is_empty() {
                    errors.push("复制副本源路径为空".into());
                    continue;
                }
                let sp = Path::new(src);
                if is_drive_root(sp) {
                    errors.push(format!("已拒绝复制驱动器根目录：{src}"));
                    continue;
                }
                if !io_exists(sp) {
                    errors.push(format!("复制副本失败 {src}: 路径不存在"));
                    continue;
                }
                let dest = unique_duplicate_dest(sp);
                match copy_recursive(sp, &dest, ctx, &mut skipped) {
                    Ok(()) => undo.push(delete_undo(&dest.to_string_lossy())),
                    Err(CopyFailure::Cancelled) => {
                        let _ = remove_path(&dest);
                        cancelled = true;
                        break;
                    }
                    Err(CopyFailure::Io(e)) => {
                        let _ = remove_path(&dest);
                        if is_access_denied(&e) {
                            denied.push(src.clone());
                        }
                        errors.push(format!("复制副本失败 {src}: {e}"));
                    }
                }
            }
        }
    }

    ctx.report("done", "", true);
    FsOpResult {
        ok: errors.is_empty(),
        errors,
        denied,
        undo,
        skipped,
        cancelled,
    }
}

/// 预检同名冲突：Copy/Move 的目标目录里是否已有同名项，Rename 的目标是否已存在。
///
/// 前端在发起操作前调用，若有冲突则弹「替换 / 跳过 / 保留两者」让用户选策略，
/// 避免现在的「永远静默改名」。
pub fn check_conflicts(op: &FsOp) -> Vec<ConflictItem> {
    match op {
        FsOp::Copy { sources, target, .. } | FsOp::Move { sources, target, .. } => {
            let tdir = Path::new(target);
            sources
                .iter()
                .filter_map(|src| {
                    let sp = Path::new(src);
                    let name = sp.file_name()?;
                    let dest = tdir.join(name);
                    if io_exists(&dest) {
                        Some(ConflictItem {
                            path: dest.to_string_lossy().into_owned(),
                            source: src.clone(),
                            is_dir: io_path(&dest).is_dir(),
                        })
                    } else {
                        None
                    }
                })
                .collect()
        }
        FsOp::Rename { sources, target, .. } => {
            let dest = Path::new(target);
            let same = sources
                .iter()
                .any(|src| normalize(Path::new(src)) == normalize(dest));
            if !same && io_exists(dest) {
                vec![ConflictItem {
                    path: target.clone(),
                    source: sources.first().cloned().unwrap_or_default(),
                    is_dir: io_path(dest).is_dir(),
                }]
            } else {
                Vec::new()
            }
        }
        _ => Vec::new(),
    }
}

/// 统计来源的文件数与总字节（进度条分母）。被取消时返回 false。
fn scan_size(src: &Path, ctx: &OpContext, files: &mut u64, bytes: &mut u64) -> bool {
    let mut count = 0u64;
    let walker = WalkDir::new(io_path(src))
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| !is_reparse_dir_entry(entry));
    for entry in walker {
        if count % 256 == 0 && ctx.is_cancelled() {
            return false;
        }
        count += 1;
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_file() {
            continue;
        }
        *files += 1;
        if let Ok(meta) = entry.metadata() {
            *bytes += meta.len();
        }
        ctx.report("scan", &entry.path().to_string_lossy(), false);
    }
    true
}

/// 复制失败的原因：IO 错误或用户取消。
enum CopyFailure {
    Io(std::io::Error),
    Cancelled,
}

impl From<std::io::Error> for CopyFailure {
    fn from(error: std::io::Error) -> Self {
        CopyFailure::Io(error)
    }
}

/// 删除某条路径（移到回收站）——用于撤销「新建」与「复制」。
fn delete_undo(path: &str) -> FsOp {
    FsOp::Delete {
        sources: vec![path.to_string()],
        permanent: false,
    }
}

/// 把 dest 改回 src——用于撤销「重命名」。
fn rename_undo(dest: &Path, src: &str) -> FsOp {
    FsOp::Rename {
        sources: vec![dest.to_string_lossy().into_owned()],
        target: src.to_string(),
        overwrite: true,
    }
}

/// 把 dest 移回 parent——用于撤销「移动」。
fn move_undo(dest: &Path, parent: &str) -> FsOp {
    FsOp::Move {
        sources: vec![dest.to_string_lossy().into_owned()],
        target: parent.to_string(),
        conflict: ConflictPolicy::KeepBoth,
    }
}

/// 是否为 Windows「拒绝访问」错误（ERROR_ACCESS_DENIED = os error 5）。
///
/// 只有此错误才值得 UAC 提权重试。其它容易混淆的错误码都**不**映射为 5，故不会被误判：
/// 文件被占用 ERROR_SHARING_VIOLATION(32)、目录非空 ERROR_DIR_NOT_EMPTY(145)、
/// 权限不足 ERROR_PRIVILEGE_NOT_HELD(1314)、网络拒绝 WSAEACCES(10013)。
fn is_access_denied(e: &std::io::Error) -> bool {
    e.raw_os_error() == Some(5)
}

/// 「文件被占用」的提示后缀，形如 `（被 a.exe、b.exe 占用）`。
///
/// 调用方只在错误码是 ERROR_SHARING_VIOLATION（os error 32）时才调它；
/// 查不到占用进程（非 Windows、Restart Manager 失败、无占用者）时返回空串，
/// 拼到错误文案末尾即为无副作用的空操作。
fn locked_hint(path: &Path) -> String {
    let names = crate::file_locks::locking_processes(&path.to_string_lossy());
    if names.is_empty() {
        String::new()
    } else {
        format!("（被 {} 占用）", names.join("、"))
    }
}

/// 是否值得去查占用进程：只有 ERROR_SHARING_VIOLATION 是「被别的程序占着」。
fn is_sharing_violation(e: &std::io::Error) -> bool {
    e.raw_os_error() == Some(32)
}

// ---------------------------------------------------------------- 内部工具

/// 词法规范化路径：解析掉 . 与 ..，**不访问文件系统**（目标可不存在）。
///
/// 之所以需要它：C:\Windows\.. 的组件是 [Prefix, RootDir, Normal, ParentDir]，
/// 直接按组件判定会误认为「非根」而放行删除，从而递归清空整个卷。规范化后
/// C:\Windows\.. 归一为 C:\，可被正确拦截。.. 不会越过卷根
/// （C:\Windows\.. -> C:\），前缀 / 根 / 盘符原样保留。
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
                    // 相对路径开头的 .. 无处可归，保留原样。
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

/// 是否为驱动器根（C:\ 或 C: 之类），这类路径禁止删除 / 移动 / 重命名。
///
/// 判定前先做词法规范化，避免 C:\Windows\.. 这类路径绕过保护。
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

/// child 是否位于 ancestor 之下（对两者做**词法**规范化后比较包含关系）。
///
/// 不用 canonicalize：调用点检查的是**尚未创建**的目标目录，canonicalize
/// 失败退回原路径导致 starts_with 失配，进而放行「移动到自身子目录」并触发
/// 无限递归。词法规范化对不存在的路径同样有效。
fn is_subpath(child: &Path, ancestor: &Path) -> bool {
    let c = normalize(child);
    let a = normalize(ancestor);
    c != a && c.starts_with(&a)
}

/// 计算目标路径；若同名已存在则自动改为 name (1).ext / name (1)。
fn unique_dest(dir: &Path, src: &Path) -> PathBuf {
    let name = src
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let dest = dir.join(&name);
    if !io_exists(&dest) {
        return dest;
    }
    let (stem, ext) = if io_path(src).is_dir() {
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
        if !io_exists(&d) {
            return d;
        }
    }
    dest
}

/// 生成同目录副本的目标名：原名 - 副本.ext，重复时追加 (2)、(3)…
fn unique_duplicate_dest(src: &Path) -> PathBuf {
    let dir = src.parent().unwrap_or_else(|| Path::new(""));
    let name = src
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let (stem, ext) = if io_path(src).is_dir() {
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
        if !io_exists(&dest) {
            return dest;
        }
    }
    dir.join(format!("{stem} - 副本"))
}

/// 移动：优先 rename；失败（常见于跨盘）时回退为「复制 + 删除」。
///
/// 返回值表示**源是否已被删除**：
/// - Ok(true)：完全移动（rename 成功，或复制成功且源已删）。
/// - Ok(false)：复制成功但源删不掉（文件被占用等）——此时副本完整，
///   调用方按「复制」语义记录撤销，并把删除失败报给用户。
/// - Err：复制中途失败/取消——调用方会清掉写了一半的副本。
fn move_path(
    src: &Path,
    dest: &Path,
    ctx: &OpContext,
    skipped: &mut Vec<String>,
) -> Result<bool, CopyFailure> {
    match fs::rename(io_path(src), io_path(dest)) {
        Ok(_) => {
            if let Ok(meta) = fs::symlink_metadata(io_path(src)) {
                ctx.add_file(if meta.is_dir() { 0 } else { meta.len() });
            }
            Ok(true)
        }
        Err(_) => {
            copy_recursive(src, dest, ctx, skipped)?;
            match remove_path(src) {
                Ok(_) => Ok(true),
                // 源删不掉：保留完整副本（不删），由调用方报告并允许撤销。
                Err(_) => Ok(false),
            }
        }
    }
}

/// 把单个路径移到系统回收站。
///
/// trash crate 在 Windows 上走 IFileOperation（回收站），
/// macOS / Linux 分别走废纸篓与 Freedesktop Trash 规范。
pub(crate) fn trash_path(p: &Path) -> Result<(), trash::Error> {
    trash::delete(p)
}

/// 回收站操作是否属于权限不足（可触发 UAC 提权重试）。
///
/// Windows 下多为 HRESULT_FROM_WIN32(ERROR_ACCESS_DENIED) = 0x80070005；
/// CouldNotAccess 表示路径不存在或当前进程无权读取。
fn is_trash_access_denied(e: &trash::Error) -> bool {
    match e {
        trash::Error::Os { code, .. } => *code as u32 == 0x8007_0005,
        trash::Error::CouldNotAccess { .. } => true,
        _ => false,
    }
}

/// 直接物理删除（仅用于 Move 跨盘回退等内部清理，不面向用户的删除操作）。
fn remove_path(p: &Path) -> std::io::Result<()> {
    let io = io_path(p);
    if io.is_dir() {
        fs::remove_dir_all(io)
    } else {
        fs::remove_file(io)
    }
}

/// 递归复制。
///
/// - 跳过符号链接 / junction（见 is_reparse_point），避免重复复制与目录环。
/// - 每个文件前检查取消标记；取消返回 CopyFailure::Cancelled。
/// - 目标已存在的目录会被复用（合并语义），文件由 fs::copy 覆盖。
fn copy_recursive(
    src: &Path,
    dest: &Path,
    ctx: &OpContext,
    skipped: &mut Vec<String>,
) -> Result<(), CopyFailure> {
    if ctx.is_cancelled() {
        return Err(CopyFailure::Cancelled);
    }
    let meta = fs::symlink_metadata(io_path(src))?;
    if is_reparse_point(&meta) {
        skipped.push(src.to_string_lossy().into_owned());
        return Ok(());
    }
    if meta.is_dir() {
        fs::create_dir_all(io_path(dest))?;
        for item in fs::read_dir(io_path(src))? {
            let item = item?;
            if ctx.is_cancelled() {
                return Err(CopyFailure::Cancelled);
            }
            let child_src = src.join(item.file_name());
            let child_dest = dest.join(item.file_name());
            copy_recursive(&child_src, &child_dest, ctx, skipped)?;
        }
        Ok(())
    } else {
        if let Some(parent) = dest.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(io_path(parent))?;
            }
        }
        fs::copy(io_path(src), io_path(dest))?;
        ctx.add_file(meta.len());
        ctx.tick(&src.to_string_lossy());
        Ok(())
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("stelix-fs-core-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn run(op: &FsOp) -> FsOpResult {
        execute(op, &OpContext::noop())
    }

    /// 地址栏输入：%VAR% / ~ / 引号 / 正斜杠都要能落到真实路径。
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
        // ~ 只在路径开头且后面紧跟分隔符时才是主目录。
        assert_eq!(expand_path("~foo"), "~foo");
    }

    #[test]
    fn create_file_creates_empty_file() {
        let dir = temp_dir("create-file");
        let target = dir.join("hello.txt");
        let res = run(&FsOp::CreateFile {
            target: target.to_string_lossy().into_owned(),
        });
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
        let res = run(&FsOp::CreateFile {
            target: target.to_string_lossy().into_owned(),
        });
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

        let res = run(&FsOp::Rename {
            sources: vec![source.to_string_lossy().into_owned()],
            target: renamed.to_string_lossy().into_owned(),
            overwrite: false,
        });
        assert!(res.ok, "{:?}", res.errors);
        assert_eq!(res.undo.len(), 1);
        assert!(matches!(res.undo[0], FsOp::Rename { .. }));
        assert_eq!(
            res.undo[0].sources(),
            vec![renamed.to_string_lossy().to_string()]
        );
        assert_eq!(
            res.undo[0].target(),
            Some(source.to_string_lossy().as_ref())
        );

        let copies = dir.join("copies");
        let res = run(&FsOp::Copy {
            sources: vec![renamed.to_string_lossy().into_owned()],
            target: copies.to_string_lossy().into_owned(),
            conflict: ConflictPolicy::KeepBoth,
        });
        assert!(res.ok, "{:?}", res.errors);
        assert_eq!(res.undo.len(), 1);
        assert!(matches!(res.undo[0], FsOp::Delete { .. }));
        assert_eq!(
            res.undo[0].sources(),
            vec![copies.join("b.txt").to_string_lossy().to_string()]
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn duplicate_creates_copy_with_chinese_suffix() {
        let dir = temp_dir("duplicate");
        let file = dir.join("report.txt");
        fs::write(&file, b"data").unwrap();
        let res = run(&FsOp::Duplicate {
            sources: vec![file.to_string_lossy().into_owned()],
        });
        assert!(res.ok, "{:?}", res.errors);
        assert_eq!(fs::read(dir.join("report - 副本.txt")).unwrap(), b"data");
        // 再次副本：追加序号，且原文件保持不变。
        assert!(run(&FsOp::Duplicate {
            sources: vec![file.to_string_lossy().into_owned()],
        })
        .ok);
        assert!(dir.join("report - 副本 (2).txt").is_file());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn mkdir_reports_existing_name() {
        let dir = temp_dir("mkdir-dup");
        let target = dir.join("sub");
        fs::create_dir(&target).unwrap();
        let res = run(&FsOp::Mkdir {
            targets: vec![target.to_string_lossy().into_owned()],
        });
        assert!(!res.ok);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn mkdir_creates_multiple_targets() {
        let dir = temp_dir("mkdir-batch");
        let first = dir.join("first");
        let second = dir.join("second");
        let existing = dir.join("existing");
        fs::create_dir(&existing).unwrap();
        let res = run(&FsOp::Mkdir {
            targets: vec![
                first.to_string_lossy().into_owned(),
                second.to_string_lossy().into_owned(),
                existing.to_string_lossy().into_owned(),
            ],
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
        assert!(run(&FsOp::Mkdir {
            targets: vec![target.to_string_lossy().into_owned()],
        })
        .ok);

        let res = run(&FsOp::Move {
            sources: vec![source.to_string_lossy().into_owned()],
            target: target.to_string_lossy().into_owned(),
            conflict: ConflictPolicy::KeepBoth,
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
        let res = run(&FsOp::Delete {
            sources: vec![file.to_string_lossy().into_owned()],
            permanent: true,
        });
        assert!(res.ok, "{:?}", res.errors);
        assert!(!file.exists(), "永久删除后文件不应存在");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn result_serializes_denied_field() {
        let res = run(&FsOp::Delete {
            sources: vec![String::new()],
            permanent: false,
        });
        let json = serde_json::to_string(&res).unwrap();
        assert!(
            json.contains("\"denied\""),
            "序列化结果应含 denied 字段: {json}"
        );
        assert!(json.contains("\"skipped\""), "序列化结果应含 skipped 字段");
        // 普通失败（源路径为空）不进入 denied。
        assert!(!res.ok);
        assert!(res.denied.is_empty());
    }

    /// BUG A：C:\Windows\.. 归一后即为驱动器根，必须被拦截；
    /// 普通目录不能被误判为根。
    #[cfg(windows)]
    #[test]
    fn drive_root_detection_resolves_parent_dir() {
        assert!(is_drive_root(Path::new(r"C:\")));
        assert!(is_drive_root(Path::new(r"C:")));
        assert!(is_drive_root(Path::new(r"C:\Windows\..")));
        assert!(is_drive_root(Path::new(r"C:\Windows\System32\..\..")));
        // 普通目录（含解析 .. 后仍非根的路径）不应被误判。
        assert!(!is_drive_root(Path::new(r"C:\Windows")));
        assert!(!is_drive_root(Path::new(r"C:\Windows\..\Users")));
    }

    /// BUG B：目标（C:\a\b\c）尚不存在时也要判定为子路径；
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
        let res = run(&FsOp::Copy {
            sources: vec![src.to_string_lossy().into_owned()],
            target: target.to_string_lossy().into_owned(),
            conflict: ConflictPolicy::KeepBoth,
        });
        assert!(!res.ok, "应拒绝把目录复制到自身子目录: {:?}", res.errors);
        assert!(
            res.errors.iter().any(|e| e.contains("自身子目录")),
            "{:?}",
            res.errors
        );
        assert!(!target.exists(), "被拒绝的目标目录不应被创建");
        let _ = fs::remove_dir_all(&dir);
    }

    // ---------------------------------------------------------------- 新增能力

    /// 冲突策略：Skip 不覆盖、Replace 覆盖、KeepBoth 改名。
    #[test]
    fn conflict_policy_controls_overwrite() {
        let dir = temp_dir("conflict-policy");
        let src = dir.join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("a.txt"), b"new").unwrap();
        let target = dir.join("dst");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("a.txt"), b"old").unwrap();

        // Skip：目标保持原样，来源被记为 skipped。
        let res = run(&FsOp::Copy {
            sources: vec![src.join("a.txt").to_string_lossy().into_owned()],
            target: target.to_string_lossy().into_owned(),
            conflict: ConflictPolicy::Skip,
        });
        assert!(res.ok, "{:?}", res.errors);
        assert_eq!(fs::read(target.join("a.txt")).unwrap(), b"old");
        assert_eq!(res.skipped.len(), 1);

        // Replace：目标被覆盖。
        let res = run(&FsOp::Copy {
            sources: vec![src.join("a.txt").to_string_lossy().into_owned()],
            target: target.to_string_lossy().into_owned(),
            conflict: ConflictPolicy::Replace,
        });
        assert!(res.ok, "{:?}", res.errors);
        assert_eq!(fs::read(target.join("a.txt")).unwrap(), b"new");

        // KeepBoth：生成 a (1).txt。
        let res = run(&FsOp::Copy {
            sources: vec![src.join("a.txt").to_string_lossy().into_owned()],
            target: target.to_string_lossy().into_owned(),
            conflict: ConflictPolicy::KeepBoth,
        });
        assert!(res.ok, "{:?}", res.errors);
        assert!(target.join("a (1).txt").is_file());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn check_conflicts_reports_existing_target() {
        let dir = temp_dir("check-conflicts");
        let src = dir.join("a.txt");
        fs::write(&src, b"x").unwrap();
        let target = dir.join("dst");
        fs::create_dir_all(&target).unwrap();

        let none = check_conflicts(&FsOp::Copy {
            sources: vec![src.to_string_lossy().into_owned()],
            target: target.to_string_lossy().into_owned(),
            conflict: ConflictPolicy::KeepBoth,
        });
        assert!(none.is_empty());

        fs::write(target.join("a.txt"), b"y").unwrap();
        let hit = check_conflicts(&FsOp::Copy {
            sources: vec![src.to_string_lossy().into_owned()],
            target: target.to_string_lossy().into_owned(),
            conflict: ConflictPolicy::KeepBoth,
        });
        assert_eq!(hit.len(), 1);
        assert!(!hit[0].is_dir);
        let _ = fs::remove_dir_all(&dir);
    }

    /// 取消标记一旦置位，复制立即停止并标记 cancelled，且不留下写了一半的副本。
    #[test]
    fn cancelled_copy_stops_and_cleans_partial_dest() {
        let dir = temp_dir("cancel-copy");
        let src = dir.join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("a.txt"), b"a").unwrap();
        let target = dir.join("dst");

        let cancel = Arc::new(AtomicBool::new(true));
        let ctx = OpContext::new("t1".into(), "copy", cancel, None);
        let res = execute(
            &FsOp::Copy {
                sources: vec![src.to_string_lossy().into_owned()],
                target: target.to_string_lossy().into_owned(),
                conflict: ConflictPolicy::KeepBoth,
            },
            &ctx,
        );
        assert!(res.cancelled, "应标记为已取消: {:?}", res.errors);
        assert!(!target.join("a.txt").exists(), "不应留下副本");
        let _ = fs::remove_dir_all(&dir);
    }

    /// 进度回调至少能收到一次 force 上报（开始 / 结束）。
    #[test]
    fn progress_sink_receives_events() {
        let dir = temp_dir("progress");
        let src = dir.join("a.txt");
        fs::write(&src, b"abc").unwrap();
        let target = dir.join("dst");

        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let sink_seen = seen.clone();
        let sink: ProgressSink = Arc::new(move |p: FsProgress| {
            sink_seen.lock().unwrap().push(p.phase);
        });
        let ctx = OpContext::new(
            "p1".into(),
            "copy",
            Arc::new(AtomicBool::new(false)),
            Some(sink),
        );
        let res = execute(
            &FsOp::Copy {
                sources: vec![src.to_string_lossy().into_owned()],
                target: target.to_string_lossy().into_owned(),
                conflict: ConflictPolicy::KeepBoth,
            },
            &ctx,
        );
        assert!(res.ok, "{:?}", res.errors);
        let phases = seen.lock().unwrap().clone();
        assert!(phases.contains(&"run".to_string()), "{phases:?}");
        assert!(phases.contains(&"done".to_string()), "{phases:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// 复制目录时跳过 junction，避免重复复制与目录环。
    #[cfg(windows)]
    #[test]
    fn copy_skips_junction_inside_tree() {
        let dir = temp_dir("copy-junction");
        let src = dir.join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("a.txt"), b"a").unwrap();
        // 用 mklink /J 造一个 junction（不需要管理员权限），指向父目录形成环。
        let junction = src.join("loop");
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&junction)
            .arg(&dir)
            .status();
        let Ok(status) = status else { return };
        if !status.success() {
            // 环境不支持建 junction 时跳过（不算失败）。
            let _ = fs::remove_dir_all(&dir);
            return;
        }
        let target = dir.join("dst");
        let res = run(&FsOp::Copy {
            sources: vec![src.to_string_lossy().into_owned()],
            target: target.to_string_lossy().into_owned(),
            conflict: ConflictPolicy::KeepBoth,
        });
        assert!(res.ok, "{:?}", res.errors);
        assert!(target.join("src").join("a.txt").is_file());
        assert!(
            !target.join("src").join("loop").exists(),
            "junction 不应被复制（会形成环）"
        );
        assert!(!res.skipped.is_empty(), "应记录被跳过的链接");
        let _ = fs::remove_dir_all(&dir);
    }

    /// 长路径：io_path 对绝对路径加 verbatim 前缀，且幂等。
    #[cfg(windows)]
    #[test]
    fn io_path_adds_verbatim_prefix_idempotently() {
        let p = io_path(Path::new(r"C:\Windows\System32"));
        let text = p.to_string_lossy();
        assert!(text.starts_with(r"\\?\C:\"), "{text}");
        assert_eq!(io_path(&p), p, "重复调用不应叠加前缀");
        // 相对路径原样返回。
        assert_eq!(io_path(Path::new("a\\b")), PathBuf::from("a\\b"));
    }

    /// 超长路径（> MAX_PATH）可正常新建与删除。
    #[cfg(windows)]
    #[test]
    fn long_path_roundtrip() {
        let dir = temp_dir("long-path");
        let mut deep = dir.clone();
        for i in 0..12 {
            deep.push(format!("segment-{i:02}-padding-padding"));
        }
        let target = deep.join("file.txt");
        assert!(target.to_string_lossy().len() > 260, "测试路径应超过 MAX_PATH");
        let res = run(&FsOp::Mkdir {
            targets: vec![deep.to_string_lossy().into_owned()],
        });
        assert!(res.ok, "{:?}", res.errors);
        let res = run(&FsOp::CreateFile {
            target: target.to_string_lossy().into_owned(),
        });
        assert!(res.ok, "{:?}", res.errors);
        assert!(io_exists(&target));
        let res = run(&FsOp::Delete {
            sources: vec![target.to_string_lossy().into_owned()],
            permanent: true,
        });
        assert!(res.ok, "{:?}", res.errors);
        assert!(!io_exists(&target));
        let _ = fs::remove_dir_all(&dir);
    }
}

//! 目录变更监听：为「当前打开的目录集合」挂 `notify` watcher，
//! 事件经防抖后以 `fs:changed` 事件上报（payload = 内容发生变化的目录列表）。
//!
//! 设计要点：
//! - 只监听**本地盘符目录**（`X:\…`），UNC / 网络路径直接跳过。
//! - `NonRecursive`：监听目录 X 时，X 的**直接子项**增删改都会产生事件。
//! - 防抖 250ms：把一批连续操作（如复制多个文件）合并成一次上报。
//! - 上报的是**父目录**：前端按目录匹配所有打开的标签与目录树节点。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

/// 事件名，与前端 `events.onFsChanged` 对应。
pub const FS_CHANGED_EVENT: &str = "fs:changed";

/// 最后一次事件之后静默这么久才上报（防抖窗口）。
const DEBOUNCE: Duration = Duration::from_millis(250);
/// flush 线程的轮询间隔。
const POLL: Duration = Duration::from_millis(120);

/// `fs:changed` 事件负载。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FsChanged {
    /// 内容发生变化的目录（已去重）。
    pub dirs: Vec<String>,
}

/// 待上报的目录 + 最近一次事件时间。
type Pending = Arc<Mutex<(HashSet<String>, Option<Instant>)>>;

/// 目录监听服务：持有 watcher 与监听集合，全局单例（存于 `AppState`）。
pub struct WatchService {
    watcher: Mutex<RecommendedWatcher>,
    /// 当前已挂监听的目录集合。
    watched: Mutex<HashSet<PathBuf>>,
    pending: Pending,
}

impl WatchService {
    pub fn new() -> Self {
        let pending: Pending = Arc::new(Mutex::new((HashSet::new(), None)));
        let sink = Arc::clone(&pending);
        let watcher = notify::recommended_watcher(move |result: notify::Result<Event>| {
            let Ok(event) = result else { return };
            if !is_relevant(&event.kind) {
                return;
            }
            let mut guard = sink.lock();
            for path in &event.paths {
                if let Some(dir) = affected_dir(path) {
                    guard.0.insert(dir);
                }
            }
            if !guard.0.is_empty() {
                guard.1 = Some(Instant::now());
            }
        })
        .expect("初始化文件系统 watcher 失败");

        Self {
            watcher: Mutex::new(watcher),
            watched: Mutex::new(HashSet::new()),
            pending,
        }
    }

    /// 全量设置需要监听的目录集合（幂等；前端每次上报完整集合）。
    pub fn set_dirs(&self, dirs: &[String]) {
        let want: HashSet<PathBuf> = dirs
            .iter()
            .map(PathBuf::from)
            .filter(|path| is_watchable(path))
            .collect();

        let mut watched = self.watched.lock();
        let mut watcher = self.watcher.lock();

        for old in watched.iter() {
            if !want.contains(old) {
                let _ = watcher.unwatch(old);
            }
        }
        for new in want.iter() {
            if !watched.contains(new) {
                // 单个目录挂载失败（权限 / 刚被删除）不影响其余目录。
                let _ = watcher.watch(new, RecursiveMode::NonRecursive);
            }
        }
        *watched = want;
    }

    /// 启动 flush 线程：把防抖后的变更批量 emit 给前端。
    pub fn start_flush_loop(&self, app: AppHandle) {
        let pending = Arc::clone(&self.pending);
        std::thread::spawn(move || loop {
            std::thread::sleep(POLL);
            let dirs = {
                let mut guard = pending.lock();
                let due = guard.1.map(|at| at.elapsed() >= DEBOUNCE).unwrap_or(false);
                if due && !guard.0.is_empty() {
                    let batch: Vec<String> = guard.0.drain().collect();
                    guard.1 = None;
                    Some(batch)
                } else {
                    None
                }
            };
            if let Some(dirs) = dirs {
                let _ = app.emit(FS_CHANGED_EVENT, FsChanged { dirs });
            }
        });
    }
}

impl Default for WatchService {
    fn default() -> Self {
        Self::new()
    }
}

/// 是否是我们关心的变更（排除纯粹的读取 / 属性访问事件）。
fn is_relevant(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_)
            | EventKind::Modify(_)
            | EventKind::Remove(_)
            | EventKind::Any
            | EventKind::Other
    )
}

/// 事件路径所属的目录（父目录）。驱动器根没有父目录，返回 `None`。
fn affected_dir(path: &Path) -> Option<String> {
    let parent = path.parent()?;
    if parent.as_os_str().is_empty() {
        return None;
    }
    Some(parent.to_string_lossy().to_string())
}

/// 只监听本地绝对目录（Windows 上限定 `X:\` 盘符，类 Unix 上接受 `/` 开头的目录）。
fn is_watchable(path: &Path) -> bool {
    if !path.is_absolute() || !path.is_dir() {
        return false;
    }
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        let bytes = text.as_bytes();
        bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && (bytes[2] == b'\\' || bytes[2] == b'/')
    }
    #[cfg(not(windows))]
    {
        true
    }
}

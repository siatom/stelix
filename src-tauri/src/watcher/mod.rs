//! 目录变更监听：为「当前打开的目录集合」挂 `notify` watcher，
//! 事件经防抖后以 `fs:changed` 事件上报（payload = 内容发生变化的目录列表）。
//!
//! 设计要点：
//! - Windows 只监听**本地盘符目录**（`X:\…`），UNC / 网络路径直接跳过。
//! - `NonRecursive`：监听目录 X 时，X 的**直接子项**增删改都会产生事件。
//! - 防抖 250ms：把一批连续操作（如复制多个文件）合并成一次上报。
//! - 上报的是**父目录**：前端按目录匹配所有打开的标签与目录树节点。

use std::collections::{HashMap, HashSet};
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
const INITIAL_RETRY_DELAY: Duration = Duration::from_secs(1);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(30);

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
    state: Arc<Mutex<WatchState<RecommendedWatcher>>>,
    pending: Pending,
}

struct WatchState<W> {
    watcher: W,
    watched: HashSet<PathBuf>,
    // 期望集合由已注册目录和等待重试的目录组成。
    retries: HashMap<PathBuf, WatchRetry>,
}

struct WatchRetry {
    next_attempt: Instant,
    delay: Duration,
    last_error: String,
}

impl<W: Watcher> WatchState<W> {
    fn new(watcher: W) -> Self {
        Self {
            watcher,
            watched: HashSet::new(),
            retries: HashMap::new(),
        }
    }

    fn set_dirs(&mut self, want: HashSet<PathBuf>, now: Instant) {
        self.retries.retain(|path, _| want.contains(path));
        self.watched.retain(|path| {
            if want.contains(path) {
                return true;
            }
            let _ = self.watcher.unwatch(path);
            false
        });

        for path in want {
            if !self.watched.contains(&path) && !self.retries.contains_key(&path) {
                self.register(path, now);
            }
        }
    }

    fn retry_due(&mut self, now: Instant) {
        let due: Vec<_> = self
            .retries
            .iter()
            .filter(|(_, retry)| now >= retry.next_attempt)
            .map(|(path, _)| path.clone())
            .collect();
        for path in due {
            self.register(path, now);
        }
    }

    fn register(&mut self, path: PathBuf, now: Instant) {
        let previous = self.retries.remove(&path);
        let result = std::fs::metadata(&path)
            .map_err(notify::Error::io)
            .and_then(|metadata| {
                if metadata.is_dir() {
                    self.watcher.watch(&path, RecursiveMode::NonRecursive)
                } else {
                    Err(notify::Error::generic("目标不是目录"))
                }
            });

        match result {
            Ok(()) => {
                if let Some(retry) = previous {
                    eprintln!(
                        "目录监听已恢复（{}），上次失败原因：{}",
                        path.display(),
                        retry.last_error
                    );
                }
                self.watched.insert(path);
            }
            Err(error) => {
                let delay = match previous {
                    Some(retry) => (retry.delay * 2).min(MAX_RETRY_DELAY),
                    None => {
                        eprintln!("目录监听注册失败（{}）：{error}", path.display());
                        INITIAL_RETRY_DELAY
                    }
                };
                self.retries.insert(
                    path,
                    WatchRetry {
                        next_attempt: now + delay,
                        delay,
                        last_error: error.to_string(),
                    },
                );
            }
        }
    }
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
            state: Arc::new(Mutex::new(WatchState::new(watcher))),
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

        self.state.lock().set_dirs(want, Instant::now());
    }

    /// 启动 flush 线程：把防抖后的变更批量 emit 给前端。
    pub fn start_flush_loop(&self, app: AppHandle) {
        let pending = Arc::clone(&self.pending);
        let state = Arc::clone(&self.state);
        std::thread::spawn(move || loop {
            std::thread::sleep(POLL);
            state.lock().retry_due(Instant::now());
            if let Some(change) = take_due_changes(&pending) {
                let _ = app.emit(FS_CHANGED_EVENT, change);
            }
        });
    }
}

impl Default for WatchService {
    fn default() -> Self {
        Self::new()
    }
}

fn take_due_changes(pending: &Pending) -> Option<FsChanged> {
    let mut guard = pending.lock();
    let due = guard.1.is_some_and(|at| at.elapsed() >= DEBOUNCE);
    if !due || guard.0.is_empty() {
        return None;
    }
    guard.1 = None;
    Some(FsChanged {
        dirs: guard.0.drain().collect(),
    })
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
    if !path.is_absolute() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[derive(Default)]
    struct TestWatcher {
        failures: HashSet<PathBuf>,
        attempts: HashMap<PathBuf, usize>,
    }

    impl Watcher for TestWatcher {
        fn new<F: notify::EventHandler>(_: F, _: notify::Config) -> notify::Result<Self> {
            Ok(Self::default())
        }

        fn watch(&mut self, path: &Path, _: RecursiveMode) -> notify::Result<()> {
            *self.attempts.entry(path.to_owned()).or_default() += 1;
            if self.failures.contains(path) {
                return Err(notify::Error::generic("injected registration failure"));
            }
            Ok(())
        }

        fn unwatch(&mut self, _: &Path) -> notify::Result<()> {
            Ok(())
        }

        fn kind() -> notify::WatcherKind {
            notify::WatcherKind::NullWatcher
        }
    }

    #[test]
    fn retries_failed_registration_without_reregistering_healthy_paths() {
        let path_a = std::env::temp_dir();
        let path_b = std::env::current_dir().unwrap();
        let mut state = WatchState::new(TestWatcher::default());
        state.watcher.failures.insert(path_a.clone());
        let want = HashSet::from([path_a.clone(), path_b.clone()]);
        let now = Instant::now();

        state.set_dirs(want.clone(), now);
        assert_eq!(state.watched, HashSet::from([path_b.clone()]));
        assert!(state.retries[&path_a].last_error.contains("injected"));
        state.watcher.failures.clear();
        state.set_dirs(want.clone(), now);
        state.retry_due(now + INITIAL_RETRY_DELAY / 2);
        assert_eq!(state.watcher.attempts[&path_a], 1);

        state.retry_due(now + INITIAL_RETRY_DELAY);
        assert_eq!(state.watched, want);
        assert!(state.retries.is_empty());
        assert_eq!(state.watcher.attempts[&path_a], 2);
        assert_eq!(state.watcher.attempts[&path_b], 1);
    }

    #[test]
    fn caps_retry_backoff_and_cancels_removed_paths() {
        let path = std::env::temp_dir();
        let mut state = WatchState::new(TestWatcher::default());
        state.watcher.failures.insert(path.clone());
        state.set_dirs(HashSet::from([path.clone()]), Instant::now());

        for (index, delay_secs) in [1, 2, 4, 8, 16, 30, 30].into_iter().enumerate() {
            let retry = &state.retries[&path];
            let due = retry.next_attempt;
            assert_eq!(retry.delay, Duration::from_secs(delay_secs));
            state.retry_due(due - Duration::from_millis(1));
            assert_eq!(state.watcher.attempts[&path], index + 1);
            state.retry_due(due);
            assert_eq!(state.watcher.attempts[&path], index + 2);
        }
        let due = state.retries[&path].next_attempt;
        state.set_dirs(HashSet::new(), due - Duration::from_millis(1));
        state.retry_due(due);
        assert!(state.retries.is_empty());
        assert!(state.watched.is_empty());
        assert_eq!(state.watcher.attempts[&path], 8);
    }

    #[test]
    fn native_watcher_reports_create_and_remove_after_recovery() {
        struct TempDir(PathBuf);
        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }

        let root = TempDir(std::env::temp_dir().join(uuid::Uuid::new_v4().to_string()));
        fs::create_dir(&root.0).unwrap();
        // On macOS, notify may report the resolved path behind the temp directory.
        #[cfg(not(windows))]
        let dir = root.0.canonicalize().unwrap().join("watched");
        #[cfg(windows)]
        let dir = root.0.join("watched");
        let service = WatchService::new();
        service.set_dirs(&[dir.to_string_lossy().into_owned()]);
        let due = service.state.lock().retries[&dir].next_attempt;
        fs::create_dir(&dir).unwrap();
        service.state.lock().retry_due(due);
        assert!(service.state.lock().watched.contains(&dir));

        let wait_for_change = || {
            let deadline = Instant::now() + Duration::from_secs(8);
            loop {
                if let Some(change) = take_due_changes(&service.pending) {
                    assert!(change.dirs.contains(&dir.to_string_lossy().into_owned()));
                    return;
                }
                assert!(Instant::now() < deadline, "no debounced change for {dir:?}");
                std::thread::sleep(POLL);
            }
        };

        let file = dir.join("child");
        fs::write(&file, b"test").unwrap();
        wait_for_change();
        fs::remove_file(file).unwrap();
        wait_for_change();
    }
}

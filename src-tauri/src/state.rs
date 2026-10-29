//! 全局应用状态。由 Phase 0 拥有，各 workstream 只使用自己相关的字段。

use std::sync::Arc;

use crate::ai_agent::AiService;
use crate::dir_size::DirSizeService;
use crate::index_engine::IndexStore;
use crate::schedule::ScheduleService;
use crate::shell_icon::IconCache;
use crate::ssh::SshManager;
use crate::watcher::WatchService;

pub struct AppState {
    /// WS-A 拥有其内部实现
    pub index: Arc<IndexStore>,
    /// WS-B 拥有其内部实现
    pub ai: Arc<AiService>,
    /// Windows 系统图标缓存（按图标种类缓存，见 shell_icon.rs）
    pub icons: Arc<IconCache>,
    /// 定时任务服务（规格存储 + 后台调度，见 schedule/mod.rs）
    pub tasks: Arc<ScheduleService>,
    /// 目录变更监听（notify watcher + 防抖上报，见 watcher/mod.rs）
    pub watcher: Arc<WatchService>,
    /// 目录大小统计（详情视图按需计算，见 dir_size/mod.rs）
    pub dir_sizes: Arc<DirSizeService>,
    /// SSH 终端会话（见 ssh.rs）
    pub ssh: Arc<SshManager>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            index: Arc::new(IndexStore::new()),
            ai: Arc::new(AiService::new()),
            icons: Arc::new(IconCache::new()),
            tasks: Arc::new(ScheduleService::new()),
            watcher: Arc::new(WatchService::new()),
            dir_sizes: Arc::new(DirSizeService::new()),
            ssh: Arc::new(SshManager::new()),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

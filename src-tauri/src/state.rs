//! 全局应用状态。由 Phase 0 拥有，各 workstream 只使用自己相关的字段。

use std::sync::Arc;

use crate::ai_agent::AiService;
use crate::index_engine::IndexStore;
use crate::schedule::ScheduleService;
use crate::shell_icon::IconCache;
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
}

impl AppState {
    pub fn new() -> Self {
        Self {
            index: Arc::new(IndexStore::new()),
            ai: Arc::new(AiService::new()),
            icons: Arc::new(IconCache::new()),
            tasks: Arc::new(ScheduleService::new()),
            watcher: Arc::new(WatchService::new()),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

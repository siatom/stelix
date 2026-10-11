//! 全局应用状态。由 Phase 0 拥有，各 workstream 只使用自己相关的字段。

use std::sync::Arc;

use crate::ai_agent::AiService;
use crate::index_engine::IndexStore;
use crate::shell_icon::IconCache;

pub struct AppState {
    /// WS-A 拥有其内部实现
    pub index: Arc<IndexStore>,
    /// WS-B 拥有其内部实现
    pub ai: Arc<AiService>,
    /// Windows 系统图标缓存（按图标种类缓存，见 shell_icon.rs）
    pub icons: Arc<IconCache>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            index: Arc::new(IndexStore::new()),
            ai: Arc::new(AiService::new()),
            icons: Arc::new(IconCache::new()),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

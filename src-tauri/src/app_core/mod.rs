//! 应用核心：事件名常量与跨模块的公共约定。
//!
//! 本模块由 Phase 0 拥有，各 workstream 只读取常量，不修改。

/// 后端 → 前端的事件名
pub mod events {
    /// 索引进度：payload = IndexProgress
    pub const INDEX_PROGRESS: &str = "index:progress";
    /// 索引完成：payload = IndexStatus
    pub const INDEX_DONE: &str = "index:done";
    /// 检索结果分批推送：payload = Vec<SearchHit>
    pub const SEARCH_RESULT: &str = "search:result";
    /// AI 流式 token：payload = String
    pub const AI_TOKEN: &str = "ai:token";
    /// 文件操作进度：payload = String
    pub const FS_PROGRESS: &str = "fs:progress";
}

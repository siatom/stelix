//! 应用核心：事件名常量与跨模块的公共约定。
//!
//! 本模块由 Phase 0 拥有，各 workstream 只读取常量，不修改。

/// 后端 → 前端的事件名
pub mod events {
    /// 索引进度：payload = IndexProgress
    pub const INDEX_PROGRESS: &str = "index:progress";
    /// 索引完成：payload = IndexStatus
    pub const INDEX_DONE: &str = "index:done";

    // 以下三条是 Phase 0 冻结契约里预留的事件，MVP 阶段尚未 emit：
    // 检索结果走 search_files 的返回值一次性返回，AI 回复走 ai_chat 的非流式返回，
    // 文件操作是同步阻塞返回。留作 Phase 2 的流式/进度增强，前端 events 封装已就绪。
    /// 检索结果分批推送（预留）：payload = Vec<SearchHit>
    #[allow(dead_code)]
    pub const SEARCH_RESULT: &str = "search:result";
    /// AI 流式 token（预留）：payload = String
    #[allow(dead_code)]
    pub const AI_TOKEN: &str = "ai:token";
    /// 文件操作进度（预留）：payload = String
    #[allow(dead_code)]
    pub const FS_PROGRESS: &str = "fs:progress";
}

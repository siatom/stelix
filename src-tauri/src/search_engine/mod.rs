//! 查询解析与匹配 —— 由 **WS-A (feat/index-core)** 实现。
//!
//! 契约（Phase 0 冻结）：`parse_query` / `matches` / `score` 三个纯函数。

use crate::types::{FileEntry, SearchQuery};

/// 把用户输入（或 AI 生成的关键词）解析为结构化查询。
/// 支持形如 `ext:pdf size:>100mb modified:today 报告` 的语法。
pub fn parse_query(text: &str) -> SearchQuery {
    SearchQuery {
        text: text.trim().to_string(),
        ..Default::default()
    }
}

/// 条目是否满足查询条件
pub fn matches(entry: &FileEntry, query: &SearchQuery) -> bool {
    if !query.text.is_empty() {
        let needle = query.text.to_lowercase();
        if !entry.name.to_lowercase().contains(&needle) {
            return false;
        }
    }
    if let Some(is_dir) = query.is_dir {
        if entry.is_dir != is_dir {
            return false;
        }
    }
    true
}

/// 相关度打分，越大越靠前
pub fn score(entry: &FileEntry, query: &SearchQuery) -> i32 {
    if query.text.is_empty() {
        return 0;
    }
    let name = entry.name.to_lowercase();
    let needle = query.text.to_lowercase();
    if name == needle {
        100
    } else if name.starts_with(&needle) {
        50
    } else {
        10
    }
}

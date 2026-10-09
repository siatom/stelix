//! 索引引擎 —— 由 **WS-A (feat/index-core)** 实现。
//!
//! 契约（Phase 0 冻结）：
//! - `IndexStore::new()` / `start_scan` / `status` / `search`
//! - 扫描期间通过 `app.emit(events::INDEX_PROGRESS, IndexProgress)` 上报进度，
//!   完成后 emit `events::INDEX_DONE`。
//! - Phase 2 的 MFT 直读通过实现 `IndexSource` trait 接入，不改动本文件对外签名。

use std::sync::Arc;

use dashmap::DashMap;
use parking_lot::RwLock;
use tauri::{AppHandle, Emitter};

use crate::app_core::events;
use crate::types::{FileEntry, IndexProgress, IndexStatus, SearchHit, SearchQuery};

/// 索引数据源抽象：Phase 1 用目录遍历实现，Phase 2 增加 NTFS MFT 实现。
pub trait IndexSource: Send + Sync {
    /// 扫描给定根，产出条目（可分批回调）
    fn scan(&self, roots: &[String], on_batch: &mut dyn FnMut(Vec<FileEntry>));
}

/// 内存索引存储
pub struct IndexStore {
    /// 路径 -> 条目
    entries: DashMap<String, FileEntry>,
    status: RwLock<IndexStatus>,
}

impl IndexStore {
    pub fn new() -> Self {
        Self {
            entries: DashMap::new(),
            status: RwLock::new(IndexStatus::default()),
        }
    }

    /// 启动后台扫描（非阻塞）。TODO(WS-A): 用 rayon + walkdir 并行遍历实现。
    pub fn start_scan(&self, app: AppHandle, roots: Vec<String>) {
        {
            let mut st = self.status.write();
            st.roots = roots.clone();
            st.scanning = true;
            st.scanned = 0;
        }
        let _ = app.emit(
            events::INDEX_PROGRESS,
            IndexProgress {
                scanned: 0,
                total: self.entries.len(),
                current: String::new(),
                done: false,
            },
        );
        // TODO(WS-A): 实际扫描 + 增量更新 + emit(INDEX_DONE)
        let mut st = self.status.write();
        st.scanning = false;
    }

    pub fn status(&self) -> IndexStatus {
        let st = self.status.read();
        IndexStatus {
            total: self.entries.len(),
            ..st.clone()
        }
    }

    /// 检索内存索引。TODO(WS-A): 交给 search_engine 做匹配与排序。
    pub fn search(&self, _query: &SearchQuery) -> Vec<SearchHit> {
        Vec::new()
    }

    /// 供 AI 工具调用复用的内部条目快照
    pub fn snapshot(&self) -> Arc<Vec<FileEntry>> {
        Arc::new(self.entries.iter().map(|e| e.value().clone()).collect())
    }
}

impl Default for IndexStore {
    fn default() -> Self {
        Self::new()
    }
}

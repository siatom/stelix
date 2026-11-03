//! 回收站管理：列出、还原、永久删除与清空。
//!
//! 该功能依赖 Windows Shell 的回收站枚举能力（`trash::os_limited`），
//! 与 `fs_core` 中的删除操作共用同一个 `trash` crate。

use crate::types::{TrashEntry, TrashOpResult};

#[cfg(windows)]
mod imp {
    use std::collections::HashMap;

    use super::{TrashEntry, TrashOpResult};
    use trash::os_limited;

    /// 列出回收站条目，按删除时间从新到旧排序。
    pub fn list() -> Result<Vec<TrashEntry>, String> {
        let items = os_limited::list().map_err(|error| format!("读取回收站失败：{error}"))?;
        let mut entries: Vec<TrashEntry> = items.into_iter().map(to_entry).collect();
        entries.sort_by(|a, b| {
            b.deleted_at
                .cmp(&a.deleted_at)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(entries)
    }

    fn to_entry(item: trash::TrashItem) -> TrashEntry {
        let (is_dir, size, item_count) =
            match os_limited::metadata(&item).ok().map(|meta| meta.size) {
                Some(trash::TrashItemSize::Bytes(bytes)) => (false, bytes, 0),
                Some(trash::TrashItemSize::Entries(count)) => (true, 0, count as u64),
                None => (false, 0, 0),
            };
        let original_parent = item.original_parent.to_string_lossy().into_owned();
        let original_path = item.original_path().to_string_lossy().into_owned();
        TrashEntry {
            id: item.id.to_string_lossy().into_owned(),
            name: item.name.to_string_lossy().into_owned(),
            original_path,
            original_parent,
            deleted_at: item.time_deleted.saturating_mul(1000),
            is_dir,
            size,
            item_count,
        }
    }

    /// 按 id 从当前回收站快照中取条目；不存在的 id 记入 `result.errors`。
    fn select(ids: &[String]) -> Result<(Vec<trash::TrashItem>, TrashOpResult), String> {
        let items = os_limited::list().map_err(|error| format!("读取回收站失败：{error}"))?;
        let mut remaining: HashMap<String, trash::TrashItem> = items
            .into_iter()
            .map(|item| (item.id.to_string_lossy().into_owned(), item))
            .collect();
        let mut selected = Vec::with_capacity(ids.len());
        let mut result = TrashOpResult::default();
        for id in ids {
            match remaining.remove(id) {
                Some(item) => selected.push(item),
                None => result
                    .errors
                    .push("部分条目已不在回收站，请刷新后重试".to_string()),
            }
        }
        result.errors.dedup();
        Ok((selected, result))
    }

    /// 还原指定条目。逐项执行：某一项与原位置同名冲突时不影响其它项。
    pub fn restore(ids: &[String]) -> Result<TrashOpResult, String> {
        let (items, mut result) = select(ids)?;
        for item in items {
            let name = item.name.to_string_lossy().into_owned();
            match os_limited::restore_all([item]) {
                Ok(()) => result.ok += 1,
                Err(error) => result
                    .errors
                    .push(format!("无法还原 {name}：{}", restore_error(&error))),
            }
        }
        Ok(result)
    }

    /// 永久删除指定条目。
    pub fn purge(ids: &[String]) -> Result<TrashOpResult, String> {
        let (items, mut result) = select(ids)?;
        for item in items {
            let name = item.name.to_string_lossy().into_owned();
            match os_limited::purge_all([&item]) {
                Ok(()) => result.ok += 1,
                Err(error) => result.errors.push(format!("无法永久删除 {name}：{error}")),
            }
        }
        Ok(result)
    }

    /// 清空回收站。
    pub fn empty() -> Result<TrashOpResult, String> {
        let items = os_limited::list().map_err(|error| format!("读取回收站失败：{error}"))?;
        if items.is_empty() {
            return Ok(TrashOpResult::default());
        }
        let total = items.len();
        // 先尝试一次批量删除；失败时回退逐项处理，尽量删掉能删的条目。
        if os_limited::purge_all(&items).is_ok() {
            return Ok(TrashOpResult {
                ok: total,
                errors: Vec::new(),
            });
        }
        let mut result = TrashOpResult::default();
        for item in items {
            let name = item.name.to_string_lossy().into_owned();
            match os_limited::purge_all([&item]) {
                Ok(()) => result.ok += 1,
                Err(error) => result.errors.push(format!("无法永久删除 {name}：{error}")),
            }
        }
        if result.ok == 0 && result.errors.is_empty() {
            result.errors.push("清空回收站失败".to_string());
        }
        Ok(result)
    }

    fn restore_error(error: &trash::Error) -> String {
        match error {
            trash::Error::RestoreCollision { path, .. } => {
                format!("原位置已存在同名项（{}）", path.display())
            }
            trash::Error::RestoreTwins { path, .. } => {
                format!("多个条目指向同一原位置（{}）", path.display())
            }
            _ => error.to_string(),
        }
    }
}

#[cfg(windows)]
pub use imp::{empty, list, purge, restore};

#[cfg(not(windows))]
pub fn list() -> Result<Vec<TrashEntry>, String> {
    Ok(Vec::new())
}

#[cfg(not(windows))]
pub fn restore(_ids: &[String]) -> Result<TrashOpResult, String> {
    Err("当前平台暂不支持回收站管理".to_string())
}

#[cfg(not(windows))]
pub fn purge(_ids: &[String]) -> Result<TrashOpResult, String> {
    Err("当前平台暂不支持回收站管理".to_string())
}

#[cfg(not(windows))]
pub fn empty() -> Result<TrashOpResult, String> {
    Err("当前平台暂不支持回收站管理".to_string())
}

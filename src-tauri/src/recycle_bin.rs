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

    /// 回收站是否为空；读取失败时按「非空」处理（用于图标显示）。
    pub fn is_empty() -> bool {
        os_limited::list()
            .map(|items| items.is_empty())
            .unwrap_or(false)
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
pub use imp::{empty, is_empty, list, purge, restore};

/// macOS 废纸篓：列出 `~/.Trash`，支持永久删除 / 清空。
///
/// `trash` crate 在 macOS 只提供「移入废纸篓」，不提供枚举与还原；这里直接读取
/// 废纸篓目录。还原需要 Finder 的「放回原处」（原路径记录在 `.DS_Store` 中），
/// 暂不支持，因此给出明确提示。
#[cfg(target_os = "macos")]
mod macos_imp {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::UNIX_EPOCH;

    use super::{TrashEntry, TrashOpResult};

    fn trash_dir() -> Result<PathBuf, String> {
        dirs::home_dir()
            .map(|home| home.join(".Trash"))
            .ok_or_else(|| "无法定位用户废纸篓".to_string())
    }

    /// 只会操作废纸篓目录内的直接子项，避免前端传入任意路径导致误删。
    fn safe_item(id: &str) -> Option<PathBuf> {
        let dir = trash_dir().ok()?;
        let path = PathBuf::from(id);
        if path.parent() != Some(dir.as_path()) {
            return None;
        }
        let name = path.file_name()?.to_string_lossy();
        if name == ".DS_Store" || name == ".localized" {
            return None;
        }
        Some(path)
    }

    pub fn list() -> Result<Vec<TrashEntry>, String> {
        let dir = trash_dir()?;
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(format!("读取废纸篓失败：{error}")),
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name == ".DS_Store" || name == ".localized" {
                continue;
            }
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            let is_dir = meta.is_dir();
            let deleted_at = meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_millis() as i64)
                .unwrap_or(0);
            let item_count = if is_dir {
                fs::read_dir(&path)
                    .map(|items| items.flatten().count() as u64)
                    .unwrap_or(0)
            } else {
                0
            };
            let path_text = path.to_string_lossy().to_string();
            out.push(TrashEntry {
                id: path_text.clone(),
                name,
                original_path: path_text,
                original_parent: "废纸篓".to_string(),
                deleted_at,
                is_dir,
                size: if is_dir { 0 } else { meta.len() },
                item_count,
            });
        }
        out.sort_by(|a, b| {
            b.deleted_at
                .cmp(&a.deleted_at)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(out)
    }

    pub fn is_empty() -> bool {
        list().map(|items| items.is_empty()).unwrap_or(false)
    }

    pub fn restore(_ids: &[String]) -> Result<TrashOpResult, String> {
        Err("macOS 暂不支持在应用内还原废纸篓项目，请在 Finder 中右键选择「放回原处」".to_string())
    }

    fn remove(path: &Path) -> std::io::Result<()> {
        if path.is_dir() && !path.is_symlink() {
            fs::remove_dir_all(path)
        } else {
            fs::remove_file(path)
        }
    }

    pub fn purge(ids: &[String]) -> Result<TrashOpResult, String> {
        let mut result = TrashOpResult::default();
        for id in ids {
            let Some(path) = safe_item(id) else {
                result.errors.push("无效的废纸篓条目".to_string());
                continue;
            };
            let name = path
                .file_name()
                .map(|value| value.to_string_lossy().to_string())
                .unwrap_or_default();
            match remove(&path) {
                Ok(()) => result.ok += 1,
                Err(error) => result
                    .errors
                    .push(format!("无法永久删除 {name}：{error}")),
            }
        }
        result.errors.dedup();
        Ok(result)
    }

    pub fn empty() -> Result<TrashOpResult, String> {
        let dir = trash_dir()?;
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(TrashOpResult::default())
            }
            Err(error) => return Err(format!("读取废纸篓失败：{error}")),
        };
        let mut result = TrashOpResult::default();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name == ".DS_Store" || name == ".localized" {
                continue;
            }
            match remove(&entry.path()) {
                Ok(()) => result.ok += 1,
                Err(error) => result.errors.push(format!("无法永久删除 {name}：{error}")),
            }
        }
        // 目录为空（或只剩 .DS_Store / .localized）时视为成功，不报错。
        Ok(result)
    }
}

#[cfg(target_os = "macos")]
pub use macos_imp::{empty, is_empty, list, purge, restore};

#[cfg(not(any(windows, target_os = "macos")))]
pub fn list() -> Result<Vec<TrashEntry>, String> {
    Ok(Vec::new())
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn is_empty() -> bool {
    true
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn restore(_ids: &[String]) -> Result<TrashOpResult, String> {
    Err("当前平台暂不支持回收站管理".to_string())
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn purge(_ids: &[String]) -> Result<TrashOpResult, String> {
    Err("当前平台暂不支持回收站管理".to_string())
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn empty() -> Result<TrashOpResult, String> {
    Err("当前平台暂不支持回收站管理".to_string())
}

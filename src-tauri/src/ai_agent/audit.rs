//! AI 写操作审计。
//!
//! 以 JSON Lines 追加到 `{config_dir}/ai-audit.jsonl`，一行一条记录。
//! 审计失败只打印到 stderr，绝不阻断文件操作本身。

use std::io::Write;
use std::path::PathBuf;

use once_cell::sync::Lazy;
use parking_lot::Mutex;

use crate::persistence;
use crate::types::AiAuditEntry;

static AUDIT_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

fn audit_path() -> PathBuf {
    persistence::config_dir().join("ai-audit.jsonl")
}

pub fn append(entry: &AiAuditEntry) {
    let _guard = AUDIT_LOCK.lock();
    let Ok(line) = serde_json::to_string(entry) else {
        eprintln!("序列化 AI 审计记录失败");
        return;
    };
    let path = audit_path();
    if let Some(parent) = path.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            eprintln!("创建 AI 审计目录失败（{}）：{error}", parent.display());
            return;
        }
    }
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        Ok(mut file) => {
            if let Err(error) = writeln!(file, "{line}") {
                eprintln!("写入 AI 审计记录失败（{}）：{error}", path.display());
            }
        }
        Err(error) => eprintln!("打开 AI 审计文件失败（{}）：{error}", path.display()),
    }
}

/// 读取最近 `limit` 条审计记录（按时间正序返回）。
pub fn list(limit: usize) -> Vec<AiAuditEntry> {
    let _guard = AUDIT_LOCK.lock();
    let limit = limit.clamp(1, 500);
    let Ok(text) = std::fs::read_to_string(audit_path()) else {
        return Vec::new();
    };
    let mut entries: Vec<AiAuditEntry> = text
        .lines()
        .rev()
        .take(limit)
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    entries.reverse();
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_roundtrips_as_json_line() {
        let entry = AiAuditEntry {
            timestamp: 1,
            run_id: "run".into(),
            tool: "delete_files".into(),
            description: "移到回收站 1 项".into(),
            decision: "approve".into(),
            ok: true,
            error: String::new(),
            duration_ms: 12,
        };
        let line = serde_json::to_string(&entry).unwrap();
        let restored: AiAuditEntry = serde_json::from_str(&line).unwrap();
        assert_eq!(restored.tool, "delete_files");
        assert!(restored.ok);
    }
}

//! 每条对话独立保存，临时文件替换避免中途写入破坏历史。
use crate::types::AiConversation;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use std::path::{Path, PathBuf};

static HISTORY_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

fn path_for(root: &Path, id: &str) -> Result<PathBuf, String> {
    let uuid = uuid::Uuid::parse_str(id).map_err(|_| "对话标识无效")?;
    if uuid.to_string() != id {
        return Err("对话标识无效".into());
    }
    Ok(root.join(format!("{uuid}.json")))
}

fn validate(conversation: &AiConversation) -> Result<(), String> {
    if conversation.title.trim().is_empty() || conversation.title.chars().count() > 120 {
        return Err("对话标题须为 1–120 个字符".into());
    }
    let mut ids = std::collections::HashSet::new();
    for message in &conversation.messages {
        if message.id.is_empty()
            || !ids.insert(&message.id)
            || !matches!(message.role.as_str(), "user" | "assistant")
            || !matches!(
                message.status.as_str(),
                "pending" | "complete" | "error" | "interrupted"
            )
            || !matches!(message.kind.as_str(), "message" | "operation")
            || message.operation.as_deref().is_some_and(|op| {
                !matches!(
                    op,
                    "pending" | "completed" | "cancelled" | "expired" | "error"
                )
            })
        {
            return Err("对话消息格式无效".into());
        }
    }
    Ok(())
}

fn save_at(root: &Path, conversation: &AiConversation) -> Result<(), String> {
    validate(conversation)?;
    let path = path_for(root, &conversation.id)?;
    std::fs::create_dir_all(root).map_err(|e| format!("创建历史目录失败：{e}"))?;
    let bytes = serde_json::to_vec_pretty(conversation).map_err(|e| e.to_string())?;
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, bytes).map_err(|e| format!("保存历史对话失败：{e}"))?;
    std::fs::rename(&temp, &path).map_err(|e| format!("更新历史对话失败：{e}"))
}

fn list_at(root: &Path) -> Result<Vec<AiConversation>, String> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("读取历史对话失败：{e}")),
    };
    let mut conversations = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.extension().and_then(|v| v.to_str()) != Some("json") {
            continue;
        }
        // 单个文件损坏 / 不可读 / 标识不一致只跳过它，不让整张历史列表失败。
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                eprintln!("跳过无法读取的历史对话（{}）：{error}", path.display());
                continue;
            }
        };
        let mut conversation: AiConversation = match serde_json::from_slice(&bytes) {
            Ok(conversation) => conversation,
            Err(error) => {
                eprintln!("跳过损坏的历史对话文件（{}）：{error}", path.display());
                continue;
            }
        };
        if let Err(error) = validate(&conversation) {
            eprintln!("跳过格式无效的历史对话（{}）：{error}", path.display());
            continue;
        }
        if path_for(root, &conversation.id).map_or(true, |expected| expected != path) {
            eprintln!("跳过历史对话标识与文件名不一致的文件（{}）", path.display());
            continue;
        }
        for message in &mut conversation.messages {
            if message.status == "pending" {
                message.status = "interrupted".into();
                if message.content.is_empty() {
                    message.content = "上次回复未完成，可以重试。".into();
                }
            }
            if message.operation.as_deref() == Some("pending") {
                message.operation = Some("expired".into());
            }
        }
        conversations.push(conversation);
    }
    conversations.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(conversations)
}

fn delete_at(root: &Path, id: &str) -> Result<(), String> {
    match std::fs::remove_file(path_for(root, id)?) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("删除历史对话失败：{e}")),
    }
}

pub fn list() -> Result<Vec<AiConversation>, String> {
    let _guard = HISTORY_LOCK.lock();
    list_at(&super::config_dir().join("ai-conversations"))
}
pub fn save(conversation: &AiConversation) -> Result<(), String> {
    let _guard = HISTORY_LOCK.lock();
    save_at(&super::config_dir().join("ai-conversations"), conversation)
}
pub fn delete(id: &str) -> Result<(), String> {
    let _guard = HISTORY_LOCK.lock();
    delete_at(&super::config_dir().join("ai-conversations"), id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::AiChatMessage;

    #[test]
    fn history_roundtrip_update_and_delete_with_expired_actions() {
        let root = std::env::temp_dir().join(format!("stelix-history-{}", uuid::Uuid::new_v4()));
        let id = uuid::Uuid::new_v4().to_string();
        let mut conversation = AiConversation {
            id: id.clone(),
            title: "文件整理".into(),
            created_at: 1,
            updated_at: 2,
            messages: vec![AiChatMessage {
                id: "message-1".into(),
                role: "assistant".into(),
                content: String::new(),
                created_at: 1,
                status: "pending".into(),
                model: "Test".into(),
                kind: "message".into(),
                operation: Some("pending".into()),
                usage: None,
                duration_ms: None,
            }],
        };
        save_at(&root, &conversation).unwrap();
        let restored = list_at(&root).unwrap();
        assert_eq!(restored[0].messages[0].status, "interrupted");
        assert_eq!(
            restored[0].messages[0].operation.as_deref(),
            Some("expired")
        );
        conversation.title = "重命名后的对话".into();
        save_at(&root, &conversation).unwrap();
        assert_eq!(list_at(&root).unwrap()[0].title, "重命名后的对话");
        assert!(path_for(&root, "../ai").is_err());
        delete_at(&root, &id).unwrap();
        assert!(list_at(&root).unwrap().is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn list_skips_corrupt_and_mismatched_files() {
        let root =
            std::env::temp_dir().join(format!("stelix-history-skip-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();

        let conversation = AiConversation {
            id: uuid::Uuid::new_v4().to_string(),
            title: "正常对话".into(),
            created_at: 1,
            updated_at: 4,
            messages: Vec::new(),
        };
        save_at(&root, &conversation).unwrap();

        // 损坏 JSON
        std::fs::write(
            root.join(format!("{}.json", uuid::Uuid::new_v4())),
            b"{ broken",
        )
        .unwrap();
        // 内容合法但 id 与文件名不一致
        let mismatched = AiConversation {
            id: uuid::Uuid::new_v4().to_string(),
            title: "错位对话".into(),
            created_at: 2,
            updated_at: 3,
            messages: Vec::new(),
        };
        std::fs::write(
            root.join(format!("{}.json", uuid::Uuid::new_v4())),
            serde_json::to_vec(&mismatched).unwrap(),
        )
        .unwrap();
        // 文件名不是合法标识
        std::fs::write(root.join("not-a-uuid.json"), b"{}").unwrap();
        // 非 JSON 文件应被忽略
        std::fs::write(root.join("notes.txt"), b"ignore me").unwrap();

        let restored = list_at(&root).unwrap();
        assert_eq!(restored.len(), 1, "损坏文件不应拖垮整张列表");
        assert_eq!(restored[0].id, conversation.id);
        std::fs::remove_dir_all(root).unwrap();
    }
}

//! 每条对话独立保存，临时文件替换避免中途写入破坏历史。
//!
//! 列表只读取轻量索引（标题、更新时间、消息数、预览），正文在打开对话时按 ID 读取。
//! 索引是可重建的缓存：丢失或与文件不一致时，按文件签名（大小 + 修改时间）重新生成。
use crate::types::{AiConversation, AiConversationSummary};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

static HISTORY_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

const INDEX_FILE: &str = "index.json";
const INDEX_VERSION: u32 = 1;
const PREVIEW_LIMIT: usize = 100;

fn history_root() -> PathBuf {
    super::config_dir().join("ai-conversations")
}

fn path_for(root: &Path, id: &str) -> Result<PathBuf, String> {
    let uuid = uuid::Uuid::parse_str(id).map_err(|_| "对话标识无效")?;
    if uuid.to_string() != id {
        return Err("对话标识无效".into());
    }
    Ok(root.join(format!("{uuid}.json")))
}

fn validate_title(title: &str) -> Result<(), String> {
    if title.trim().is_empty() || title.chars().count() > 120 {
        return Err("对话标题须为 1–120 个字符".into());
    }
    Ok(())
}

fn validate(conversation: &AiConversation) -> Result<(), String> {
    validate_title(&conversation.title)?;
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

/// 重新打开的历史只能继续对话，不能恢复待执行的操作。
fn normalize(conversation: &mut AiConversation) {
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
}

fn preview_of(conversation: &AiConversation) -> String {
    conversation
        .messages
        .last()
        .map(|message| {
            message
                .content
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default()
        .chars()
        .take(PREVIEW_LIMIT)
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IndexEntry {
    id: String,
    title: String,
    created_at: i64,
    updated_at: i64,
    message_count: usize,
    preview: String,
    /// 生成摘要时的文件大小与修改时间（Unix 毫秒），用于发现绕过索引的改动。
    size: u64,
    modified: u64,
}

impl IndexEntry {
    fn from_conversation(conversation: &AiConversation, size: u64, modified: u64) -> Self {
        Self {
            id: conversation.id.clone(),
            title: conversation.title.clone(),
            created_at: conversation.created_at,
            updated_at: conversation.updated_at,
            message_count: conversation.messages.len(),
            preview: preview_of(conversation),
            size,
            modified,
        }
    }

    fn summary(&self) -> AiConversationSummary {
        AiConversationSummary {
            id: self.id.clone(),
            title: self.title.clone(),
            created_at: self.created_at,
            updated_at: self.updated_at,
            message_count: self.message_count,
            preview: self.preview.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HistoryIndex {
    version: u32,
    entries: Vec<IndexEntry>,
}

impl Default for HistoryIndex {
    fn default() -> Self {
        Self {
            version: INDEX_VERSION,
            entries: Vec::new(),
        }
    }
}

fn read_index(root: &Path) -> Option<HistoryIndex> {
    let bytes = std::fs::read(root.join(INDEX_FILE)).ok()?;
    let index: HistoryIndex = serde_json::from_slice(&bytes).ok()?;
    (index.version == INDEX_VERSION).then_some(index)
}

fn write_index(root: &Path, index: &HistoryIndex) -> Result<(), String> {
    std::fs::create_dir_all(root).map_err(|e| format!("创建历史目录失败：{e}"))?;
    let path = root.join(INDEX_FILE);
    let temp = root.join("index.json.tmp");
    let bytes = serde_json::to_vec(index).map_err(|e| e.to_string())?;
    std::fs::write(&temp, bytes).map_err(|e| format!("保存历史索引失败：{e}"))?;
    std::fs::rename(&temp, &path).map_err(|e| format!("更新历史索引失败：{e}"))
}

fn file_signature(path: &Path) -> Option<(u64, u64)> {
    let metadata = std::fs::metadata(path).ok()?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0);
    Some((metadata.len(), modified))
}

fn write_conversation(root: &Path, conversation: &AiConversation) -> Result<PathBuf, String> {
    validate(conversation)?;
    let path = path_for(root, &conversation.id)?;
    std::fs::create_dir_all(root).map_err(|e| format!("创建历史目录失败：{e}"))?;
    let bytes = serde_json::to_vec_pretty(conversation).map_err(|e| e.to_string())?;
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, bytes).map_err(|e| format!("保存历史对话失败：{e}"))?;
    std::fs::rename(&temp, &path).map_err(|e| format!("更新历史对话失败：{e}"))?;
    Ok(path)
}

fn read_conversation(root: &Path, path: &Path) -> Result<AiConversation, String> {
    let bytes = std::fs::read(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            "历史对话不存在".to_string()
        } else {
            format!("读取历史对话失败：{e}")
        }
    })?;
    let conversation: AiConversation =
        serde_json::from_slice(&bytes).map_err(|e| format!("历史对话格式无效：{e}"))?;
    validate(&conversation)?;
    if path_for(root, &conversation.id).map_or(true, |expected| expected != path) {
        return Err("历史对话标识与文件名不一致".into());
    }
    Ok(conversation)
}

fn update_index(root: &Path, conversation: &AiConversation) {
    let path = match path_for(root, &conversation.id) {
        Ok(path) => path,
        Err(_) => return,
    };
    let (size, modified) = file_signature(&path).unwrap_or((0, 0));
    let mut index = read_index(root).unwrap_or_default();
    let entry = IndexEntry::from_conversation(conversation, size, modified);
    match index
        .entries
        .iter_mut()
        .find(|item| item.id == conversation.id)
    {
        Some(existing) => *existing = entry,
        None => index.entries.push(entry),
    }
    index
        .entries
        .sort_by_key(|entry| std::cmp::Reverse(entry.updated_at));
    // 索引只是缓存：写入失败时下次列表会按文件签名重建，不影响对话本身。
    if let Err(error) = write_index(root, &index) {
        eprintln!("更新历史对话索引失败：{error}");
    }
}

fn list_at(root: &Path) -> Result<Vec<AiConversationSummary>, String> {
    let mut index = read_index(root).unwrap_or_default();
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("读取历史对话失败：{e}")),
    };
    let mut seen = std::collections::HashSet::new();
    let mut changed = false;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.file_name().and_then(|value| value.to_str()) == Some(INDEX_FILE)
            || path.extension().and_then(|value| value.to_str()) != Some("json")
        {
            continue;
        }
        // 只接受规范 UUID 文件名；非对话 JSON 直接忽略。
        let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        let Ok(uuid) = uuid::Uuid::parse_str(stem) else {
            continue;
        };
        if uuid.to_string() != stem {
            continue;
        }
        let id = stem.to_string();
        seen.insert(id.clone());
        let Some((size, modified)) = file_signature(&path) else {
            continue;
        };
        let up_to_date = index
            .entries
            .iter()
            .any(|item| item.id == id && item.size == size && item.modified == modified);
        if up_to_date {
            continue;
        }
        // 单个文件损坏 / 不可读 / 标识不一致只跳过它，不让整张历史列表失败。
        let mut conversation = match read_conversation(root, &path) {
            Ok(conversation) => conversation,
            Err(error) => {
                eprintln!("跳过无法读取的历史对话（{}）：{error}", path.display());
                index.entries.retain(|item| item.id != id);
                seen.remove(&id);
                changed = true;
                continue;
            }
        };
        normalize(&mut conversation);
        let item = IndexEntry::from_conversation(&conversation, size, modified);
        match index.entries.iter_mut().find(|existing| existing.id == id) {
            Some(existing) => *existing = item,
            None => index.entries.push(item),
        }
        changed = true;
    }
    let before = index.entries.len();
    index.entries.retain(|entry| seen.contains(&entry.id));
    if before != index.entries.len() {
        changed = true;
    }
    index
        .entries
        .sort_by_key(|entry| std::cmp::Reverse(entry.updated_at));
    if changed {
        if let Err(error) = write_index(root, &index) {
            eprintln!("更新历史对话索引失败：{error}");
        }
    }
    Ok(index.entries.iter().map(IndexEntry::summary).collect())
}

fn get_at(root: &Path, id: &str) -> Result<AiConversation, String> {
    let path = path_for(root, id)?;
    let mut conversation = read_conversation(root, &path)?;
    normalize(&mut conversation);
    Ok(conversation)
}

fn save_at(root: &Path, conversation: &AiConversation) -> Result<(), String> {
    write_conversation(root, conversation)?;
    update_index(root, conversation);
    Ok(())
}

fn rename_at(root: &Path, id: &str, title: &str) -> Result<(), String> {
    let name = title.trim();
    validate_title(name)?;
    let path = path_for(root, id)?;
    let mut conversation = read_conversation(root, &path)?;
    if conversation.title == name {
        return Ok(());
    }
    conversation.title = name.to_string();
    write_conversation(root, &conversation)?;
    update_index(root, &conversation);
    Ok(())
}

fn delete_at(root: &Path, id: &str) -> Result<(), String> {
    match std::fs::remove_file(path_for(root, id)?) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("删除历史对话失败：{e}")),
    }
    if let Some(mut index) = read_index(root) {
        let before = index.entries.len();
        index.entries.retain(|entry| entry.id != id);
        if before != index.entries.len() {
            if let Err(error) = write_index(root, &index) {
                eprintln!("更新历史对话索引失败：{error}");
            }
        }
    }
    Ok(())
}

pub fn list() -> Result<Vec<AiConversationSummary>, String> {
    let _guard = HISTORY_LOCK.lock();
    list_at(&history_root())
}

pub fn get(id: &str) -> Result<AiConversation, String> {
    let _guard = HISTORY_LOCK.lock();
    get_at(&history_root(), id)
}

pub fn save(conversation: &AiConversation) -> Result<(), String> {
    let _guard = HISTORY_LOCK.lock();
    save_at(&history_root(), conversation)
}

pub fn rename(id: &str, title: &str) -> Result<(), String> {
    let _guard = HISTORY_LOCK.lock();
    rename_at(&history_root(), id, title)
}

pub fn delete(id: &str) -> Result<(), String> {
    let _guard = HISTORY_LOCK.lock();
    delete_at(&history_root(), id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::AiChatMessage;

    fn temp_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("stelix-{name}-{}", uuid::Uuid::new_v4()))
    }

    fn conversation(title: &str, messages: Vec<AiChatMessage>) -> AiConversation {
        AiConversation {
            id: uuid::Uuid::new_v4().to_string(),
            title: title.into(),
            created_at: 1,
            updated_at: 2,
            messages,
        }
    }

    fn message(id: &str, content: &str) -> AiChatMessage {
        AiChatMessage {
            id: id.into(),
            role: "assistant".into(),
            content: content.into(),
            created_at: 1,
            status: "complete".into(),
            model: "Test".into(),
            kind: "message".into(),
            operation: None,
            usage: None,
            duration_ms: None,
            steps: Vec::new(),
        }
    }

    #[test]
    fn history_roundtrip_update_and_delete_with_expired_actions() {
        let root = temp_root("history");
        let conversation = conversation(
            "文件整理",
            vec![AiChatMessage {
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
                steps: Vec::new(),
            }],
        );
        save_at(&root, &conversation).unwrap();

        let summaries = list_at(&root).unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].message_count, 1);
        // 保存时回复仍在进行，摘要不提前写入过期文案。
        assert_eq!(summaries[0].preview, "");

        let restored = get_at(&root, &conversation.id).unwrap();
        assert_eq!(restored.messages[0].status, "interrupted");
        assert_eq!(restored.messages[0].operation.as_deref(), Some("expired"));

        // 索引重建（例如升级后首次打开）时同样应用过期语义。
        std::fs::remove_file(root.join(INDEX_FILE)).unwrap();
        let rebuilt = list_at(&root).unwrap();
        assert_eq!(rebuilt[0].preview, "上次回复未完成，可以重试。");

        rename_at(&root, &conversation.id, " 重命名后的对话 ").unwrap();
        assert_eq!(list_at(&root).unwrap()[0].title, "重命名后的对话");
        assert_eq!(
            get_at(&root, &conversation.id).unwrap().title,
            "重命名后的对话"
        );
        assert!(path_for(&root, "../ai").is_err());
        assert!(rename_at(&root, &conversation.id, "  ").is_err());

        delete_at(&root, &conversation.id).unwrap();
        assert!(list_at(&root).unwrap().is_empty());
        assert!(get_at(&root, &conversation.id).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn list_returns_summaries_without_message_bodies() {
        let root = temp_root("history-summary");
        let body = "长正文".repeat(2_000);
        let conversation = conversation("摘要测试", vec![message("message-1", &body)]);
        save_at(&root, &conversation).unwrap();

        let summaries = list_at(&root).unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].message_count, 1);
        assert_eq!(summaries[0].preview.chars().count(), PREVIEW_LIMIT);

        // 索引只保存摘要，不保存消息正文；正文仍需按 ID 读取。
        let index = std::fs::read_to_string(root.join(INDEX_FILE)).unwrap();
        assert!(!index.contains(&body));
        assert_eq!(
            get_at(&root, &conversation.id).unwrap().messages[0].content,
            body
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn list_migrates_legacy_files_and_rebuilds_missing_index() {
        let root = temp_root("history-migrate");
        std::fs::create_dir_all(&root).unwrap();
        let conversation = conversation("旧历史", vec![message("message-1", "旧正文")]);
        std::fs::write(
            root.join(format!("{}.json", conversation.id)),
            serde_json::to_vec(&conversation).unwrap(),
        )
        .unwrap();
        assert!(!root.join(INDEX_FILE).exists());

        let summaries = list_at(&root).unwrap();
        assert_eq!(summaries.len(), 1);
        assert!(root.join(INDEX_FILE).exists());
        assert_eq!(
            get_at(&root, &conversation.id).unwrap().messages[0].content,
            "旧正文"
        );

        // 索引损坏时应重建而不是让整张列表失败。
        std::fs::write(root.join(INDEX_FILE), b"{ broken").unwrap();
        assert_eq!(list_at(&root).unwrap().len(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn list_skips_corrupt_and_mismatched_files() {
        let root = temp_root("history-skip");
        std::fs::create_dir_all(&root).unwrap();

        let healthy = conversation("正常对话", Vec::new());
        save_at(&root, &healthy).unwrap();

        // 损坏 JSON
        std::fs::write(
            root.join(format!("{}.json", uuid::Uuid::new_v4())),
            b"{ broken",
        )
        .unwrap();
        // 内容合法但 id 与文件名不一致
        let mismatched = conversation("错位对话", Vec::new());
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
        assert_eq!(restored[0].id, healthy.id);
        assert!(get_at(&root, &mismatched.id).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}

//! 配置持久化 —— 由 Phase 0 拥有，WS-B 直接复用。

use std::path::PathBuf;

use crate::types::{AiConfig, FileEntry};
use serde::{Deserialize, Serialize};

fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("stelix")
}

fn ai_config_path() -> PathBuf {
    config_dir().join("ai.json")
}

fn index_cache_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| config_dir())
        .join("stelix")
        .join("index-cache.json")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexJournal {
    pub volume: String,
    pub journal_id: u64,
    pub next_usn: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct IndexCache {
    pub roots: Vec<String>,
    pub entries: Vec<FileEntry>,
    pub journals: Vec<IndexJournal>,
}

pub fn load_index_cache() -> Option<IndexCache> {
    let text = std::fs::read_to_string(index_cache_path()).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save_index_cache(cache: &IndexCache) -> Result<(), String> {
    let dir = config_dir();
    let path = index_cache_path();
    let dir = path.parent().unwrap_or(&dir);
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let temp = path.with_extension("json.tmp");
    let text = serde_json::to_vec(cache).map_err(|e| e.to_string())?;
    std::fs::write(&temp, text).map_err(|e| e.to_string())?;
    // Windows does not replace an existing destination with `rename`.
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| e.to_string())?;
    }
    std::fs::rename(&temp, &path).map_err(|e| e.to_string())
}

pub fn load_ai_config() -> AiConfig {
    let path = ai_config_path();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return AiConfig::default();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

pub fn save_ai_config(cfg: &AiConfig) -> Result<(), String> {
    let dir = config_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let text = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    std::fs::write(ai_config_path(), text).map_err(|e| e.to_string())
}

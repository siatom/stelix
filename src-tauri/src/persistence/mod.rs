//! 配置持久化 —— 由 Phase 0 拥有，WS-B 直接复用。

use std::path::PathBuf;

use crate::types::AiConfig;

fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("file-povit")
}

fn ai_config_path() -> PathBuf {
    config_dir().join("ai.json")
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

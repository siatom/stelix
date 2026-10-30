//! Configuration and index persistence.

pub mod ai_history;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::types::{AiConfig, ScheduledTask};
use once_cell::sync::Lazy;
use parking_lot::Mutex;

/// 应用数据 / 配置根目录，由 [`init_dirs`] 在启动时按 bundle identifier 固定。
///
/// 取值遵循 Tauri 约定：`%LOCALAPPDATA%\{identifier}`（本地数据，与 WebView2
/// 用户目录同处）与 `%APPDATA%\{identifier}`（漫游配置）。
///
/// ⚠️ 刻意**不**使用 `%LOCALAPPDATA%\{productName}`：那是 NSIS 的安装目录
/// （`installer.nsi` 里 `$INSTDIR = $LOCALAPPDATA\${PRODUCTNAME}`）。两者同名会让
/// 用户数据落进安装目录，造成「卸载残留、清装/升级丢数据、改名后数据分叉」。
static LOCAL_DATA_DIR: OnceLock<PathBuf> = OnceLock::new();
static CONFIG_DIR: OnceLock<PathBuf> = OnceLock::new();

/// 用 bundle identifier 初始化数据 / 配置目录。
///
/// 必须在 `AppState::new()`（会读取 AI 配置）之前调用一次；幂等，重复调用只保留首次结果。
/// identifier 不同（如 dev 版 `com.stelix.desktop.dev`）→ 数据目录天然隔离。
pub fn init_dirs(identifier: &str) {
    let _ = LOCAL_DATA_DIR.set(
        dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(identifier),
    );
    let _ = CONFIG_DIR.set(
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(identifier),
    );
}

/// 兜底数据目录：仅在 [`init_dirs`] 未被调用时使用（如单元测试）。
fn fallback_local_data_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("stelix")
}

/// 兜底配置目录：仅在 [`init_dirs`] 未被调用时使用（如单元测试）。
fn fallback_config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("stelix")
}

fn config_dir() -> PathBuf {
    CONFIG_DIR
        .get()
        .cloned()
        .unwrap_or_else(fallback_config_dir)
}

fn ai_config_path() -> PathBuf {
    config_dir().join("ai.json")
}

/// 写临时文件后 rename 原子替换，避免进程中断留下截断的 JSON。
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or("配置路径无效")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let name = path
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or("配置路径无效")?;
    let temp = parent.join(format!("{name}.tmp"));
    {
        let mut file = std::fs::File::create(&temp).map_err(|e| e.to_string())?;
        file.write_all(bytes).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
    }
    std::fs::rename(&temp, path).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        e.to_string()
    })
}

/// 解析失败时把损坏文件改名备份（`<name>.corrupt-<时间戳>`），避免后续覆盖造成不可逆丢失。
fn backup_corrupt(path: &Path) {
    if !path.exists() {
        return;
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or_default();
    let name = path
        .file_name()
        .and_then(|v| v.to_str())
        .unwrap_or("config");
    let backup = path.with_file_name(format!("{name}.corrupt-{stamp}"));
    if let Err(error) = std::fs::rename(path, &backup) {
        eprintln!("备份损坏的配置文件失败（{}）：{error}", path.display());
    }
}

/// 索引根目录配置（用户显式选择的目录）。索引不再默认覆盖整盘。
fn index_roots_path() -> PathBuf {
    config_dir().join("index-roots.json")
}

/// 读取索引根目录配置；文件缺失或损坏时返回 None，由调用方回退到默认目录。
pub fn load_index_roots() -> Option<Vec<String>> {
    let text = std::fs::read_to_string(index_roots_path()).ok()?;
    serde_json::from_str::<Vec<String>>(&text).ok()
}

pub fn save_index_roots(roots: &[String]) -> Result<(), String> {
    let text = serde_json::to_string_pretty(roots).map_err(|e| e.to_string())?;
    write_atomic(&index_roots_path(), text.as_bytes())
}

/// 删除旧版索引缓存（JSON 与 SQLite 两代）。索引已改为纯内存、不再落盘，
/// 留着这些文件只会白占磁盘（实测 SQLite 缓存达 1.6 GB）。
pub fn remove_index_cache_files() {
    let dir = LOCAL_DATA_DIR
        .get()
        .cloned()
        .unwrap_or_else(fallback_local_data_dir);
    for name in [
        "index-cache.sqlite3",
        "index-cache.sqlite3-wal",
        "index-cache.sqlite3-shm",
        "index-cache.json",
    ] {
        let path = dir.join(name);
        if path.exists() {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// AI 密钥存取；抽成 trait 便于在无密钥环环境下测试降级路径。
trait SecretStore {
    fn set(&self, provider_id: &str, secret: &str) -> Result<(), String>;
    fn get(&self, provider_id: &str) -> Result<Option<String>, String>;
    fn delete(&self, provider_id: &str) -> Result<(), String>;
}

const AI_SECRET_SERVICE: &str = "stelix.ai-provider";
static AI_SECRET_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

/// 由 provider id 派生密钥环条目名：FNV-1a 稳定哈希，避免 id 含非法字符或超长。
fn provider_secret_key(provider_id: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in provider_id.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("stelix-ai-{hash:016x}")
}

fn ai_secret_entry(provider_id: &str) -> Result<keyring::Entry, String> {
    if provider_id.is_empty() || provider_id.len() > 256 {
        return Err("AI Provider 标识无效".into());
    }
    keyring::Entry::new(AI_SECRET_SERVICE, &provider_secret_key(provider_id))
        .map_err(|error| format!("无法访问系统凭据库：{error}"))
}

struct KeyringStore;

impl SecretStore for KeyringStore {
    fn set(&self, provider_id: &str, secret: &str) -> Result<(), String> {
        let _guard = AI_SECRET_LOCK.lock();
        ai_secret_entry(provider_id)?
            .set_password(secret)
            .map_err(|error| format!("无法保存 AI 密钥：{error}"))
    }

    fn get(&self, provider_id: &str) -> Result<Option<String>, String> {
        let _guard = AI_SECRET_LOCK.lock();
        match ai_secret_entry(provider_id)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(format!("无法读取 AI 密钥：{error}")),
        }
    }

    fn delete(&self, provider_id: &str) -> Result<(), String> {
        let _guard = AI_SECRET_LOCK.lock();
        match ai_secret_entry(provider_id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(format!("无法删除 AI 密钥：{error}")),
        }
    }
}

/// 读取 AI 配置：文件里为空 / 旧版明文的 `apiKey` 仍按原语义返回可用密钥。
pub fn load_ai_config() -> AiConfig {
    load_ai_config_at(&KeyringStore, &ai_config_path())
}

fn load_ai_config_at(store: &dyn SecretStore, path: &Path) -> AiConfig {
    let Ok(text) = std::fs::read_to_string(path) else {
        return AiConfig::default();
    };
    let mut config = match parse_ai_config(&text) {
        Ok(config) => config,
        Err(error) => {
            eprintln!(
                "AI 配置无法解析，已备份原文件（{}）：{error}",
                path.display()
            );
            backup_corrupt(path);
            return AiConfig::default();
        }
    };
    for provider in &mut config.providers {
        // 旧版明文密钥直接沿用，下次保存时自然迁移进密钥环。
        if !provider.api_key.is_empty() {
            continue;
        }
        match store.get(&provider.id) {
            Ok(Some(secret)) => provider.api_key = secret,
            Ok(None) => {}
            Err(error) => eprintln!(
                "读取 AI 密钥失败，改用配置文件中的值（{}）：{error}",
                provider.id
            ),
        }
    }
    config
}

/// 仅在读取旧配置时迁移单一接入；保存后只保留新的配置结构。
fn parse_ai_config(text: &str) -> Result<AiConfig, serde_json::Error> {
    let value: serde_json::Value = serde_json::from_str(text)?;
    if value.get("providers").is_some() {
        return serde_json::from_value(value);
    }
    let model = value["model"].as_str().unwrap_or("gpt-4o-mini");
    serde_json::from_value(serde_json::json!({
        "enabled": value["enabled"].as_bool().unwrap_or(false),
        "activeProviderId": "legacy", "activeModelId": model,
        "providers": [{"id":"legacy", "name":"原有 AI 接入", "vendor":"custom",
            "protocol":"openai-chat",
            "baseUrl":value["baseUrl"].as_str().unwrap_or("https://api.openai.com/v1"),
            "apiKey":value["apiKey"].as_str().unwrap_or(""),
            "models":[{"id":model,"name":model,"vendor":""}]}]
    }))
}

#[cfg(test)]
mod ai_config_tests {
    use super::*;

    #[test]
    fn old_config_migrates_and_new_config_roundtrips() {
        let config = parse_ai_config(r#"{"enabled":true,"baseUrl":"http://localhost:1234/v1","apiKey":"test-key","model":"local-model"}"#).unwrap();
        let (provider, model) = config.active().unwrap();
        assert_eq!(provider.api_key, "test-key");
        assert_eq!(model.id, "local-model");
        config.validate().unwrap();
        let saved = serde_json::to_string(&config).unwrap();
        assert!(serde_json::from_str::<serde_json::Value>(&saved)
            .unwrap()
            .get("baseUrl")
            .is_none());
        let restored = parse_ai_config(&saved).unwrap();
        assert_eq!(
            restored.active().unwrap().0.base_url,
            "http://localhost:1234/v1"
        );
    }

    #[test]
    fn invalid_selection_and_duplicate_models_are_rejected() {
        let mut config = parse_ai_config(r#"{"enabled":true,"model":"local-model"}"#).unwrap();
        config.active_model_id = "missing".into();
        assert!(config.validate().is_err());
        config.active_model_id = "local-model".into();
        let model = config.providers[0].models[0].clone();
        config.providers[0].models.push(model);
        assert!(config.validate().is_err());
    }
}

#[cfg(test)]
mod config_persistence_tests {
    use super::*;
    use crate::types::{AiModel, AiProtocol, AiProvider, TaskAction, TaskSchedule};
    use std::collections::HashMap;
    use std::sync::Mutex;

    struct MemoryStore {
        secrets: Mutex<HashMap<String, String>>,
    }

    impl MemoryStore {
        fn new() -> Self {
            Self {
                secrets: Mutex::new(HashMap::new()),
            }
        }

        fn peek(&self, provider_id: &str) -> Option<String> {
            self.secrets.lock().unwrap().get(provider_id).cloned()
        }
    }

    impl SecretStore for MemoryStore {
        fn set(&self, provider_id: &str, secret: &str) -> Result<(), String> {
            self.secrets
                .lock()
                .unwrap()
                .insert(provider_id.to_string(), secret.to_string());
            Ok(())
        }

        fn get(&self, provider_id: &str) -> Result<Option<String>, String> {
            Ok(self.secrets.lock().unwrap().get(provider_id).cloned())
        }

        fn delete(&self, provider_id: &str) -> Result<(), String> {
            self.secrets.lock().unwrap().remove(provider_id);
            Ok(())
        }
    }

    /// 模拟系统凭据库不可用（Linux 无 secret service、服务被锁定等）。
    struct UnavailableStore;

    impl SecretStore for UnavailableStore {
        fn set(&self, _: &str, _: &str) -> Result<(), String> {
            Err("系统凭据库不可用".into())
        }

        fn get(&self, _: &str) -> Result<Option<String>, String> {
            Err("系统凭据库不可用".into())
        }

        fn delete(&self, _: &str) -> Result<(), String> {
            Err("系统凭据库不可用".into())
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("stelix-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample_config(api_key: &str) -> AiConfig {
        AiConfig {
            providers: vec![AiProvider {
                id: "provider-1".into(),
                name: "测试接入".into(),
                vendor: "custom".into(),
                protocol: AiProtocol::OpenaiChat,
                base_url: "https://example.com/v1".into(),
                api_key: api_key.into(),
                models: vec![AiModel {
                    id: "model-1".into(),
                    name: "model-1".into(),
                    vendor: String::new(),
                }],
            }],
            active_provider_id: "provider-1".into(),
            active_model_id: "model-1".into(),
            enabled: true,
        }
    }

    fn backup_files(dir: &Path, name: &str) -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|value| value.to_str())
                    .is_some_and(|file| file.starts_with(&format!("{name}.corrupt-")))
            })
            .collect()
    }

    #[test]
    fn write_atomic_replaces_whole_file_and_cleans_temp() {
        let dir = temp_dir("atomic");
        let path = dir.join("nested").join("config.json");
        write_atomic(&path, b"first").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "first");
        write_atomic(&path, b"second-version").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second-version");
        assert!(!path.with_file_name("config.json.tmp").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn api_key_moves_to_secret_store_and_is_loaded_back() {
        let dir = temp_dir("ai-secret");
        let path = dir.join("ai.json");
        let store = MemoryStore::new();
        save_ai_config_at(&store, &path, &sample_config("sk-secret-123")).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            !text.contains("sk-secret-123"),
            "配置文件不应包含明文密钥：{text}"
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&text).unwrap()["providers"][0]["apiKey"],
            ""
        );
        assert_eq!(store.peek("provider-1").as_deref(), Some("sk-secret-123"));
        assert!(!path.with_file_name("ai.json.tmp").exists());

        let loaded = load_ai_config_at(&store, &path);
        assert_eq!(loaded.providers[0].api_key, "sk-secret-123");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn legacy_plaintext_key_is_used_then_migrated() {
        let dir = temp_dir("ai-legacy");
        let path = dir.join("ai.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&sample_config("legacy-key")).unwrap(),
        )
        .unwrap();

        let store = MemoryStore::new();
        let loaded = load_ai_config_at(&store, &path);
        assert_eq!(loaded.providers[0].api_key, "legacy-key");
        assert_eq!(store.peek("provider-1"), None);

        save_ai_config_at(&store, &path, &loaded).unwrap();
        assert_eq!(store.peek("provider-1").as_deref(), Some("legacy-key"));
        assert!(!std::fs::read_to_string(&path)
            .unwrap()
            .contains("legacy-key"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cleared_key_is_removed_from_secret_store() {
        let dir = temp_dir("ai-clear");
        let path = dir.join("ai.json");
        let store = MemoryStore::new();
        save_ai_config_at(&store, &path, &sample_config("sk-secret-123")).unwrap();
        save_ai_config_at(&store, &path, &sample_config("")).unwrap();
        assert_eq!(store.peek("provider-1"), None);
        assert_eq!(load_ai_config_at(&store, &path).providers[0].api_key, "");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unavailable_secret_store_falls_back_to_plaintext_without_panicking() {
        let dir = temp_dir("ai-unavailable");
        let path = dir.join("ai.json");
        save_ai_config_at(&UnavailableStore, &path, &sample_config("sk-fallback")).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("sk-fallback"), "退化为明文保存：{text}");
        assert_eq!(
            load_ai_config_at(&UnavailableStore, &path).providers[0].api_key,
            "sk-fallback"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn corrupt_ai_config_is_backed_up_and_leaves_siblings_intact() {
        let dir = temp_dir("ai-corrupt");
        let path = dir.join("ai.json");
        let sibling = dir.join("tasks.json");
        std::fs::write(&path, b"{ broken json").unwrap();
        std::fs::write(&sibling, b"[]").unwrap();

        let loaded = load_ai_config_at(&MemoryStore::new(), &path);
        assert!(loaded.providers.is_empty(), "损坏配置应返回默认值");
        assert!(!path.exists(), "损坏文件应被移走而不是被覆盖");

        let backups = backup_files(&dir, "ai.json");
        assert_eq!(backups.len(), 1);
        assert_eq!(std::fs::read(&backups[0]).unwrap(), b"{ broken json");
        assert_eq!(std::fs::read(&sibling).unwrap(), b"[]");
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn sample_task() -> ScheduledTask {
        ScheduledTask {
            id: "task-1".into(),
            name: "清理下载目录".into(),
            enabled: true,
            schedule: TaskSchedule {
                kind: "daily".into(),
                hour: Some(3),
                minute: Some(0),
                ..Default::default()
            },
            action: TaskAction {
                kind: "delete".into(),
                root: "D:\\Downloads".into(),
                ..Default::default()
            },
            created_at: 1,
            last_run: None,
            last_result: None,
            next_run: Some(2),
            run_count: 0,
        }
    }

    #[test]
    fn tasks_roundtrip_atomically_and_corrupt_file_is_backed_up() {
        let dir = temp_dir("tasks");
        let path = dir.join("tasks.json");
        let sibling = dir.join("ai.json");
        save_tasks_at(&path, &[sample_task()]).unwrap();
        assert!(!path.with_file_name("tasks.json.tmp").exists());
        let restored = load_tasks_at(&path);
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].name, "清理下载目录");

        std::fs::write(&sibling, b"{\"enabled\":false}").unwrap();
        std::fs::write(&path, b"[\"truncated\"").unwrap();
        assert!(load_tasks_at(&path).is_empty());
        assert!(!path.exists());

        let backups = backup_files(&dir, "tasks.json");
        assert_eq!(backups.len(), 1);
        assert_eq!(std::fs::read(&backups[0]).unwrap(), b"[\"truncated\"");
        assert_eq!(
            std::fs::read(&sibling).unwrap(),
            b"{\"enabled\":false}",
            "备份损坏文件不应影响其它配置"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn provider_secret_key_is_stable_and_keyring_safe() {
        assert_eq!(
            provider_secret_key("provider-1"),
            provider_secret_key("provider-1")
        );
        assert_ne!(
            provider_secret_key("provider-1"),
            provider_secret_key("provider-2")
        );
        assert!(provider_secret_key("provider-1").starts_with("stelix-ai-"));
        assert!(ai_secret_entry("").is_err());
    }
}

pub fn save_ai_config(cfg: &AiConfig) -> Result<(), String> {
    save_ai_config_at(&KeyringStore, &ai_config_path(), cfg)
}

fn save_ai_config_at(store: &dyn SecretStore, path: &Path, cfg: &AiConfig) -> Result<(), String> {
    // 密钥写入密钥环，落盘副本清空该字段；失败则保留明文，宁可少一层保护也不丢数据。
    let mut stored = cfg.clone();
    for provider in &mut stored.providers {
        if provider.api_key.is_empty() {
            // 用户清空密钥时同步删除条目，避免下次读取又被旧值填回。
            if let Err(error) = store.delete(&provider.id) {
                eprintln!("清理 AI 密钥失败（{}）：{error}", provider.id);
            }
            continue;
        }
        match store.set(&provider.id, &provider.api_key) {
            Ok(()) => provider.api_key.clear(),
            Err(error) => eprintln!(
                "保存 AI 密钥到系统凭据库失败，退回明文存储（{}）：{error}",
                provider.id
            ),
        }
    }
    let text = serde_json::to_string_pretty(&stored).map_err(|e| e.to_string())?;
    write_atomic(path, text.as_bytes())
}

fn tasks_path() -> PathBuf {
    config_dir().join("tasks.json")
}

/// 读取定时任务列表；文件缺失或损坏时返回空列表，损坏文件先备份。
pub fn load_tasks() -> Vec<ScheduledTask> {
    load_tasks_at(&tasks_path())
}

fn load_tasks_at(path: &Path) -> Vec<ScheduledTask> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    match serde_json::from_str::<Vec<ScheduledTask>>(&text) {
        Ok(tasks) => tasks,
        Err(error) => {
            eprintln!(
                "定时任务无法解析，已备份原文件（{}）：{error}",
                path.display()
            );
            backup_corrupt(path);
            Vec::new()
        }
    }
}

pub fn save_tasks(tasks: &[ScheduledTask]) -> Result<(), String> {
    save_tasks_at(&tasks_path(), tasks)
}

fn save_tasks_at(path: &Path, tasks: &[ScheduledTask]) -> Result<(), String> {
    let text = serde_json::to_string_pretty(tasks).map_err(|e| e.to_string())?;
    write_atomic(path, text.as_bytes())
}

//! Configuration and index persistence.

use std::path::PathBuf;
use std::sync::OnceLock;

use crate::types::{AiConfig, FileEntry, ScheduledTask};
use dashmap::DashMap;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};

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
    CONFIG_DIR.get().cloned().unwrap_or_else(fallback_config_dir)
}

fn ai_config_path() -> PathBuf {
    config_dir().join("ai.json")
}

fn index_cache_path() -> PathBuf {
    LOCAL_DATA_DIR
        .get()
        .cloned()
        .unwrap_or_else(fallback_local_data_dir)
        .join("index-cache.sqlite3")
}

fn legacy_index_cache_path() -> PathBuf {
    index_cache_path().with_extension("json")
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

pub enum IndexChange {
    Upsert(FileEntry),
    DeleteTree(String),
}

pub fn load_index_cache_into(
    roots: &[String],
    entries: &DashMap<String, FileEntry>,
) -> Option<Vec<IndexJournal>> {
    entries.clear();
    if index_cache_path().exists() {
        if let Ok(mut connection) = open_index_db() {
            let stored_roots = connection
                .query_row(
                    "SELECT value FROM cache_meta WHERE key = 'roots'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .ok()
                .flatten()
                .and_then(|text| serde_json::from_str::<Vec<String>>(&text).ok());
            let journals = connection
                .query_row(
                    "SELECT value FROM cache_meta WHERE key = 'journals'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .ok()
                .flatten()
                .and_then(|text| serde_json::from_str::<Vec<IndexJournal>>(&text).ok());
            if stored_roots
                .as_deref()
                .is_some_and(|stored| same_roots(stored, roots))
            {
                let mut statement = connection
                    .prepare("SELECT id, name, path, is_dir, size, modified, ext, hidden, system FROM entries")
                    .ok()?;
                let rows = statement
                    .query_map([], |row| {
                        Ok(FileEntry {
                            id: row.get::<_, i64>(0)? as u64,
                            name: row.get(1)?,
                            path: row.get(2)?,
                            is_dir: row.get::<_, i64>(3)? != 0,
                            size: row.get::<_, i64>(4)? as u64,
                            modified: row.get(5)?,
                            ext: row.get(6)?,
                            hidden: row.get::<_, i64>(7)? != 0,
                            system: row.get::<_, i64>(8)? != 0,
                        })
                    })
                    .ok()?;
                for row in rows {
                    let entry = row.ok()?;
                    entries.insert(entry.path.clone(), entry);
                }
                return journals;
            }
        }
        entries.clear();
    }

    // One-time compatibility path for the previous JSON cache format.
    let text = std::fs::read_to_string(legacy_index_cache_path()).ok()?;
    let cache: IndexCache = serde_json::from_str(&text).ok()?;
    if !same_roots(&cache.roots, roots) {
        return None;
    }
    for entry in cache.entries {
        entries.insert(entry.path.clone(), entry);
    }
    if save_index_cache_stream(
        roots,
        entries.iter().map(|entry| entry.value().clone()),
        &cache.journals,
    )
    .is_err()
    {
        entries.clear();
        return None;
    }
    Some(cache.journals)
}

fn same_roots(left: &[String], right: &[String]) -> bool {
    let normalize = |roots: &[String]| {
        let mut roots: Vec<String> = roots
            .iter()
            .map(|root| root.trim_end_matches(['\\', '/']).to_lowercase())
            .collect();
        roots.sort();
        roots
    };
    normalize(left) == normalize(right)
}

pub fn save_index_cache_stream(
    roots: &[String],
    entries: impl IntoIterator<Item = FileEntry>,
    journals: &[IndexJournal],
) -> Result<(), String> {
    let mut connection = open_index_db()?;
    let transaction = connection.transaction().map_err(|e| e.to_string())?;
    transaction
        .execute("DELETE FROM entries", [])
        .map_err(|e| e.to_string())?;
    {
        let mut statement = transaction
            .prepare_cached("INSERT INTO entries (id,name,path,is_dir,size,modified,ext,hidden,system) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(path) DO UPDATE SET id=excluded.id,name=excluded.name,is_dir=excluded.is_dir,size=excluded.size,modified=excluded.modified,ext=excluded.ext,hidden=excluded.hidden,system=excluded.system")
            .map_err(|e| e.to_string())?;
        for entry in entries {
            insert_entry(&mut statement, &entry).map_err(|e| e.to_string())?;
        }
    }
    set_meta(&transaction, "roots", roots)?;
    set_meta(&transaction, "journals", journals)?;
    transaction.commit().map_err(|e| e.to_string())
}

pub fn apply_index_changes(
    roots: &[String],
    changes: &[IndexChange],
    journals: &[IndexJournal],
) -> Result<(), String> {
    let mut connection = open_index_db()?;
    let transaction = connection.transaction().map_err(|e| e.to_string())?;
    let mut upsert = transaction
        .prepare_cached("INSERT INTO entries (id,name,path,is_dir,size,modified,ext,hidden,system) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(path) DO UPDATE SET id=excluded.id,name=excluded.name,is_dir=excluded.is_dir,size=excluded.size,modified=excluded.modified,ext=excluded.ext,hidden=excluded.hidden,system=excluded.system")
        .map_err(|e| e.to_string())?;
    let mut delete = transaction
        .prepare_cached("DELETE FROM entries WHERE path = ?1 COLLATE NOCASE OR substr(path,1,length(?2)) = ?2 COLLATE NOCASE")
        .map_err(|e| e.to_string())?;
    for change in changes {
        match change {
            IndexChange::Upsert(entry) => {
                insert_entry(&mut upsert, entry).map_err(|e| e.to_string())?;
            }
            IndexChange::DeleteTree(path) => {
                let prefix = format!("{}\\", path.trim_end_matches(['\\', '/']));
                delete
                    .execute(params![path, prefix])
                    .map_err(|e| e.to_string())?;
            }
        }
    }
    drop(upsert);
    drop(delete);
    set_meta(&transaction, "roots", roots)?;
    set_meta(&transaction, "journals", journals)?;
    transaction.commit().map_err(|e| e.to_string())
}

fn insert_entry(
    statement: &mut rusqlite::CachedStatement<'_>,
    entry: &FileEntry,
) -> rusqlite::Result<usize> {
    statement.execute(params![
        entry.id as i64,
        entry.name,
        entry.path,
        if entry.is_dir { 1i64 } else { 0 },
        entry.size as i64,
        entry.modified,
        entry.ext,
        if entry.hidden { 1i64 } else { 0 },
        if entry.system { 1i64 } else { 0 },
    ])
}

fn set_meta<T: Serialize + ?Sized>(
    transaction: &Transaction<'_>,
    key: &str,
    value: &T,
) -> Result<(), String> {
    let value = serde_json::to_string(value).map_err(|e| e.to_string())?;
    transaction
        .execute(
            "INSERT INTO cache_meta (key,value) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, value],
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn open_index_db() -> Result<Connection, String> {
    let path = index_cache_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let connection = Connection::open(path).map_err(|e| e.to_string())?;
    connection
        .execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA temp_store=MEMORY;
             CREATE TABLE IF NOT EXISTS cache_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS entries (
                 id INTEGER NOT NULL,
                 name TEXT NOT NULL,
                 path TEXT PRIMARY KEY,
                 is_dir INTEGER NOT NULL,
                 size INTEGER NOT NULL,
                 modified INTEGER NOT NULL,
                 ext TEXT,
                 hidden INTEGER NOT NULL,
                 system INTEGER NOT NULL
             );",
        )
        .map_err(|e| e.to_string())?;
    Ok(connection)
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

fn tasks_path() -> PathBuf {
    config_dir().join("tasks.json")
}

/// 读取定时任务列表；文件缺失或损坏时返回空列表。
pub fn load_tasks() -> Vec<ScheduledTask> {
    let Ok(text) = std::fs::read_to_string(tasks_path()) else {
        return Vec::new();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

pub fn save_tasks(tasks: &[ScheduledTask]) -> Result<(), String> {
    let dir = config_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let text = serde_json::to_string_pretty(tasks).map_err(|e| e.to_string())?;
    std::fs::write(tasks_path(), text).map_err(|e| e.to_string())
}

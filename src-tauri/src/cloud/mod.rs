//! 云存储：百度网盘（Open API / OAuth2）与迅雷云盘（App API）。
//!
//! 账号令牌只写入系统凭据库（service `stelix.cloud-account`），前端 localStorage
//! 仅保存账号列表（id / 服务商 / 显示名）。目录浏览复用 `list_remote_dir`，
//! 上传下载复用 `remote_transfer`，因此文件窗格、侧栏目录树与拖拽传输无需区分
//! FTP / SFTP 与云存储。

pub mod baidu;
pub mod xunlei;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::credentials;
use crate::fs_core::path_id;
use crate::network::{
    join_relative_remote, join_remote, normalize_remote_dir, remote_ext, remote_file_name,
    safe_local_name, validate_remote_path, RemoteConnection, RemoteRequest, RemoteTransferProgress,
    RemoteTransferRequest, RemoteTransferResult, TransferDirection,
};
use crate::types::{DirListing, FileEntry};

pub const BAIDU_PROTOCOL: &str = "baidu";
pub const XUNLEI_PROTOCOL: &str = "xunlei";

/// 系统凭据库中的云存储账号令牌（JSON）。
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudToken {
    /// 账号显示名（网盘昵称 / 迅雷账号）。
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: String,
    /// 过期时间（Unix 毫秒；0 表示未知，不主动刷新）。
    #[serde(default)]
    pub expires_at: i64,
    /// 百度网盘 OAuth 应用信息（刷新令牌时必需）。
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub client_secret: String,
    /// 迅雷设备 id / 用户 id / 验证码令牌。
    #[serde(default)]
    pub device_id: String,
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub captcha_token: String,
}

/// 云存储账号登录结果（前端据此保存账号列表项）。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudAccountInfo {
    pub account_id: String,
    pub provider: String,
    pub label: String,
    pub username: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BaiduLoginRequest {
    pub account_id: String,
    /// 百度网盘开放平台应用的 AppKey / SecretKey。
    pub client_id: String,
    pub client_secret: String,
    #[serde(default)]
    pub label: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct XunleiLoginRequest {
    pub account_id: String,
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub label: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct XunleiTokenLoginRequest {
    pub account_id: String,
    pub refresh_token: String,
    #[serde(default)]
    pub label: String,
}

/// 云存储目录条目（构造 `FileEntry` 前的中间形态）。
///
/// `id` 是服务商内部 id（百度 fs_id / 迅雷文件 id），下载与删除时使用。
#[derive(Clone)]
pub(crate) struct CloudEntry {
    pub name: String,
    pub path: String,
    pub id: String,
    pub is_dir: bool,
    pub size: u64,
    /// Unix 毫秒
    pub modified: i64,
}

impl CloudEntry {
    fn to_file_entry(&self) -> FileEntry {
        FileEntry {
            id: path_id(&self.path),
            ext: if self.is_dir {
                None
            } else {
                remote_ext(&self.name)
            },
            name: self.name.clone(),
            path: self.path.clone(),
            is_dir: self.is_dir,
            size: if self.is_dir { 0 } else { self.size },
            modified: self.modified,
            hidden: false,
            system: false,
        }
    }
}

pub(crate) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub(crate) fn validate_account_id(account_id: &str) -> Result<String, String> {
    let id = account_id.trim();
    if id.is_empty()
        || id.len() > 128
        || !id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return Err("云存储账号标识无效".into());
    }
    Ok(id.to_string())
}

pub(crate) fn account_id(connection: &RemoteConnection) -> Result<String, String> {
    validate_account_id(&connection.account_id)
}

pub(crate) fn load_token(account_id: &str) -> Result<CloudToken, String> {
    let raw = credentials::get_cloud_token(account_id)?
        .ok_or_else(|| "尚未登录该云存储账号，请重新登录".to_string())?;
    serde_json::from_str(&raw).map_err(|_| "云存储登录信息已损坏，请重新登录".to_string())
}

pub(crate) fn save_token(account_id: &str, token: &CloudToken) -> Result<(), String> {
    let json = serde_json::to_string(token).map_err(|error| format!("令牌序列化失败：{error}"))?;
    credentials::save_cloud_token(account_id, &json)
}

fn delete_token(account_id: &str) -> Result<(), String> {
    credentials::delete_cloud_token(account_id)
}

/// 退出云存储账号：删除系统凭据库中的令牌（账号列表由前端负责）。
pub fn logout(account_id: &str) -> Result<(), String> {
    delete_token(&validate_account_id(account_id)?)
}

/// 云存储会话：两种网盘对外暴露同一组操作，传输流程因此在 `mod.rs` 内复用。
pub(crate) enum Session {
    Baidu(baidu::BaiduSession),
    Xunlei(xunlei::XunleiSession),
}

impl Session {
    async fn open(connection: &RemoteConnection) -> Result<Self, String> {
        match connection.protocol.as_str() {
            BAIDU_PROTOCOL => Ok(Session::Baidu(baidu::BaiduSession::open(connection).await?)),
            XUNLEI_PROTOCOL => Ok(Session::Xunlei(
                xunlei::XunleiSession::open(connection).await?,
            )),
            _ => Err("不支持的云存储类型".into()),
        }
    }

    async fn list(&mut self, path: &str) -> Result<Vec<CloudEntry>, String> {
        match self {
            Session::Baidu(session) => session.list(path).await,
            Session::Xunlei(session) => session.list(path).await,
        }
    }

    async fn lookup(&mut self, path: &str) -> Result<CloudEntry, String> {
        match self {
            Session::Baidu(session) => session.lookup(path).await,
            Session::Xunlei(session) => session.lookup(path).await,
        }
    }

    async fn download(&mut self, entry: &CloudEntry, local: &Path) -> Result<u64, String> {
        match self {
            Session::Baidu(session) => session.download(entry, local).await,
            Session::Xunlei(session) => session.download(entry, local).await,
        }
    }

    async fn upload(&mut self, local: &Path, remote: &str) -> Result<u64, String> {
        match self {
            Session::Baidu(session) => session.upload(local, remote).await,
            Session::Xunlei(session) => session.upload(local, remote).await,
        }
    }

    async fn ensure_dir(&mut self, remote: &str) -> Result<(), String> {
        match self {
            Session::Baidu(session) => session.ensure_dir(remote).await,
            Session::Xunlei(session) => session.ensure_dir(remote).await,
        }
    }

    async fn remove(&mut self, path: &str) -> Result<(), String> {
        match self {
            Session::Baidu(session) => session.remove(path).await,
            Session::Xunlei(session) => session.remove(path).await,
        }
    }
}

/// 列出云存储目录（接入 `list_remote_dir`）。
pub async fn list_remote_dir(request: RemoteRequest) -> Result<DirListing, String> {
    let path = normalize_remote_dir(&request.path);
    validate_remote_path(&path)?;
    let mut session = Session::open(&request.connection).await?;
    let mut entries: Vec<FileEntry> = session
        .list(&path)
        .await?
        .iter()
        .map(CloudEntry::to_file_entry)
        .collect();
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(DirListing {
        path,
        entries,
    })
}

/// 云存储上传 / 下载（接入 `remote_transfer`，目录自动递归）。
pub async fn transfer_remote(
    request: RemoteTransferRequest,
    progress: &mut (dyn FnMut(RemoteTransferProgress) + Send),
) -> RemoteTransferResult {
    let mut result = RemoteTransferResult::default();
    if request.items.is_empty() {
        result.errors.push("没有要传输的条目".into());
        return result;
    }
    let mut session = match Session::open(&request.connection).await {
        Ok(session) => session,
        Err(error) => {
            result.errors.push(error);
            return result;
        }
    };
    match request.direction {
        TransferDirection::Download => {
            download_items(&mut session, &request, &mut result, progress).await;
        }
        TransferDirection::Upload => {
            upload_items(&mut session, &request, &mut result, progress).await;
        }
    }
    result.ok = result.errors.is_empty();
    result
}

async fn download_items(
    session: &mut Session,
    request: &RemoteTransferRequest,
    result: &mut RemoteTransferResult,
    progress: &mut (dyn FnMut(RemoteTransferProgress) + Send),
) {
    let target = request.target.trim();
    if target.is_empty() {
        result.errors.push("下载目标目录为空".into());
        return;
    }
    let target = PathBuf::from(target);
    if let Err(error) = tokio::fs::create_dir_all(&target).await {
        result
            .errors
            .push(format!("无法创建下载目录 {}：{error}", target.display()));
        return;
    }

    let total = request.items.len();
    for (index, item) in request.items.iter().enumerate() {
        progress(RemoteTransferProgress {
            direction: "download".into(),
            done: index,
            total,
            current: item.path.clone(),
        });

        if let Err(error) = validate_remote_path(&item.path) {
            result
                .errors
                .push(format!("下载 {} 失败：{error}", item.path));
            continue;
        }
        let remote = normalize_remote_dir(&item.path);
        let is_root = item.is_dir && remote == "/";
        let local = if is_root {
            target.clone()
        } else {
            let name = match remote_file_name(&remote).and_then(|name| safe_local_name(&name)) {
                Ok(name) => name,
                Err(error) => {
                    result
                        .errors
                        .push(format!("下载 {} 失败：{error}", remote));
                    continue;
                }
            };
            target.join(name)
        };
        let ok = if item.is_dir {
            download_dir(session, &remote, &local, result).await
        } else {
            match session.lookup(&remote).await {
                Ok(entry) => match session.download(&entry, &local).await {
                    Ok(bytes) => {
                        result.bytes += bytes;
                        true
                    }
                    Err(error) => {
                        result
                            .errors
                            .push(format!("下载 {} 失败：{error}", remote));
                        false
                    }
                },
                Err(error) => {
                    result
                        .errors
                        .push(format!("下载 {} 失败：{error}", remote));
                    false
                }
            }
        };
        if ok {
            result.transferred += 1;
            if request.remove_sources {
                if is_root {
                    result.errors.push("已下载，但不会删除云端根目录".into());
                } else if let Err(error) = session.remove(&remote).await {
                    result
                        .errors
                        .push(format!("已下载，但删除云端源 {} 失败：{error}", remote));
                }
            }
        }
    }
}

async fn download_dir(
    session: &mut Session,
    remote_dir: &str,
    local_dir: &Path,
    result: &mut RemoteTransferResult,
) -> bool {
    if let Err(error) = tokio::fs::create_dir_all(local_dir).await {
        result
            .errors
            .push(format!("无法创建目录 {}：{error}", local_dir.display()));
        return false;
    }
    let entries = match session.list(remote_dir).await {
        Ok(entries) => entries,
        Err(error) => {
            result
                .errors
                .push(format!("读取云端目录 {remote_dir} 失败：{error}"));
            return false;
        }
    };

    let mut ok = true;
    for entry in entries {
        let name = match safe_local_name(&entry.name) {
            Ok(name) => name,
            Err(error) => {
                result.errors.push(format!("跳过 {}：{error}", entry.path));
                ok = false;
                continue;
            }
        };
        let local = local_dir.join(name);
        if entry.is_dir {
            if !Box::pin(download_dir(session, &entry.path, &local, result)).await {
                ok = false;
            }
            continue;
        }
        match session.download(&entry, &local).await {
            Ok(bytes) => result.bytes += bytes,
            Err(error) => {
                result
                    .errors
                    .push(format!("下载 {} 失败：{error}", entry.path));
                ok = false;
            }
        }
    }
    ok
}

async fn upload_items(
    session: &mut Session,
    request: &RemoteTransferRequest,
    result: &mut RemoteTransferResult,
    progress: &mut (dyn FnMut(RemoteTransferProgress) + Send),
) {
    let target = normalize_remote_dir(&request.target);
    if let Err(error) = validate_remote_path(&target) {
        result.errors.push(format!("上传目标目录无效：{error}"));
        return;
    }

    let total = request.items.len();
    for (index, item) in request.items.iter().enumerate() {
        progress(RemoteTransferProgress {
            direction: "upload".into(),
            done: index,
            total,
            current: item.path.clone(),
        });

        let local = PathBuf::from(&item.path);
        let meta = match tokio::fs::metadata(&local).await {
            Ok(meta) => meta,
            Err(error) => {
                result
                    .errors
                    .push(format!("读取本地文件 {} 失败：{error}", item.path));
                continue;
            }
        };
        let name = match local.file_name().and_then(|name| name.to_str()) {
            Some(name) if !name.is_empty() => name.to_string(),
            _ => {
                result
                    .errors
                    .push(format!("无法确定 {} 的文件名", item.path));
                continue;
            }
        };
        let remote = join_remote(&target, &name);
        let ok = if meta.is_dir() {
            upload_dir(session, &local, &remote, result).await
        } else {
            match session.upload(&local, &remote).await {
                Ok(bytes) => {
                    result.bytes += bytes;
                    true
                }
                Err(error) => {
                    result
                        .errors
                        .push(format!("上传 {} 失败：{error}", item.path));
                    false
                }
            }
        };
        if ok {
            result.transferred += 1;
            if request.remove_sources {
                let removed = if meta.is_dir() {
                    tokio::fs::remove_dir_all(&local).await
                } else {
                    tokio::fs::remove_file(&local).await
                };
                if let Err(error) = removed {
                    result
                        .errors
                        .push(format!("已上传，但删除本地源 {} 失败：{error}", item.path));
                }
            }
        }
    }
}

/// 递归上传目录：先补齐云端目录（含空目录），再逐个上传文件。
async fn upload_dir(
    session: &mut Session,
    local_root: &Path,
    remote_root: &str,
    result: &mut RemoteTransferResult,
) -> bool {
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    let mut ok = true;
    for entry in walkdir::WalkDir::new(local_root).follow_links(false) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                result.errors.push(format!(
                    "读取本地目录 {} 失败：{error}",
                    local_root.display()
                ));
                ok = false;
                continue;
            }
        };
        let file_type = entry.file_type();
        if file_type.is_symlink() {
            continue;
        }
        let relative = match entry.path().strip_prefix(local_root) {
            Ok(relative) => relative,
            Err(_) => continue,
        };
        let remote = join_relative_remote(remote_root, relative);
        if file_type.is_dir() {
            dirs.push(remote);
        } else if file_type.is_file() {
            files.push((entry.path().to_path_buf(), remote));
        }
    }

    for dir in dirs {
        if let Err(error) = session.ensure_dir(&dir).await {
            result
                .errors
                .push(format!("创建云端目录 {remote_root} 失败：{error}"));
            return false;
        }
    }

    for (local, remote) in files {
        match session.upload(&local, &remote).await {
            Ok(bytes) => result.bytes += bytes,
            Err(error) => {
                result
                    .errors
                    .push(format!("上传 {} 失败：{error}", local.display()));
                ok = false;
            }
        }
    }
    ok
}

/// 云端 POSIX 父目录（根目录返回自身）。
pub(crate) fn parent_path(path: &str) -> String {
    let normalized = normalize_remote_dir(path);
    if normalized == "/" {
        return "/".into();
    }
    match normalized.rfind('/') {
        Some(0) | None => "/".into(),
        Some(index) => normalized[..index].to_string(),
    }
}

/// 把 HTTP 响应流式写入本地文件：先写临时文件，成功后再替换目标，避免失败时破坏已有文件。
pub(crate) async fn save_stream(
    mut response: reqwest::Response,
    local: &Path,
) -> Result<u64, String> {
    use tokio::io::AsyncWriteExt;

    if let Some(parent) = local.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| format!("无法创建目录 {}：{error}", parent.display()))?;
    }
    let temp = local.with_file_name(format!(".stelix-{}.part", uuid::Uuid::new_v4()));
    let mut file = tokio::fs::File::create(&temp)
        .await
        .map_err(|error| format!("无法创建临时文件 {}：{error}", temp.display()))?;
    let mut total = 0u64;
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if let Err(error) = file.write_all(&chunk).await {
                    let _ = tokio::fs::remove_file(&temp).await;
                    return Err(format!("写入 {} 失败：{error}", temp.display()));
                }
                total += chunk.len() as u64;
            }
            Ok(None) => break,
            Err(error) => {
                let _ = tokio::fs::remove_file(&temp).await;
                return Err(format!("下载中断：{error}"));
            }
        }
    }
    if let Err(error) = file.flush().await {
        let _ = tokio::fs::remove_file(&temp).await;
        return Err(format!("写入 {} 失败：{error}", temp.display()));
    }
    drop(file);
    if tokio::fs::symlink_metadata(local).await.is_ok() {
        if let Err(error) = tokio::fs::remove_file(local).await {
            let _ = tokio::fs::remove_file(&temp).await;
            return Err(format!("无法覆盖 {}：{error}", local.display()));
        }
    }
    tokio::fs::rename(&temp, local)
        .await
        .map_err(|error| format!("无法保存 {}：{error}", local.display()))?;
    Ok(total)
}

pub(crate) fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(20))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// 读取 JSON 字段（数字可能是字符串，统一转成 i64）。
pub(crate) fn value_i64(value: &Value, key: &str) -> Option<i64> {
    match value.get(key) {
        Some(Value::Number(number)) => number.as_i64(),
        Some(Value::String(text)) => text.parse::<i64>().ok(),
        _ => None,
    }
}

pub(crate) fn value_str<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

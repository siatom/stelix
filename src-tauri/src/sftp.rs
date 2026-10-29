//! SFTP 目录浏览与文件传输：基于 `russh` + `russh-sftp`，与 SSH 终端共用连接与认证。
//!
//! 不走 curl 的原因：curl 依赖的 libssh2（尤其 Windows 的 WinCNG 构建）对
//! `ssh-ed25519` / `ecdsa-sha2-nistp256` 主机密钥支持有限，连接现代 OpenSSH
//! 服务器会直接失败。这里复用 `ssh.rs` 的连接、认证与 known_hosts 校验逻辑，
//! 只额外打开一个 SFTP 子系统通道。
//!
//! 一次操作（列目录 / 一次传输）只建立一条 SSH 连接，目录递归在连接内完成；
//! 上传前先用 `create_dir_all` 补齐远端目录，下载先写临时文件再原子替换。
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use parking_lot::Mutex;
use russh::client;
use russh_sftp::client::fs::DirEntry;
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::FileType;

use crate::fs_core::path_id;
use crate::network::{
    join_relative_remote, join_remote, normalize_remote_dir, remote_ext, remote_file_name,
    safe_local_name, validate_connection, validate_remote_path, RemoteConnection, RemoteRequest,
    RemoteTransferProgress, RemoteTransferRequest, RemoteTransferResult, TransferDirection,
};
use crate::ssh::{connect_authenticated, ClientHandler, SshAuth};
use crate::types::{DirListing, FileEntry};

/// 单次 SFTP 请求的超时（秒）：慢速网络下大目录 readdir 也可能较久。
const REQUEST_TIMEOUT_SECS: u64 = 60;

/// 一条已认证的 SFTP 连接。
struct SftpClient {
    /// SSH 会话本身：SFTP 通道关闭前必须保活。
    _ssh: client::Handle<ClientHandler>,
    sftp: SftpSession,
    /// 本次连接内已确认存在的远端目录（避免逐文件重复建目录）。
    created_dirs: Mutex<HashSet<String>>,
}

impl SftpClient {
    async fn connect(connection: &RemoteConnection) -> Result<Self, String> {
        validate_connection(connection)?;
        if connection.protocol != "sftp" {
            return Err("仅 SFTP 使用内置实现".into());
        }
        if connection.username.trim().is_empty() {
            return Err("SSH 用户名不能为空".into());
        }

        let auth = SshAuth::from_connection(connection);
        let outcome = Arc::new(Mutex::new(None::<String>));
        let fingerprint = Arc::new(Mutex::new(String::new()));
        let ssh = connect_authenticated(
            &connection.host,
            connection.port,
            &auth,
            outcome,
            fingerprint,
        )
        .await?;

        let channel = ssh
            .channel_open_session()
            .await
            .map_err(|error| format!("无法打开 SSH 会话：{error}"))?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(|error| format!("服务器未启用 SFTP 子系统：{error}"))?;
        let sftp = SftpSession::new(channel.into_stream())
            .await
            .map_err(|error| format!("SFTP 初始化失败：{error}"))?;
        sftp.set_timeout(REQUEST_TIMEOUT_SECS);

        Ok(Self {
            _ssh: ssh,
            sftp,
            created_dirs: Mutex::new(HashSet::new()),
        })
    }

    /// 列出一层目录（目录在前，名称不区分大小写排序）。
    async fn list(&self, path: &str) -> Result<Vec<FileEntry>, String> {
        validate_remote_path(path)?;
        let read_dir = self
            .sftp
            .read_dir(path)
            .await
            .map_err(|error| format!("读取远端目录 {path} 失败：{error}"))?;
        let mut entries: Vec<FileEntry> =
            read_dir.map(|entry| to_file_entry(entry, path)).collect();
        entries.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(entries)
    }

    /// 逐级创建远端目录（已存在的层级跳过）。
    async fn create_dir_all(&self, path: &str) -> Result<(), String> {
        let normalized = normalize_remote_dir(path);
        if normalized == "/" {
            return Ok(());
        }
        if self.created_dirs.lock().contains(&normalized) {
            return Ok(());
        }
        let mut current = String::new();
        for part in normalized.split('/').filter(|part| !part.is_empty()) {
            current.push('/');
            current.push_str(part);
            if self.created_dirs.lock().contains(&current) {
                continue;
            }
            if let Err(error) = self.sftp.create_dir(&current).await {
                // 目录已存在是正常情况（重复上传），只有确实不存在才算失败。
                if !self.sftp.try_exists(&current).await.unwrap_or(false) {
                    return Err(format!("无法创建远端目录 {current}：{error}"));
                }
            }
            self.created_dirs.lock().insert(current.clone());
        }
        Ok(())
    }

    /// 递归删除远端路径（供「剪切」下载后清理源文件）。
    async fn remove_tree(&self, path: &str, is_dir: bool) -> Result<(), String> {
        if normalize_remote_dir(path) == "/" {
            return Err("不能删除远程根目录".into());
        }
        if !is_dir {
            return self
                .sftp
                .remove_file(path)
                .await
                .map_err(|error| format!("无法删除 {path}：{error}"));
        }

        let mut stack = vec![path.to_string()];
        let mut dirs = Vec::new();
        while let Some(dir) = stack.pop() {
            let read_dir = self
                .sftp
                .read_dir(&dir)
                .await
                .map_err(|error| format!("读取远端目录 {dir} 失败：{error}"))?;
            for entry in read_dir {
                let child = join_remote(&dir, &entry.file_name());
                if entry.file_type().is_dir() {
                    stack.push(child);
                } else {
                    self.sftp
                        .remove_file(&child)
                        .await
                        .map_err(|error| format!("无法删除 {child}：{error}"))?;
                }
            }
            dirs.push(dir);
        }
        for dir in dirs.iter().rev() {
            self.sftp
                .remove_dir(dir)
                .await
                .map_err(|error| format!("无法删除目录 {dir}：{error}"))?;
        }
        Ok(())
    }

    /// 下载单个文件：先写临时文件，成功后再替换目标，失败不破坏已有文件。
    async fn download_file(&self, remote: &str, local: &Path) -> Result<u64, String> {
        if let Some(parent) = local.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|error| format!("无法创建目录 {}：{error}", parent.display()))?;
        }
        let temp = local.with_file_name(format!(".stelix-{}.part", uuid::Uuid::new_v4()));

        let copied = async {
            let mut source = self
                .sftp
                .open(remote)
                .await
                .map_err(|error| format!("无法打开远端文件 {remote}：{error}"))?;
            let mut target = tokio::fs::File::create(&temp)
                .await
                .map_err(|error| format!("无法创建 {}：{error}", temp.display()))?;
            let bytes = tokio::io::copy(&mut source, &mut target)
                .await
                .map_err(|error| format!("下载 {remote} 失败：{error}"))?;
            target
                .sync_all()
                .await
                .map_err(|error| format!("写入 {} 失败：{error}", temp.display()))?;
            Ok::<u64, String>(bytes)
        }
        .await;

        let bytes = match copied {
            Ok(bytes) => bytes,
            Err(error) => {
                let _ = tokio::fs::remove_file(&temp).await;
                return Err(error);
            }
        };

        if tokio::fs::symlink_metadata(local).await.is_ok() {
            if let Err(error) = tokio::fs::remove_file(local).await {
                let _ = tokio::fs::remove_file(&temp).await;
                return Err(format!("无法覆盖 {}：{error}", local.display()));
            }
        }
        if let Err(error) = tokio::fs::rename(&temp, local).await {
            let _ = tokio::fs::remove_file(&temp).await;
            return Err(format!("无法保存 {}：{error}", local.display()));
        }
        Ok(bytes)
    }

    /// 上传单个文件（目标目录自动补齐），返回本地文件大小。
    async fn upload_file(&self, local: &Path, remote: &str) -> Result<u64, String> {
        if let Some(parent) = remote_parent(remote) {
            self.create_dir_all(parent).await?;
        }
        let size = tokio::fs::metadata(local)
            .await
            .map_err(|error| format!("读取本地文件 {} 失败：{error}", local.display()))?
            .len();

        let mut source = tokio::fs::File::open(local)
            .await
            .map_err(|error| format!("读取本地文件 {} 失败：{error}", local.display()))?;
        let mut target = self
            .sftp
            .create(remote)
            .await
            .map_err(|error| format!("无法创建远端文件 {remote}：{error}"))?;
        tokio::io::copy(&mut source, &mut target)
            .await
            .map_err(|error| format!("上传 {} 失败：{error}", local.display()))?;
        target
            .close()
            .await
            .map_err(|error| format!("关闭远端文件 {remote} 失败：{error}"))?;
        Ok(size)
    }
}

/// 列出 SFTP 目录。
pub async fn list_remote_dir(request: RemoteRequest) -> Result<DirListing, String> {
    let client = SftpClient::connect(&request.connection).await?;
    let entries = client.list(&request.path).await?;
    Ok(DirListing {
        path: request.path,
        entries,
    })
}

/// 执行一次上传 / 下载。`progress` 在每个顶层条目开始前回调一次。
pub async fn transfer_remote(
    request: RemoteTransferRequest,
    progress: &mut (dyn FnMut(RemoteTransferProgress) + Send),
) -> RemoteTransferResult {
    let mut result = RemoteTransferResult::default();
    if let Err(error) = validate_connection(&request.connection) {
        result.errors.push(error);
        return result;
    }
    if request.connection.protocol != "sftp" {
        result.errors.push("仅 SFTP 使用内置实现".into());
        return result;
    }
    if request.items.is_empty() {
        result.errors.push("没有要传输的条目".into());
        return result;
    }

    let client = match SftpClient::connect(&request.connection).await {
        Ok(client) => client,
        Err(error) => {
            result.errors.push(error);
            return result;
        }
    };

    match request.direction {
        TransferDirection::Download => {
            download_items(&client, &request, &mut result, progress).await;
        }
        TransferDirection::Upload => {
            upload_items(&client, &request, &mut result, progress).await;
        }
    }
    result.ok = result.errors.is_empty();
    result
}

async fn download_items(
    client: &SftpClient,
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
        let is_root = item.is_dir && normalize_remote_dir(&item.path) == "/";
        let local = if is_root {
            target.clone()
        } else {
            let name = match remote_file_name(&item.path).and_then(|name| safe_local_name(&name)) {
                Ok(name) => name,
                Err(error) => {
                    result
                        .errors
                        .push(format!("下载 {} 失败：{error}", item.path));
                    continue;
                }
            };
            target.join(name)
        };

        let ok = if item.is_dir {
            download_tree(client, &item.path, &local, result).await
        } else {
            match client.download_file(&item.path, &local).await {
                Ok(bytes) => {
                    result.bytes += bytes;
                    true
                }
                Err(error) => {
                    result
                        .errors
                        .push(format!("下载 {} 失败：{error}", item.path));
                    false
                }
            }
        };
        if ok {
            result.transferred += 1;
            if request.remove_sources {
                if is_root {
                    result.errors.push("已下载，但不会删除远程根目录".into());
                } else if let Err(error) = client.remove_tree(&item.path, item.is_dir).await {
                    result
                        .errors
                        .push(format!("已下载，但删除远端源 {} 失败：{error}", item.path));
                }
            }
        }
    }
}

/// 递归下载目录，返回是否所有子项都成功。失败的子项会写入 `result.errors`。
async fn download_tree(
    client: &SftpClient,
    remote_dir: &str,
    local_dir: &Path,
    result: &mut RemoteTransferResult,
) -> bool {
    match tokio::fs::create_dir_all(local_dir).await {
        Ok(()) => {}
        Err(error) => {
            result
                .errors
                .push(format!("无法创建目录 {}：{error}", local_dir.display()));
            return false;
        }
    }

    let mut ok = true;
    let mut dirs = vec![(remote_dir.to_string(), local_dir.to_path_buf())];
    while let Some((remote, local)) = dirs.pop() {
        let entries = match client.list(&remote).await {
            Ok(entries) => entries,
            Err(error) => {
                result
                    .errors
                    .push(format!("读取远端目录 {remote} 失败：{error}"));
                ok = false;
                continue;
            }
        };
        for entry in entries {
            let name = match safe_local_name(&entry.name) {
                Ok(name) => name,
                Err(error) => {
                    result.errors.push(format!("跳过 {}：{error}", entry.path));
                    ok = false;
                    continue;
                }
            };
            let target = local.join(name);
            if entry.is_dir {
                // 空目录也要在本地建出来（非空目录随后由下载文件的父目录创建兜底）。
                if let Err(error) = tokio::fs::create_dir_all(&target).await {
                    result
                        .errors
                        .push(format!("无法创建目录 {}：{error}", target.display()));
                    ok = false;
                    continue;
                }
                dirs.push((entry.path, target));
                continue;
            }
            match client.download_file(&entry.path, &target).await {
                Ok(bytes) => result.bytes += bytes,
                Err(error) => {
                    result
                        .errors
                        .push(format!("下载 {} 失败：{error}", entry.path));
                    ok = false;
                }
            }
        }
    }
    ok
}

async fn upload_items(
    client: &SftpClient,
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
            upload_tree(client, &local, &remote, result).await
        } else {
            match client.upload_file(&local, &remote).await {
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

/// 递归上传目录：先补齐远端目录（含空目录），再逐个上传文件。
async fn upload_tree(
    client: &SftpClient,
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

    // 源目录本身也要建（WalkDir 不包含根目录自身）。
    if let Err(error) = client.create_dir_all(remote_root).await {
        result
            .errors
            .push(format!("创建远端目录 {remote_root} 失败：{error}"));
        return false;
    }
    for dir in dirs {
        if let Err(error) = client.create_dir_all(&dir).await {
            result
                .errors
                .push(format!("创建远端目录 {dir} 失败：{error}"));
            ok = false;
        }
    }
    for (local, remote) in files {
        match client.upload_file(&local, &remote).await {
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

/// SFTP 条目 -> 前端 `FileEntry`。
fn to_file_entry(entry: DirEntry, base: &str) -> FileEntry {
    let name = entry.file_name();
    let metadata = entry.metadata();
    let is_dir = matches!(entry.file_type(), FileType::Dir);
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0);
    let path = join_remote(base, &name);

    FileEntry {
        id: path_id(&path),
        ext: if is_dir { None } else { remote_ext(&name) },
        name,
        path,
        is_dir,
        size: if is_dir { 0 } else { metadata.len() },
        modified,
        hidden: false,
        system: false,
    }
}

/// 远端路径的父目录（根目录下返回 `/`）。
fn remote_parent(path: &str) -> Option<&str> {
    let trimmed = path.trim_end_matches('/');
    let index = trimmed.rfind('/')?;
    Some(if index == 0 { "/" } else { &trimmed[..index] })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_remote_parent() {
        assert_eq!(remote_parent("/var/www/index.html"), Some("/var/www"));
        assert_eq!(remote_parent("/index.html"), Some("/"));
        assert_eq!(remote_parent("/var/www/"), Some("/var"));
        assert_eq!(remote_parent("relative.txt"), None);
    }

    /// 真实服务器上的往返测试（CI 默认跳过）。
    ///
    /// 本地准备一个启用了 SFTP 子系统的 SSH 服务器，例如：
    /// `wsl -d Ubuntu -u root -- systemctl start ssh`，然后：
    /// `STELIX_SFTP_TEST_PORT=22 STELIX_SFTP_TEST_USER=stelix
    ///  STELIX_SFTP_TEST_PASSWORD=stelix-test STELIX_SFTP_TEST_HOME=/home/stelix
    ///  cargo test --lib sftp::tests::roundtrips -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "需要真实 SFTP 服务器（环境变量 STELIX_SFTP_TEST_*）"]
    async fn roundtrips_files_against_real_server() {
        use crate::network::{RemoteAuth, RemoteTransferItem, RemoteTransferRequest};

        let port: u16 = std::env::var("STELIX_SFTP_TEST_PORT")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(22);
        let username = std::env::var("STELIX_SFTP_TEST_USER").unwrap_or_else(|_| "stelix".into());
        let password =
            std::env::var("STELIX_SFTP_TEST_PASSWORD").unwrap_or_else(|_| "stelix-test".into());
        let home = std::env::var("STELIX_SFTP_TEST_HOME").unwrap_or_else(|_| "/home/stelix".into());

        let connection = RemoteConnection {
            protocol: "sftp".into(),
            host: "127.0.0.1".into(),
            port,
            username,
            password,
            auth_method: RemoteAuth::Password,
            private_key: String::new(),
            key_passphrase: String::new(),
        };

        let client = SftpClient::connect(&connection)
            .await
            .expect("连接本地 SFTP 服务器失败");

        let remote_dir = join_remote(&home, &format!("stelix-sftp-test-{}", uuid::Uuid::new_v4()));
        let remote_file = join_remote(&remote_dir, "hello.txt");
        let remote_empty = join_remote(&remote_dir, "empty");

        // 上传会自动补齐不存在的父目录。
        let local = std::env::temp_dir().join(format!("stelix-up-{}.txt", uuid::Uuid::new_v4()));
        let content = b"hello sftp \xe4\xbd\xa0\xe5\xa5\xbd\n\x00\x01\x02";
        std::fs::write(&local, content).unwrap();
        let uploaded = client
            .upload_file(&local, &remote_file)
            .await
            .expect("上传失败");
        assert_eq!(uploaded, content.len() as u64);

        client
            .create_dir_all(&remote_empty)
            .await
            .expect("创建空目录失败");

        let entries = client.list(&remote_dir).await.expect("列目录失败");
        let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, vec!["empty", "hello.txt"], "目录应排在文件前");
        let file_entry = entries
            .iter()
            .find(|entry| entry.name == "hello.txt")
            .unwrap();
        assert!(!file_entry.is_dir);
        assert_eq!(file_entry.size, content.len() as u64);
        assert_eq!(file_entry.ext.as_deref(), Some("txt"));
        assert!(file_entry.modified > 0, "修改时间应来自服务器属性");
        let dir_entry = entries.iter().find(|entry| entry.name == "empty").unwrap();
        assert!(dir_entry.is_dir);

        let downloaded =
            std::env::temp_dir().join(format!("stelix-down-{}.txt", uuid::Uuid::new_v4()));
        let bytes = client
            .download_file(&remote_file, &downloaded)
            .await
            .expect("下载失败");
        assert_eq!(bytes, content.len() as u64);
        assert_eq!(std::fs::read(&downloaded).unwrap(), content);

        // 目录递归传输（含进度回调）。
        let local_dir = std::env::temp_dir().join(format!("stelix-tree-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(local_dir.join("nested")).unwrap();
        std::fs::write(local_dir.join("nested").join("a.txt"), b"a").unwrap();
        std::fs::write(local_dir.join("b.txt"), b"bb").unwrap();

        let mut progress = Vec::new();
        let upload = transfer_remote(
            RemoteTransferRequest {
                connection: connection.clone(),
                direction: TransferDirection::Upload,
                target: remote_dir.clone(),
                items: vec![RemoteTransferItem {
                    path: local_dir.to_string_lossy().into_owned(),
                    is_dir: true,
                }],
                remove_sources: false,
            },
            &mut |report| progress.push(report.current),
        )
        .await;
        assert!(upload.ok, "目录上传失败：{:?}", upload.errors);
        assert_eq!(upload.transferred, 1);
        assert_eq!(progress.len(), 1);

        let tree_name = local_dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let tree = client
            .list(&join_remote(&remote_dir, &tree_name))
            .await
            .expect("列出上传后的目录失败");
        let tree_names: Vec<&str> = tree.iter().map(|entry| entry.name.as_str()).collect();
        assert!(tree_names.contains(&"nested"));
        assert!(tree_names.contains(&"b.txt"));

        let download_target =
            std::env::temp_dir().join(format!("stelix-tree-down-{}", uuid::Uuid::new_v4()));
        let download = transfer_remote(
            RemoteTransferRequest {
                connection: connection.clone(),
                direction: TransferDirection::Download,
                target: download_target.to_string_lossy().into_owned(),
                items: vec![RemoteTransferItem {
                    path: remote_dir.clone(),
                    is_dir: true,
                }],
                remove_sources: false,
            },
            &mut |_| {},
        )
        .await;
        assert!(download.ok, "目录下载失败：{:?}", download.errors);
        let root_name = remote_dir.rsplit('/').next().unwrap();
        let downloaded_root = download_target.join(root_name).join(&tree_name);
        assert_eq!(std::fs::read(downloaded_root.join("b.txt")).unwrap(), b"bb");
        assert_eq!(
            std::fs::read(downloaded_root.join("nested").join("a.txt")).unwrap(),
            b"a"
        );

        // 递归删除（含非空目录）。
        client
            .remove_tree(&remote_dir, true)
            .await
            .expect("删除远端目录失败");
        let remaining = client.list(&home).await.expect("列 home 失败");
        assert!(!remaining.iter().any(|entry| entry.path == remote_dir));

        let _ = std::fs::remove_dir_all(&local_dir);
        let _ = std::fs::remove_dir_all(&download_target);
        let _ = std::fs::remove_file(&local);
        let _ = std::fs::remove_file(&downloaded);
    }
}

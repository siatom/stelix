//! FTP 目录浏览与文件传输。凭据仅随本次调用传入 curl 的标准输入，不写入配置文件。
//!
//! SFTP 不走 curl（curl 的 libssh2 构建对 ed25519 / ECDSA 主机密钥支持有限），
//! 由 `sftp.rs` 基于 russh 实现，与 SSH 终端共用连接与认证逻辑。
//!
//! 列目录取 curl 的**长格式**输出（FTP 的 `LIST`），再解析为带类型 / 大小 /
//! 修改时间的 `DirListing`，供文件窗格浏览远程目录。
//! 上传 / 下载同样复用 curl：单文件用 `--output` / `--upload-file`，目录由本模块
//! 递归展开，远端空目录用 `--quote` 补齐。
//! 解析与路径规则全部是纯函数（`parse_listing` / `build_remote_url` 等），便于用固定
//! 样本单测覆盖。
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use chrono::{Datelike, Local, NaiveDate};
use serde::{Deserialize, Serialize};

use crate::fs_core::path_id;
use crate::types::{DirListing, FileEntry};

/// 远程认证方式。
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RemoteAuth {
    /// 用户名 + 密码（也兼容匿名 FTP：空用户名 / 空密码）。
    #[default]
    Password,
    /// SSH 私钥（可带密码短语）；仅 SFTP 可用。
    Key,
}

/// 一次远程连接所需的凭据与地址。
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteConnection {
    pub protocol: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    #[serde(default)]
    pub password: String,
    /// 认证方式；缺省为密码，兼容旧版前端与旧配置。
    #[serde(default)]
    pub auth_method: RemoteAuth,
    /// 私钥文件路径（`authMethod = "key"` 时必填）。
    #[serde(default)]
    pub private_key: String,
    /// 私钥密码短语（没有密码短语时为空）。
    #[serde(default)]
    pub key_passphrase: String,
    /// 云存储账号 id（`protocol = "baidu" | "xunlei"` 时使用，令牌存放在系统凭据库）。
    #[serde(default)]
    pub account_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteRequest {
    #[serde(flatten)]
    pub connection: RemoteConnection,
    pub path: String,
}

/// 传输方向：从远端下载到本地 / 从本地上传到远端。
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TransferDirection {
    Download,
    Upload,
}

/// 一个待传输条目（远端路径或本地路径，取决于方向）。
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteTransferItem {
    pub path: String,
    #[serde(default)]
    pub is_dir: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteTransferRequest {
    #[serde(flatten)]
    pub connection: RemoteConnection,
    pub direction: TransferDirection,
    /// 下载时为本地目标目录；上传时为远端目标目录（POSIX）。
    pub target: String,
    pub items: Vec<RemoteTransferItem>,
    /// 传输成功后删除源（用于「剪切 / 移动」；跨设备时逐条成功才删除）。
    #[serde(default)]
    pub remove_sources: bool,
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteTransferResult {
    pub ok: bool,
    pub errors: Vec<String>,
    /// 成功传输的顶层条目数。
    pub transferred: usize,
    /// 成功传输的字节数。
    pub bytes: u64,
}

/// 传输进度（按顶层条目推进；目录内部递归时 `current` 指向正在处理的路径）。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteTransferProgress {
    pub direction: String,
    pub done: usize,
    pub total: usize,
    pub current: String,
}

/// 一行列表解析出的属性（构造 `FileEntry` 前的中间形态）。
#[derive(Debug, PartialEq)]
struct ParsedLine {
    name: String,
    is_dir: bool,
    size: u64,
    modified: i64,
}

const MONTHS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];

fn quote_config(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r");
    format!("\"{escaped}\"")
}

pub(crate) fn validate_connection(connection: &RemoteConnection) -> Result<(), String> {
    if connection.protocol != "ftp" && connection.protocol != "sftp" {
        return Err("仅支持 FTP 和 SFTP".into());
    }
    if connection.host.is_empty() || connection.host.contains(['/', '\\', '@', ':', '\n', '\r']) {
        return Err("主机地址无效".into());
    }
    if connection.port == 0 {
        return Err("端口无效".into());
    }
    Ok(())
}

/// 生成 curl 配置里的认证段（仅 FTP 走 curl；SFTP 由 `sftp.rs` 直连）。
///
/// 凭据只随 `--config -` 从标准输入传入，不会出现在进程命令行里。
fn curl_auth_config(connection: &RemoteConnection) -> String {
    format!(
        "user = {}\n",
        quote_config(&format!("{}:{}", connection.username, connection.password))
    )
}

/// 远程路径校验：只接受 POSIX 绝对路径，拒绝 `..` 与控制字符（防目录穿越 / 配置注入）。
pub(crate) fn validate_remote_path(path: &str) -> Result<(), String> {
    if path.contains(['\n', '\r', '\0']) {
        return Err("远程路径包含非法字符".into());
    }
    if path.split('/').any(|part| part == "..") {
        return Err("远程路径不能包含 ..".into());
    }
    Ok(())
}

/// 拼出指向远端目录或文件的 URL。
///
/// `directory = true` 时追加结尾 `/`（列目录 / 作为 quote 操作的锚点）；
/// 文件 URL 不能带结尾斜杠，否则服务器会尝试列目录而不是传输文件。
fn build_remote_url(
    connection: &RemoteConnection,
    path: &str,
    directory: bool,
) -> Result<reqwest::Url, String> {
    validate_connection(connection)?;
    validate_remote_path(path)?;

    let mut url = reqwest::Url::parse(&format!(
        "{}://{}:{}/",
        connection.protocol, connection.host, connection.port
    ))
    .map_err(|_| "主机地址无效")?;
    {
        let mut segments = url.path_segments_mut().map_err(|_| "远程路径无效")?;
        segments.clear();
        for part in path.split('/').filter(|part| !part.is_empty()) {
            segments.push(part);
        }
        if directory {
            segments.push("");
        }
    }
    Ok(url)
}

/// 运行 curl 时使用的可执行文件（FTP 用；SFTP 已改为 russh 内置实现）。
///
/// 允许通过环境变量 `STELIX_CURL` 指定自定义 curl 路径。
fn curl_program() -> PathBuf {
    match std::env::var("STELIX_CURL") {
        Ok(custom) if !custom.trim().is_empty() => PathBuf::from(custom.trim()),
        // Windows 自带的是 curl.exe；macOS / Linux 使用 PATH 中的 curl。
        _ => PathBuf::from(if cfg!(windows) { "curl.exe" } else { "curl" }),
    }
}

/// 运行 curl（连接参数经 stdin 传入，避免密码出现在进程命令行里）。
fn run_curl(config: &str, args: &[&str]) -> Result<Vec<u8>, String> {
    let program = curl_program();
    let mut command = Command::new(&program);
    command
        .args([
            "--config",
            "-",
            "--silent",
            "--show-error",
            "--fail",
            "--no-progress-meter",
        ])
        .args(args);
    // CREATE_NO_WINDOW：GUI 进程启动控制台程序时若不指定，会弹出短暂的控制台窗口。
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("无法启动 curl（{}）：{error}", program.display()))?;
    child
        .stdin
        .take()
        .ok_or("无法传递连接参数")?
        .write_all(config.as_bytes())
        .map_err(|error| format!("无法传递连接参数：{error}"))?;
    let output = child
        .wait_with_output()
        .map_err(|error| format!("远程传输失败：{error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(output.stdout)
}

/// 运行 curl 取目录长格式列表（返回 stdout 原始字节，可能不是 UTF-8）。
///
/// 不带 `--list-only`：FTP 会走 `LIST`，从而带上类型与大小；
/// 仅返回名字的 NLST 无法区分目录与文件。
fn run_curl_listing(connection: &RemoteConnection, path: &str) -> Result<Vec<u8>, String> {
    let url = build_remote_url(connection, path, true)?;
    let config = format!(
        "url = {}\n{}",
        quote_config(url.as_str()),
        curl_auth_config(connection)
    );
    run_curl(&config, &["--max-time", "20"]).map_err(|error| format!("连接或目录读取失败：{error}"))
}

/// UTF-8 优先，失败时用 lossy 兜底（中文 FTP 服务器的 GBK 文件名会显示为替换字符）。
fn decode(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// 列出 FTP 目录（SFTP 由 `sftp::list_remote_dir` 处理）。
pub fn list_remote_dir(request: RemoteRequest) -> Result<DirListing, String> {
    if request.connection.protocol != "ftp" {
        return Err("SFTP 目录读取请使用内置 SSH 实现".into());
    }
    let raw = run_curl_listing(&request.connection, &request.path)?;
    let text = decode(&raw);
    Ok(DirListing {
        path: request.path.clone(),
        entries: parse_listing(&text, &request.path),
    })
}

// ---------------------------------------------------------------- 文件传输

/// 执行一次上传 / 下载。`progress` 在每个顶层条目开始前回调一次。
pub fn transfer_remote(
    request: RemoteTransferRequest,
    progress: &mut dyn FnMut(RemoteTransferProgress),
) -> RemoteTransferResult {
    let mut result = RemoteTransferResult::default();
    if let Err(error) = validate_connection(&request.connection) {
        result.errors.push(error);
        return result;
    }
    if request.items.is_empty() {
        result.errors.push("没有要传输的条目".into());
        return result;
    }

    match request.direction {
        TransferDirection::Download => {
            download_items(&request, &mut result, progress);
        }
        TransferDirection::Upload => {
            upload_items(&request, &mut result, progress);
        }
    }
    result.ok = result.errors.is_empty();
    result
}

fn download_items(
    request: &RemoteTransferRequest,
    result: &mut RemoteTransferResult,
    progress: &mut dyn FnMut(RemoteTransferProgress),
) {
    let target = request.target.trim();
    if target.is_empty() {
        result.errors.push("下载目标目录为空".into());
        return;
    }
    let target = PathBuf::from(target);
    if let Err(error) = fs::create_dir_all(&target) {
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
            download_dir(&request.connection, &item.path, &local, result)
        } else {
            match download_file(&request.connection, &item.path, &local) {
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
                } else if let Err(error) =
                    delete_remote_tree(&request.connection, &item.path, item.is_dir)
                {
                    result
                        .errors
                        .push(format!("已下载，但删除远端源 {} 失败：{error}", item.path));
                }
            }
        }
    }
}

/// 递归下载目录，返回是否所有子项都成功。失败的子项会写入 `result.errors`。
fn download_dir(
    connection: &RemoteConnection,
    remote_dir: &str,
    local_dir: &Path,
    result: &mut RemoteTransferResult,
) -> bool {
    if let Err(error) = fs::create_dir_all(local_dir) {
        result
            .errors
            .push(format!("无法创建目录 {}：{error}", local_dir.display()));
        return false;
    }
    let listing = match list_remote_dir(RemoteRequest {
        connection: connection.clone(),
        path: remote_dir.to_string(),
    }) {
        Ok(listing) => listing,
        Err(error) => {
            result
                .errors
                .push(format!("读取远端目录 {remote_dir} 失败：{error}"));
            return false;
        }
    };

    let mut ok = true;
    for entry in listing.entries {
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
            if !download_dir(connection, &entry.path, &local, result) {
                ok = false;
            }
            continue;
        }
        match download_file(connection, &entry.path, &local) {
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

fn download_file(
    connection: &RemoteConnection,
    remote_path: &str,
    local_path: &Path,
) -> Result<u64, String> {
    if let Some(parent) = local_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("无法创建目录 {}：{error}", parent.display()))?;
    }
    // 先写临时文件：失败时不会破坏目标位置已有的同名文件，成功后再替换。
    let temp = local_path.with_file_name(format!(".stelix-{}.part", uuid::Uuid::new_v4()));
    let url = build_remote_url(connection, remote_path, false)?;
    let config = format!(
        "{}output = {}\nurl = {}\n",
        curl_auth_config(connection),
        quote_config(&temp.to_string_lossy()),
        quote_config(url.as_str())
    );
    if let Err(error) = run_curl(&config, &["--connect-timeout", "20"]) {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    let bytes = fs::metadata(&temp).map(|meta| meta.len()).unwrap_or(0);
    if fs::symlink_metadata(local_path).is_ok() {
        if let Err(error) = fs::remove_file(local_path) {
            let _ = fs::remove_file(&temp);
            return Err(format!("无法覆盖 {}：{error}", local_path.display()));
        }
    }
    fs::rename(&temp, local_path)
        .map_err(|error| format!("无法保存 {}：{error}", local_path.display()))?;
    Ok(bytes)
}

fn upload_items(
    request: &RemoteTransferRequest,
    result: &mut RemoteTransferResult,
    progress: &mut dyn FnMut(RemoteTransferProgress),
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
        let meta = match fs::metadata(&local) {
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
            upload_dir(&request.connection, &local, &remote, result)
        } else {
            match upload_file(&request.connection, &local, &remote) {
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
                    fs::remove_dir_all(&local)
                } else {
                    fs::remove_file(&local)
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
fn upload_dir(
    connection: &RemoteConnection,
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

    if let Err(error) = create_remote_dirs(connection, &dirs) {
        result
            .errors
            .push(format!("创建远端目录 {remote_root} 失败：{error}"));
        return false;
    }

    for (local, remote) in files {
        match upload_file(connection, &local, &remote) {
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

fn upload_file(
    connection: &RemoteConnection,
    local_path: &Path,
    remote_path: &str,
) -> Result<u64, String> {
    let url = build_remote_url(connection, remote_path, false)?;
    let size = fs::metadata(local_path).map(|meta| meta.len()).unwrap_or(0);
    let config = format!(
        "{}ftp-create-dirs\nupload-file = {}\nurl = {}\n",
        curl_auth_config(connection),
        quote_config(&local_path.to_string_lossy()),
        quote_config(url.as_str())
    );
    run_curl(&config, &["--connect-timeout", "20"])?;
    Ok(size)
}

/// 用 `--quote MKD` 一次性补齐上传所需的全部远端目录。
///
/// `*` 前缀让 curl 忽略「目录已存在」等单条失败，避免重复上传时整体中止。
fn create_remote_dirs(connection: &RemoteConnection, dirs: &[String]) -> Result<(), String> {
    if dirs.is_empty() {
        return Ok(());
    }
    let root = build_remote_url(connection, "/", true)?;
    let mut config = curl_auth_config(connection);
    for dir in dirs {
        config.push_str(&format!(
            "quote = {}\n",
            quote_config(&format!("*MKD {dir}"))
        ));
    }
    config.push_str(&format!("url = {}\n", quote_config(root.as_str())));
    run_curl(&config, &[])?;
    Ok(())
}

/// 递归删除远端路径（供「剪切」下载后清理源文件）。
fn delete_remote_tree(
    connection: &RemoteConnection,
    path: &str,
    is_dir: bool,
) -> Result<(), String> {
    if normalize_remote_dir(path) == "/" {
        return Err("不能删除远程根目录".into());
    }
    let mut commands = Vec::new();
    collect_delete_commands(connection, path, is_dir, &mut commands)?;
    run_quote_commands(connection, &commands)
}

fn collect_delete_commands(
    connection: &RemoteConnection,
    path: &str,
    is_dir: bool,
    commands: &mut Vec<String>,
) -> Result<(), String> {
    if is_dir {
        let listing = list_remote_dir(RemoteRequest {
            connection: connection.clone(),
            path: path.to_string(),
        })?;
        for entry in listing.entries {
            collect_delete_commands(connection, &entry.path, entry.is_dir, commands)?;
        }
        commands.push(format!("RMD {path}"));
    } else {
        commands.push(format!("DELE {path}"));
    }
    Ok(())
}

fn run_quote_commands(connection: &RemoteConnection, commands: &[String]) -> Result<(), String> {
    if commands.is_empty() {
        return Ok(());
    }
    let root = build_remote_url(connection, "/", true)?;
    let mut config = curl_auth_config(connection);
    for command in commands {
        config.push_str(&format!("quote = {}\n", quote_config(command)));
    }
    config.push_str(&format!("url = {}\n", quote_config(root.as_str())));
    run_curl(&config, &[])?;
    Ok(())
}

/// 远端 POSIX 目录规范化：保证以 `/` 开头且不以 `/` 结尾（根目录除外）。
pub(crate) fn normalize_remote_dir(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() || trimmed == "/" {
        return "/".into();
    }
    let trimmed = trimmed.trim_start_matches('/').trim_end_matches('/');
    if trimmed.is_empty() {
        "/".into()
    } else {
        format!("/{trimmed}")
    }
}

/// 远端路径的最后一段（去掉结尾斜杠）。
pub(crate) fn remote_file_name(path: &str) -> Result<String, String> {
    let name = path.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    if name.is_empty() || name == "." || name == ".." {
        return Err("远程文件名无效".into());
    }
    Ok(name.to_string())
}

/// 把远端名字转换为安全的本地文件名：替换 Windows 非法字符，避开保留设备名。
pub(crate) fn safe_local_name(name: &str) -> Result<String, String> {
    if name.is_empty() || name == "." || name == ".." {
        return Err("文件名无效".into());
    }
    if name.contains(['/', '\\', '\0']) {
        return Err("文件名包含路径分隔符".into());
    }
    let mut cleaned: String = name
        .chars()
        .map(|ch| match ch {
            '<' | '>' | ':' | '"' | '|' | '?' | '*' => '_',
            ch if ch.is_control() => '_',
            ch => ch,
        })
        .collect();
    cleaned = cleaned.trim_end_matches([' ', '.']).to_string();
    if cleaned.is_empty() {
        return Err("文件名为空".into());
    }
    let stem = cleaned
        .split('.')
        .next()
        .unwrap_or(&cleaned)
        .to_ascii_uppercase();
    if matches!(
        stem.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    ) {
        cleaned = format!("_{cleaned}");
    }
    Ok(cleaned)
}

/// 把本地相对路径拼到远端目录下（本地分隔符统一转成 POSIX `/`）。
pub(crate) fn join_relative_remote(remote_root: &str, relative: &Path) -> String {
    let mut path = normalize_remote_dir(remote_root);
    for part in relative.components() {
        if let std::path::Component::Normal(value) = part {
            let value = value.to_string_lossy();
            if value.is_empty() {
                continue;
            }
            path = join_remote(&path, &value);
        }
    }
    path
}

/// 把长格式列表解析为条目：目录在前，再按名称小写排序（与本地 `fs_core::list_dir` 一致）。
pub fn parse_listing(raw: &str, base: &str) -> Vec<FileEntry> {
    let now_year = Local::now().year();
    let mut parsed: Vec<ParsedLine> = Vec::new();
    let mut candidates = 0usize;

    for line in raw.lines() {
        let line = line.trim_end_matches('\r');
        let trimmed = line.trim();
        if trimmed.is_empty() || is_total_line(trimmed) {
            continue;
        }
        candidates += 1;
        if let Some(entry) = parse_unix_line(line, now_year).or_else(|| parse_dos_line(line)) {
            // 部分 SFTP/FTP 服务器会在 readdir / LIST 里返回 `.` 与 `..`，它们不是可传输条目。
            if entry.name != "." && entry.name != ".." {
                parsed.push(entry);
            }
        }
    }

    // 兜底：整份输出没有一行能识别（服务器用了非 Unix / 非 DOS 方言）时，
    // 退回旧的 NLST 语义 —— 整行当名字、全部视为目录，至少保住左侧树能展开。
    if parsed.is_empty() && candidates > 0 {
        parsed = raw
            .lines()
            .map(|line| line.trim_end_matches('\r').trim())
            .filter(|line| {
                !line.is_empty()
                    && !is_total_line(line)
                    && *line != "."
                    && *line != ".."
                    && !line.contains(['/', '\\'])
                    && !looks_like_listing_line(line)
            })
            .map(|line| ParsedLine {
                name: line.to_string(),
                is_dir: true,
                size: 0,
                modified: 0,
            })
            .collect();
    }

    let mut entries: Vec<FileEntry> = parsed
        .into_iter()
        .map(|entry| to_file_entry(entry, base))
        .collect();
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    entries
}

fn to_file_entry(entry: ParsedLine, base: &str) -> FileEntry {
    let path = join_remote(base, &entry.name);
    FileEntry {
        id: path_id(&path),
        ext: if entry.is_dir {
            None
        } else {
            remote_ext(&entry.name)
        },
        name: entry.name,
        path,
        is_dir: entry.is_dir,
        size: if entry.is_dir { 0 } else { entry.size },
        modified: entry.modified,
        hidden: false,
        system: false,
    }
}

/// `total 8` 之类的统计行（部分服务器会本地化首词，此时不足以识别，但也不会被误解析）。
fn is_total_line(line: &str) -> bool {
    let mut parts = line.split_whitespace();
    parts.next() == Some("total")
        && parts
            .next()
            .map(|value| value.parse::<u64>().is_ok())
            .unwrap_or(false)
}

/// POSIX 拼接（`base` 为 `/` 或空时返回 `/name`）。
pub(crate) fn join_remote(base: &str, name: &str) -> String {
    let name = name.trim_start_matches('/');
    let base = base.trim_end_matches('/');
    if base.is_empty() {
        format!("/{name}")
    } else {
        format!("{base}/{name}")
    }
}

pub(crate) fn remote_ext(name: &str) -> Option<String> {
    let index = name.rfind('.')?;
    if index == 0 || index == name.len() - 1 {
        return None;
    }
    Some(name[index + 1..].to_lowercase())
}

/// 带字节偏移的分词（名字可能含空格，因此不能只靠 `split_whitespace`）。
fn tokens_with_offsets(line: &str) -> Vec<(&str, usize)> {
    let bytes = line.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index].is_ascii_whitespace() {
            index += 1;
            continue;
        }
        let start = index;
        while index < bytes.len() && !bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        tokens.push((&line[start..index], start));
    }
    tokens
}

fn month_number(token: &str) -> Option<u32> {
    MONTHS
        .iter()
        .position(|month| token.eq_ignore_ascii_case(month))
        .map(|index| index as u32 + 1)
}

/// `HH:MM`（`ls -l` 的「当年」写法）。
fn parse_hhmm(token: &str) -> Option<(u32, u32)> {
    let (hour, minute) = token.split_once(':')?;
    let hour = hour.parse::<u32>().ok()?;
    let minute = minute.parse::<u32>().ok()?;
    if hour > 23 || minute > 59 {
        return None;
    }
    Some((hour, minute))
}

/// 校验 Unix 权限串：首字符为类型，随后 9 位为权限位；尾随的 ACL `+`、SELinux `.`、
/// macOS `@` 等扩展标记忽略（这些标记不在权限位白名单内，但仍应视为合法行）。
/// 返回类型字符。
fn parse_permission_kind(perms: &str) -> Option<char> {
    if perms.len() < 10 {
        return None;
    }
    let mut chars = perms.chars();
    let kind = chars.next()?;
    if !matches!(kind, 'd' | '-' | 'l' | 'b' | 'c' | 'p' | 's' | 'D') {
        return None;
    }
    if !chars
        .take(9)
        .all(|c| matches!(c, 'r' | 'w' | 'x' | '-' | 's' | 'S' | 't' | 'T'))
    {
        return None;
    }
    Some(kind)
}

/// 该行是否长得像结构化列表（权限串开头，或 DOS 方言的日期 + 时间列）。
/// 兜底时用它排除原始行，避免把权限、大小、时间整行当成目录名。
fn looks_like_listing_line(line: &str) -> bool {
    let mut tokens = line.split_whitespace();
    let Some(first) = tokens.next() else {
        return false;
    };
    if parse_permission_kind(first).is_some() {
        return true;
    }
    match tokens.next() {
        Some(second) => parse_dos_date(first).is_some() && parse_dos_time(second).is_some(),
        None => false,
    }
}

/// 解析 Unix 长格式行（FTP `LIST` 与 SFTP 长名通用）。
///
/// 字段：`权限 链接数 属主 [属组] 大小 月份 日 时间|年份 名字…`
/// —— 属组可能缺失，故用「月份字段的位置」定位大小与名字，而非固定下标。
fn parse_unix_line(line: &str, now_year: i32) -> Option<ParsedLine> {
    let tokens = tokens_with_offsets(line);
    let perms = tokens.first()?.0;
    let kind = parse_permission_kind(perms)?;

    let mut found = None;
    for (index, (token, _)) in tokens.iter().enumerate().skip(1) {
        if let Some(month) = month_number(token) {
            found = Some((index, month));
            break;
        }
    }
    let (month_pos, month) = found?;
    if month_pos < 2 || tokens.len() < month_pos + 4 {
        return None;
    }
    // 校验「日 + 时间|年份」以排除把属主名误当月份的情况。
    let day = tokens[month_pos + 1].0.parse::<u32>().ok()?;
    if !(1..=31).contains(&day) {
        return None;
    }
    let stamp = tokens[month_pos + 2].0;
    if parse_hhmm(stamp).is_none() && stamp.parse::<i32>().is_err() {
        return None;
    }

    let size = tokens[month_pos - 1].0.parse::<u64>().unwrap_or(0);
    let mut name = line[tokens[month_pos + 3].1..].trim().to_string();
    if kind == 'l' {
        // 符号链接：只取链接名（`name -> target`），并按非目录处理以防目录环。
        if let Some(arrow) = name.find(" -> ") {
            name.truncate(arrow);
        }
    }
    if name.is_empty() {
        return None;
    }

    let (year, hour, minute) = match parse_hhmm(stamp) {
        Some((hour, minute)) => (now_year, hour, minute),
        None => (stamp.parse::<i32>().ok()?, 0, 0),
    };
    let modified = NaiveDate::from_ymd_opt(year, month, day)
        .and_then(|date| date.and_hms_opt(hour, minute, 0))
        .map(|stamp| stamp.and_utc().timestamp_millis())
        .unwrap_or(0);

    Some(ParsedLine {
        name,
        is_dir: kind == 'd',
        size,
        modified,
    })
}

/// 解析 IIS 等 Windows FTP 服务器输出的 DOS 方言行：`MM-DD-YY HH:MMPP 大小|<DIR> 名字`。
fn parse_dos_line(line: &str) -> Option<ParsedLine> {
    let tokens = tokens_with_offsets(line);
    if tokens.len() < 4 {
        return None;
    }
    let (year, month, day) = parse_dos_date(tokens[0].0)?;
    let (hour, minute) = parse_dos_time(tokens[1].0)?;
    let marker = tokens[2].0.to_ascii_uppercase();
    let is_dir = matches!(marker.as_str(), "<DIR>" | "<SYMLINK>" | "<JUNCTION>");
    let size = if is_dir {
        0
    } else {
        tokens[2].0.parse::<u64>().ok()?
    };
    let name = line[tokens[3].1..].trim().to_string();
    if name.is_empty() {
        return None;
    }
    let modified = NaiveDate::from_ymd_opt(year, month, day)
        .and_then(|date| date.and_hms_opt(hour, minute, 0))
        .map(|stamp| stamp.and_utc().timestamp_millis())
        .unwrap_or(0);
    Some(ParsedLine {
        name,
        is_dir,
        size,
        modified,
    })
}

/// `MM-DD-YY` 或 `MM-DD-YYYY`。
fn parse_dos_date(token: &str) -> Option<(i32, u32, u32)> {
    let parts: Vec<&str> = token.split('-').collect();
    if parts.len() != 3 {
        return None;
    }
    let month = parts[0].parse::<u32>().ok()?;
    let day = parts[1].parse::<u32>().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let raw = parts[2].parse::<i32>().ok()?;
    let year = match parts[2].len() {
        2 => {
            if raw < 70 {
                2000 + raw
            } else {
                1900 + raw
            }
        }
        4 => raw,
        _ => return None,
    };
    Some((year, month, day))
}

/// `HH:MM` / `HH:MMAM` / `HH:MMPM`。
fn parse_dos_time(token: &str) -> Option<(u32, u32)> {
    let upper = token.to_ascii_uppercase();
    let meridiem = if upper.ends_with("PM") {
        Some(true)
    } else if upper.ends_with("AM") {
        Some(false)
    } else {
        None
    };
    let body = match meridiem {
        Some(_) => &upper[..upper.len() - 2],
        None => upper.as_str(),
    };
    let (hour, minute) = body.split_once(':')?;
    let mut hour = hour.trim().parse::<u32>().ok()?;
    let minute = minute.trim().parse::<u32>().ok()?;
    if let Some(pm) = meridiem {
        if hour == 12 {
            hour = 0;
        }
        if pm {
            hour += 12;
        }
    }
    if hour > 23 || minute > 59 {
        return None;
    }
    Some((hour, minute))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(entries: &[FileEntry]) -> Vec<&str> {
        entries.iter().map(|entry| entry.name.as_str()).collect()
    }

    #[test]
    fn parses_unix_listing_with_dirs_files_and_spaces() {
        let raw = "total 12\r\n\
                   drwxr-xr-x    2 root     root         4096 Oct 12 09:30 public_html\r\n\
                   -rw-r--r--    1 root     root         1234 Oct 12 09:31 index.html\r\n\
                   -rw-r--r--    1 root     root           42 Jan  1  2024 我的 笔记.txt\r\n";
        let entries = parse_listing(raw, "/var/www");
        // 目录在前
        assert_eq!(
            names(&entries),
            vec!["public_html", "index.html", "我的 笔记.txt"]
        );
        assert!(entries[0].is_dir);
        assert_eq!(entries[0].path, "/var/www/public_html");
        assert_eq!(entries[0].size, 0);
        assert!(entries[0].ext.is_none());
        assert!(!entries[1].is_dir);
        assert_eq!(entries[1].size, 1234);
        assert_eq!(entries[1].path, "/var/www/index.html");
        assert_eq!(entries[1].ext.as_deref(), Some("html"));
        assert_eq!(entries[2].name, "我的 笔记.txt");
        assert_eq!(entries[2].ext.as_deref(), Some("txt"));
        assert!(entries[1].modified > 0);
    }

    #[test]
    fn parses_unix_listing_without_group_column() {
        let raw = "-rw-r--r--   1 owner 4096 Nov 20 2025 report.pdf\n";
        let entries = parse_listing(raw, "/");
        assert_eq!(names(&entries), vec!["report.pdf"]);
        assert_eq!(entries[0].size, 4096);
        assert_eq!(entries[0].path, "/report.pdf");
    }

    #[test]
    fn parses_sftp_longname_and_symlink() {
        let raw = "drwxr-xr-x    3 1000     1000         4096 Sep 30 08:00 www\n\
                   -rw-r--r--    1 1000     1000           10 Sep 30 08:00 .env\n\
                   lrwxrwxrwx    1 1000     1000           11 Sep 30 08:00 current -> www\n";
        let entries = parse_listing(raw, "/srv");
        assert!(entries.iter().all(|entry| !entry.name.contains("->")));
        let link = entries
            .iter()
            .find(|entry| entry.name == "current")
            .expect("符号链接条目");
        assert!(!link.is_dir, "符号链接按非目录处理以防目录环");
        assert_eq!(link.path, "/srv/current");
    }

    #[test]
    fn parses_dos_listing_so_it_works_with_iis_style_servers() {
        let raw = "04-27-17  03:25PM       <DIR>          logs\n\
                   04-27-17  03:25PM                 1234 backup log.txt\n";
        let entries = parse_listing(raw, "/");
        assert_eq!(names(&entries), vec!["logs", "backup log.txt"]);
        assert!(entries[0].is_dir);
        assert_eq!(entries[0].size, 0);
        assert!(!entries[1].is_dir);
        assert_eq!(entries[1].size, 1234);
        // 04-27-2017 15:25（PM）UTC
        assert_eq!(entries[0].modified, 1493306700000);
    }

    #[test]
    fn parses_unix_listing_with_acl_selinux_and_macos_markers() {
        // 权限串末尾带 ACL `+`、SELinux `.`、macOS `@` 标记时仍应正常解析，
        // 名字不能混入权限串，也不能触发整行当目录名的兜底。
        let raw = "drwxr-xr-x    2 root root 4096 Oct 12 09:30 logs\n\
                   drwxr-xr-x.   2 root root 4096 Oct 12 09:30 selinux\n\
                   -rw-r--r--+   1 root root 1234 Oct 12 09:31 acl.txt\n\
                   -rw-r--r--@   1 root root   42 Oct 12 09:32 mac.txt\n";
        let entries = parse_listing(raw, "/var/www");
        assert_eq!(
            names(&entries),
            vec!["logs", "selinux", "acl.txt", "mac.txt"]
        );
        assert!(entries[0].is_dir);
        assert!(entries[1].is_dir);
        assert_eq!(entries[1].path, "/var/www/selinux");
        assert!(!entries[2].is_dir);
        assert_eq!(entries[2].size, 1234);
        assert_eq!(entries[2].name, "acl.txt");
        assert_eq!(entries[3].ext.as_deref(), Some("txt"));
        assert!(entries.iter().all(|entry| !entry.name.contains("rw-")));
    }

    #[test]
    fn marker_lines_never_leak_as_raw_fallback_names() {
        // 所有行都带标记、且无法结构化解析时，不应把原始整行当成目录名。
        let raw = "drwxr-xr-x.   2 root root 4096 Oct 12 09:30 a\n\
                   drwxr-xr-x+   2 root root 4096 Oct 12 09:30 b\n";
        let entries = parse_listing(raw, "/");
        assert_eq!(names(&entries), vec!["a", "b"]);
        assert!(entries.iter().all(|entry| !entry.name.contains("drwx")));
    }

    #[test]
    fn falls_back_to_names_as_directories_when_nothing_matches() {
        // 非 Unix / 非 DOS 方言：整份输出无法识别时才启用兜底（保住左侧树可展开）。
        let raw = "alpha\nbeta\ngamma\n";
        let entries = parse_listing(raw, "/pub");
        assert_eq!(names(&entries), vec!["alpha", "beta", "gamma"]);
        assert!(entries.iter().all(|entry| entry.is_dir));
    }

    #[test]
    fn unknown_lines_are_skipped_when_other_lines_parse() {
        let raw = "drwxr-xr-x    2 root root 4096 Oct 12 09:30 keep\n\
                   ??? not a listing line ???\n";
        let entries = parse_listing(raw, "/");
        assert_eq!(names(&entries), vec!["keep"]);
    }

    #[test]
    fn empty_output_yields_no_entries() {
        assert!(parse_listing("", "/").is_empty());
        assert!(parse_listing("\r\n", "/").is_empty());
    }

    #[test]
    fn joins_remote_paths() {
        assert_eq!(join_remote("/", "a"), "/a");
        assert_eq!(join_remote("", "a"), "/a");
        assert_eq!(join_remote("/pub/", "a"), "/pub/a");
        assert_eq!(join_remote("/pub/sub", "/a"), "/pub/sub/a");
    }

    #[test]
    fn rejects_invalid_requests_before_running_curl() {
        let connection = RemoteConnection {
            protocol: "http".into(),
            host: "example.com".into(),
            port: 80,
            username: String::new(),
            password: String::new(),
            auth_method: RemoteAuth::Password,
            private_key: String::new(),
            key_passphrase: String::new(),
            account_id: String::new(),
        };
        assert!(build_remote_url(&connection, "/", true).is_err());

        let connection = RemoteConnection {
            protocol: "sftp".into(),
            host: "example.com".into(),
            port: 22,
            username: String::new(),
            password: String::new(),
            auth_method: RemoteAuth::Password,
            private_key: String::new(),
            key_passphrase: String::new(),
            account_id: String::new(),
        };
        assert!(build_remote_url(&connection, "/../etc", true).is_err());
    }

    #[test]
    fn builds_encoded_file_and_directory_urls() {
        let connection = RemoteConnection {
            protocol: "sftp".into(),
            host: "example.com".into(),
            port: 22,
            username: "user".into(),
            password: "secret".into(),
            auth_method: RemoteAuth::Password,
            private_key: String::new(),
            key_passphrase: String::new(),
            account_id: String::new(),
        };

        let dir = build_remote_url(&connection, "/var/www", true).unwrap();
        assert_eq!(dir.as_str(), "sftp://example.com:22/var/www/");

        let file = build_remote_url(&connection, "/var/www/我的 笔记.txt", false).unwrap();
        assert_eq!(
            file.as_str(),
            "sftp://example.com:22/var/www/%E6%88%91%E7%9A%84%20%E7%AC%94%E8%AE%B0.txt"
        );
    }

    #[test]
    fn normalizes_remote_directories_and_file_names() {
        assert_eq!(normalize_remote_dir(""), "/");
        assert_eq!(normalize_remote_dir("/"), "/");
        assert_eq!(normalize_remote_dir("pub/"), "/pub");
        assert_eq!(normalize_remote_dir("/pub/sub/"), "/pub/sub");
        assert_eq!(remote_file_name("/a/b/c.txt").unwrap(), "c.txt");
        assert_eq!(remote_file_name("/a/b/").unwrap(), "b");
        assert!(remote_file_name("/").is_err());
        assert!(remote_file_name("/a/../").is_err());
    }

    #[test]
    fn converts_remote_names_to_safe_local_names() {
        assert_eq!(safe_local_name("report.txt").unwrap(), "report.txt");
        assert_eq!(safe_local_name("a:b?.txt").unwrap(), "a_b_.txt");
        assert_eq!(safe_local_name("笔记 1.txt").unwrap(), "笔记 1.txt");
        assert_eq!(safe_local_name("CON.txt").unwrap(), "_CON.txt");
        assert_eq!(safe_local_name("name.").unwrap(), "name");
        assert!(safe_local_name("..").is_err());
        assert!(safe_local_name("a/b").is_err());
        assert!(safe_local_name("a\\b").is_err());
    }

    #[test]
    fn joins_local_relative_paths_into_remote_paths() {
        assert_eq!(join_relative_remote("/upload", Path::new("")), "/upload");
        assert_eq!(
            join_relative_remote("/upload", Path::new("sub/file.txt")),
            "/upload/sub/file.txt"
        );
        assert_eq!(join_relative_remote("/", Path::new("a/b")), "/a/b");
    }

    #[test]
    fn rejects_empty_transfer_before_running_curl() {
        let request = RemoteTransferRequest {
            connection: RemoteConnection {
                protocol: "sftp".into(),
                host: "example.com".into(),
                port: 22,
                username: String::new(),
                password: String::new(),
                auth_method: RemoteAuth::Password,
                private_key: String::new(),
                key_passphrase: String::new(),
                account_id: String::new(),
            },
            direction: TransferDirection::Download,
            target: "C:\\Temp".into(),
            items: Vec::new(),
            remove_sources: false,
        };
        let result = transfer_remote(request, &mut |_| {});
        assert!(!result.ok);
        assert_eq!(result.errors[0], "没有要传输的条目");
    }

    #[test]
    fn builds_curl_auth_config_for_ftp_password() {
        let connection = RemoteConnection {
            protocol: "ftp".into(),
            host: "example.com".into(),
            port: 21,
            username: "user".into(),
            password: "secret".into(),
            auth_method: RemoteAuth::Password,
            private_key: String::new(),
            key_passphrase: String::new(),
            account_id: String::new(),
        };
        // 凭据只进 stdin 配置，且带引号以支持特殊字符。
        assert_eq!(
            curl_auth_config(&connection),
            "user = \"user:secret\"
"
        );
    }

    #[test]
    fn deserializes_remote_requests_from_frontend_shape() {
        let listing: RemoteRequest = serde_json::from_str(
            r#"{"protocol":"sftp","host":"example.com","port":22,"username":"u","password":"p","path":"/var/www"}"#,
        )
        .unwrap();
        assert_eq!(listing.connection.protocol, "sftp");
        assert_eq!(listing.connection.host, "example.com");
        assert_eq!(listing.path, "/var/www");

        let transfer: RemoteTransferRequest = serde_json::from_str(
            r#"{"protocol":"sftp","host":"example.com","port":22,"username":"u","password":"p","direction":"download","target":"C:\\Temp","items":[{"path":"/var/www","isDir":true}],"removeSources":true}"#,
        )
        .unwrap();
        assert_eq!(transfer.target, "C:\\Temp");
        assert_eq!(transfer.direction, TransferDirection::Download);
        assert_eq!(transfer.items.len(), 1);
        assert!(transfer.items[0].is_dir);
        assert!(transfer.remove_sources);
    }

    #[test]
    fn skips_dot_entries_returned_by_some_servers() {
        let raw = "drwxr-xr-x    2 root root 4096 Oct 12 09:30 .\n\
                   drwxr-xr-x    2 root root 4096 Oct 12 09:30 ..\n\
                   -rw-r--r--    1 root root    5 Oct 12 09:30 a.txt\n";
        let entries = parse_listing(raw, "/pub");
        assert_eq!(names(&entries), vec!["a.txt"]);
    }

    #[test]
    fn normalizes_time_and_extension_helpers() {
        assert_eq!(parse_hhmm("09:30"), Some((9, 30)));
        assert_eq!(parse_hhmm("25:00"), None);
        assert_eq!(parse_dos_time("12:05AM"), Some((0, 5)));
        assert_eq!(parse_dos_time("12:05PM"), Some((12, 5)));
        assert_eq!(parse_dos_time("23:59"), Some((23, 59)));
        assert_eq!(parse_dos_date("04-27-17"), Some((2017, 4, 27)));
        assert_eq!(parse_dos_date("04-27-1998"), Some((1998, 4, 27)));
        assert_eq!(remote_ext("a.tar.gz"), Some("gz".into()));
        assert_eq!(remote_ext(".bashrc"), None);
        assert_eq!(remote_ext("README"), None);
    }
}

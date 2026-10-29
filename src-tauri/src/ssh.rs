//! SSH 终端会话：基于纯 Rust 的 `russh` 客户端。
//!
//! 与 FTP/SFTP 文件窗格相互独立：文件传输走 curl（无需常驻连接），终端则是
//! 一条持久的交互式 shell 会话。支持两种认证方式：
//! - 密码；
//! - 私钥 + 可选密码短语。
//!
//! 主机密钥采用 OpenSSH `StrictHostKeyChecking=accept-new` 语义：`~/.ssh/known_hosts`
//! 中已记录的必须匹配；首次连接自动记录；记录的密钥变化则拒绝连接（防中间人）。
//! 终端输出经事件 `ssh:data` 以 base64 传给前端（xterm.js 直接写字节，避免
//! UTF-8 多字节字符被事件边界截断）。

use base64::Engine;
use parking_lot::Mutex;
use russh::client::{self, KeyboardInteractiveAuthResponse};
use russh::keys::known_hosts;
use russh::keys::{load_secret_key, HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh::{ChannelMsg, Disconnect};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;

use crate::app_core::events::{SSH_DATA, SSH_EXIT};
use crate::network::{RemoteAuth, RemoteConnection};

const TERM: &str = "xterm-256color";
const MAX_INPUT_BYTES: usize = 1024 * 1024;

/// 一次 SSH 连接请求（凭据由前端从系统凭据库解密后传入，后端不落盘）。
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshConnectRequest {
    /// 前端生成的会话 id（用于事件路由）。
    pub session_id: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    #[serde(default)]
    pub auth_method: RemoteAuth,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub private_key: String,
    #[serde(default)]
    pub key_passphrase: String,
    #[serde(default = "default_cols")]
    pub cols: u32,
    #[serde(default = "default_rows")]
    pub rows: u32,
}

fn default_cols() -> u32 {
    120
}

fn default_rows() -> u32 {
    32
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SshSessionInfo {
    pub session_id: String,
    pub fingerprint: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SshDataEvent {
    session_id: String,
    data: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SshExitEvent {
    session_id: String,
    message: String,
    code: Option<u32>,
}

enum SshControl {
    Write(Vec<u8>),
    Resize { cols: u32, rows: u32 },
    Close,
}

struct SshSession {
    tx: mpsc::UnboundedSender<SshControl>,
}

/// 所有活动 SSH 终端会话。仅保存控制通道，实际连接由各自任务持有。
pub struct SshManager {
    sessions: Mutex<HashMap<String, SshSession>>,
}

/// 认证参数（SSH 终端与 SFTP 共用）。
pub(crate) struct SshAuth {
    pub username: String,
    pub method: RemoteAuth,
    pub password: String,
    pub private_key: String,
    pub key_passphrase: String,
}

impl SshAuth {
    pub(crate) fn from_connection(connection: &RemoteConnection) -> Self {
        Self {
            username: connection.username.clone(),
            method: connection.auth_method,
            password: connection.password.clone(),
            private_key: connection.private_key.clone(),
            key_passphrase: connection.key_passphrase.clone(),
        }
    }
}

impl SshConnectRequest {
    fn auth(&self) -> SshAuth {
        SshAuth {
            username: self.username.clone(),
            method: self.auth_method,
            password: self.password.clone(),
            private_key: self.private_key.clone(),
            key_passphrase: self.key_passphrase.clone(),
        }
    }
}

impl SshManager {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
        }
    }

    /// 建立连接、认证并请求交互式 shell。成功后返回会话 id 与主机密钥指纹。
    pub async fn connect(
        self: Arc<Self>,
        app: AppHandle,
        request: SshConnectRequest,
    ) -> Result<SshSessionInfo, String> {
        validate(&request)?;

        let outcome = Arc::new(Mutex::new(None::<String>));
        let fingerprint = Arc::new(Mutex::new(String::new()));
        let session = connect_authenticated(
            &request.host,
            request.port,
            &request.auth(),
            Arc::clone(&outcome),
            Arc::clone(&fingerprint),
        )
        .await?;

        let channel = session
            .channel_open_session()
            .await
            .map_err(|error| format!("无法打开 SSH 会话：{error}"))?;
        channel
            .request_pty(
                false,
                TERM,
                request.cols.clamp(20, 500),
                request.rows.clamp(5, 300),
                0,
                0,
                &[],
            )
            .await
            .map_err(|error| format!("服务器拒绝分配终端：{error}"))?;
        channel
            .request_shell(true)
            .await
            .map_err(|error| format!("服务器拒绝启动 shell：{error}"))?;

        let (mut read, write) = channel.split();
        let (tx, mut rx) = mpsc::unbounded_channel::<SshControl>();
        let session_id = request.session_id.clone();
        let manager = Arc::clone(&self);
        let task_app = app.clone();
        let task_id = session_id.clone();

        // 先登记再启动任务：即使远程 shell 立刻退出，任务清理时也一定能找到自己，
        // 不会留下「已结束但仍在表里」的会话。
        self.sessions
            .lock()
            .insert(session_id.clone(), SshSession { tx });

        tauri::async_runtime::spawn(async move {
            let mut exit_code = None;
            let mut message = "连接已断开".to_string();

            loop {
                tokio::select! {
                    message_opt = read.wait() => {
                        match message_opt {
                            Some(ChannelMsg::Data { data }) => {
                                emit_data(&task_app, &task_id, &data);
                            }
                            Some(ChannelMsg::ExtendedData { data, ext }) => {
                                // ext = 1 是 stderr；终端里同样要显示。
                                if ext == 1 {
                                    emit_data(&task_app, &task_id, &data);
                                }
                            }
                            Some(ChannelMsg::ExitStatus { exit_status }) => {
                                exit_code = Some(exit_status);
                                message = format!("远程 shell 已退出（{exit_status}）");
                            }
                            Some(ChannelMsg::ExitSignal { signal_name, error_message, .. }) => {
                                message = if error_message.is_empty() {
                                    format!("远程 shell 被信号终止：{signal_name:?}")
                                } else {
                                    format!("远程 shell 被信号终止：{error_message}")
                                };
                            }
                            Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => break,
                            _ => {}
                        }
                    }
                    control = rx.recv() => {
                        match control {
                            Some(SshControl::Write(bytes)) => {
                                if write.data_bytes(bytes).await.is_err() {
                                    message = "写入远程终端失败，连接已断开".into();
                                    break;
                                }
                            }
                            Some(SshControl::Resize { cols, rows }) => {
                                let _ = write.window_change(cols, rows, 0, 0).await;
                            }
                            Some(SshControl::Close) | None => {
                                let _ = write.close().await;
                                message = "连接已关闭".into();
                                break;
                            }
                        }
                    }
                }
            }

            let _ = session
                .disconnect(Disconnect::ByApplication, "", "zh-CN")
                .await;
            manager.sessions.lock().remove(&task_id);
            let _ = task_app.emit(
                SSH_EXIT,
                SshExitEvent {
                    session_id: task_id,
                    message,
                    code: exit_code,
                },
            );
        });

        let fingerprint = fingerprint.lock().clone();
        Ok(SshSessionInfo {
            session_id,
            fingerprint,
        })
    }

    /// 向终端写入用户输入（UTF-8 字符串，包括控制字符）。
    pub fn write(&self, session_id: &str, data: String) -> Result<(), String> {
        if data.len() > MAX_INPUT_BYTES {
            return Err("单次输入过长".into());
        }
        let session = self
            .sessions
            .lock()
            .get(session_id)
            .map(|session| session.tx.clone())
            .ok_or("SSH 会话不存在或已结束")?;
        session
            .send(SshControl::Write(data.into_bytes()))
            .map_err(|_| "SSH 会话已结束".to_string())
    }

    /// 终端尺寸变化（xterm 的 cols / rows）。
    pub fn resize(&self, session_id: &str, cols: u32, rows: u32) {
        let session = self
            .sessions
            .lock()
            .get(session_id)
            .map(|session| session.tx.clone());
        if let Some(tx) = session {
            let _ = tx.send(SshControl::Resize {
                cols: cols.clamp(20, 500),
                rows: rows.clamp(5, 300),
            });
        }
    }

    /// 主动关闭会话（前端关闭终端标签 / 断开按钮）。
    pub fn close(&self, session_id: &str) {
        if let Some(session) = self.sessions.lock().remove(session_id) {
            let _ = session.tx.send(SshControl::Close);
        }
    }
}

impl Default for SshManager {
    fn default() -> Self {
        Self::new()
    }
}

/// 建立一条已认证的 SSH 连接（终端与 SFTP 共用）。
///
/// 主机密钥按 OpenSSH `accept-new` 语义校验：已记录必须一致，未知则记录后放行。
pub(crate) async fn connect_authenticated(
    host: &str,
    port: u16,
    auth: &SshAuth,
    outcome: Arc<Mutex<Option<String>>>,
    fingerprint: Arc<Mutex<String>>,
) -> Result<client::Handle<ClientHandler>, String> {
    let handler = ClientHandler {
        host: host.to_string(),
        port,
        known_hosts: known_hosts_path()?,
        outcome: Arc::clone(&outcome),
        fingerprint,
    };

    let config = Arc::new(client::Config {
        // 交互式终端 / 长传输可能长时间无数据；超时交给 TCP 与调用方处理。
        inactivity_timeout: None,
        ..Default::default()
    });

    let mut session = match tokio::time::timeout(
        CONNECT_TIMEOUT,
        client::connect(config, (host, port), handler),
    )
    .await
    {
        Err(_) => {
            return Err(format!(
                "连接 {host}:{port} 超时（{} 秒）",
                CONNECT_TIMEOUT.as_secs()
            ))
        }
        Ok(Ok(session)) => session,
        Ok(Err(error)) => {
            return Err(outcome
                .lock()
                .take()
                .unwrap_or_else(|| format!("无法连接 {host}:{port}：{error}")))
        }
    };

    match tokio::time::timeout(CONNECT_TIMEOUT, authenticate(&mut session, auth)).await {
        Ok(result) => result?,
        Err(_) => return Err(format!("SSH 认证超时（{} 秒）", CONNECT_TIMEOUT.as_secs())),
    }
    Ok(session)
}

fn validate(request: &SshConnectRequest) -> Result<(), String> {
    if request.session_id.trim().is_empty() || request.session_id.len() > 128 {
        return Err("会话标识无效".into());
    }
    if request.host.is_empty()
        || request
            .host
            .contains(['/', '\\', '@', ':', '\n', '\r', '\0'])
    {
        return Err("主机地址无效".into());
    }
    if request.port == 0 {
        return Err("端口无效".into());
    }
    if request.username.trim().is_empty() {
        return Err("SSH 用户名不能为空".into());
    }
    Ok(())
}

async fn authenticate(
    session: &mut client::Handle<ClientHandler>,
    auth: &SshAuth,
) -> Result<(), String> {
    match auth.method {
        RemoteAuth::Password => {
            let result = session
                .authenticate_password(auth.username.as_str(), auth.password.as_str())
                .await
                .map_err(|error| format!("密码认证失败：{error}"))?;
            if result.success() {
                return Ok(());
            }
            // 部分服务器只启用 keyboard-interactive（如带 MFA 的配置），用同一密码再试一次。
            if authenticate_keyboard_interactive(session, auth).await? {
                return Ok(());
            }
            Err("密码认证失败：用户名或密码不正确".into())
        }
        RemoteAuth::Key => {
            let key_path = auth.private_key.trim();
            if key_path.is_empty() {
                return Err("请选择 SSH 私钥文件".into());
            }
            let passphrase = if auth.key_passphrase.is_empty() {
                None
            } else {
                Some(auth.key_passphrase.as_str())
            };
            let key = load_secret_key(Path::new(key_path), passphrase)
                .map_err(|error| format!("无法读取私钥 {key_path}：{error}"))?;
            let hash = session
                .best_supported_rsa_hash()
                .await
                .ok()
                .flatten()
                .flatten();
            let result = session
                .authenticate_publickey(
                    auth.username.as_str(),
                    PrivateKeyWithHashAlg::new(Arc::new(key), hash),
                )
                .await
                .map_err(|error| format!("私钥认证失败：{error}"))?;
            if result.success() {
                Ok(())
            } else {
                Err("私钥认证失败：服务器拒绝了该密钥".into())
            }
        }
    }
}

/// keyboard-interactive 兜底：把密码填入服务器给出的所有提示。
async fn authenticate_keyboard_interactive(
    session: &mut client::Handle<ClientHandler>,
    auth: &SshAuth,
) -> Result<bool, String> {
    let mut response = session
        .authenticate_keyboard_interactive_start(auth.username.as_str(), None::<String>)
        .await
        .map_err(|error| format!("keyboard-interactive 认证失败：{error}"))?;
    // 服务器可能连续多次询问；最多 3 轮，避免恶意服务器无限循环。
    for _ in 0..3 {
        match response {
            KeyboardInteractiveAuthResponse::Success => return Ok(true),
            KeyboardInteractiveAuthResponse::Failure { .. } => return Ok(false),
            KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
                if prompts.is_empty() {
                    return Ok(false);
                }
                let answers = prompts
                    .iter()
                    .map(|_| auth.password.clone())
                    .collect::<Vec<_>>();
                response = session
                    .authenticate_keyboard_interactive_respond(answers)
                    .await
                    .map_err(|error| format!("keyboard-interactive 认证失败：{error}"))?;
            }
        }
    }
    Ok(false)
}

pub(crate) struct ClientHandler {
    host: String,
    port: u16,
    known_hosts: PathBuf,
    /// 密钥校验失败原因（connect 返回错误时优先展示它）。
    outcome: Arc<Mutex<Option<String>>>,
    fingerprint: Arc<Mutex<String>>,
}

impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let key = server_public_key.public_key();
        let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
        *self.fingerprint.lock() = fingerprint.clone();

        match known_hosts::check_known_hosts_path(&self.host, self.port, &key, &self.known_hosts) {
            Ok(true) => Ok(true),
            // 首次连接：记录后再放行（accept-new）。
            Ok(false) => match known_hosts::learn_known_hosts_path(
                &self.host,
                self.port,
                &key,
                &self.known_hosts,
            ) {
                Ok(()) => Ok(true),
                Err(error) => {
                    *self.outcome.lock() = Some(format!(
                        "无法写入 ~/.ssh/known_hosts：{error}（主机指纹 {fingerprint}）"
                    ));
                    Ok(false)
                }
            },
            // 已有记录但不匹配：可能是中间人，也可能是服务器更换了主机密钥。
            Err(error) => {
                *self.outcome.lock() = Some(format!(
                    "服务器主机密钥校验失败：{error}（指纹 {fingerprint}）。\
                     如确认服务器更换了密钥，请从 ~/.ssh/known_hosts 删除对应记录后重试"
                ));
                Ok(false)
            }
        }
    }
}

fn known_hosts_path() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or("无法定位用户主目录")?;
    let ssh_dir = home.join(".ssh");
    std::fs::create_dir_all(&ssh_dir)
        .map_err(|error| format!("无法创建 {}：{error}", ssh_dir.display()))?;
    Ok(ssh_dir.join("known_hosts"))
}

fn emit_data(app: &AppHandle, session_id: &str, data: &[u8]) {
    if data.is_empty() {
        return;
    }
    let _ = app.emit(
        SSH_DATA,
        SshDataEvent {
            session_id: session_id.to_string(),
            data: base64::engine::general_purpose::STANDARD.encode(data),
        },
    );
}

/// 连接（DNS / TCP / 密钥交换 / 认证）超时。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> SshConnectRequest {
        SshConnectRequest {
            session_id: "session-1".into(),
            host: "example.com".into(),
            port: 22,
            username: "user".into(),
            auth_method: RemoteAuth::Password,
            password: "secret".into(),
            private_key: String::new(),
            key_passphrase: String::new(),
            cols: 120,
            rows: 32,
        }
    }

    #[test]
    fn validates_connect_requests() {
        assert!(validate(&request()).is_ok());

        let mut bad = request();
        bad.session_id.clear();
        assert!(validate(&bad).is_err());

        let mut bad = request();
        bad.host = "user@host".into();
        assert!(validate(&bad).is_err());

        let mut bad = request();
        bad.port = 0;
        assert!(validate(&bad).is_err());

        let mut bad = request();
        bad.username = "  ".into();
        assert!(validate(&bad).is_err());
    }

    #[test]
    fn write_and_resize_reject_unknown_sessions() {
        let manager = SshManager::new();
        assert!(manager.write("missing", "ls\n".into()).is_err());
        manager.resize("missing", 80, 24);
        manager.close("missing");
    }

    #[test]
    fn missing_known_hosts_file_is_treated_as_unknown_host() {
        // 首次连接时 ~/.ssh/known_hosts 可能还不存在，必须是「未知」而不是错误，
        // 否则 accept-new 会退化成永远连不上新主机。
        let path =
            std::env::temp_dir().join(format!("stelix-known-hosts-{}.tmp", uuid::Uuid::new_v4()));
        let key = russh::keys::PublicKey::from_openssh(
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIJdD7y3aLq454yWBdwLWbieU1ebz9/cu7/QEXn9OIeZJ",
        )
        .unwrap();
        let result = known_hosts::check_known_hosts_path("example.com", 22, &key, &path);
        assert!(matches!(result, Ok(false)), "unexpected result: {result:?}");
    }
}

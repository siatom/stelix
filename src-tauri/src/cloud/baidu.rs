//! 百度网盘（Open API + OAuth2）。
//!
//! 授权走本机回环地址：浏览器完成授权后百度跳回
//! `http://127.0.0.1:17890/oauth/callback`，本模块在该端口等待一次回调并换取令牌。
//! 上传按 4MB 分片计算 MD5（precreate → superfile2 → create），下载使用
//! `filemetas` 返回的 dlink（UA 必须为 `pan.baidu.com`）。

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::AsyncReadExt;

use super::{
    http_client, now_ms, parent_path, save_stream, save_token, validate_account_id, value_i64,
    value_str, BaiduLoginRequest, CloudAccountInfo, CloudEntry, CloudToken,
};
use crate::network::{join_remote, normalize_remote_dir, remote_file_name, RemoteConnection};

const AUTHORIZE_URL: &str = "https://openapi.baidu.com/oauth/2.0/authorize";
const TOKEN_URL: &str = "https://openapi.baidu.com/oauth/2.0/token";
const API_BASE: &str = "https://pan.baidu.com/rest/2.0";
const UPLOAD_BASE: &str = "https://d.pcs.baidu.com";
const CALLBACK_ADDR: &str = "127.0.0.1:17890";
const REDIRECT_URI: &str = "http://127.0.0.1:17890/oauth/callback";
const USER_AGENT: &str = "pan.baidu.com";
const BLOCK_SIZE: u64 = 4 * 1024 * 1024;
const PAGE_LIMIT: usize = 1000;
const AUTH_TIMEOUT_SECS: u64 = 180;

#[derive(Deserialize, Default)]
struct OAuthToken {
    #[serde(default)]
    access_token: String,
    #[serde(default)]
    refresh_token: String,
    #[serde(default)]
    expires_in: i64,
    #[serde(default)]
    error: String,
    #[serde(default)]
    error_description: String,
}

fn oauth_error(response: &OAuthToken, text: &str) -> Option<String> {
    if !response.error.is_empty() || response.access_token.is_empty() {
        let detail = if response.error_description.is_empty() {
            if response.error.is_empty() {
                text.to_string()
            } else {
                response.error.clone()
            }
        } else {
            format!("{}：{}", response.error, response.error_description)
        };
        return Some(format!("百度网盘授权失败：{detail}"));
    }
    None
}

/// 打开浏览器完成 OAuth 授权，并在本机回调端口取出 code。
pub async fn login(
    app: &tauri::AppHandle,
    request: BaiduLoginRequest,
) -> Result<CloudAccountInfo, String> {
    let account_id = validate_account_id(&request.account_id)?;
    let client_id = request.client_id.trim().to_string();
    let client_secret = request.client_secret.trim().to_string();
    if client_id.is_empty() || client_secret.is_empty() {
        return Err("请填写百度网盘应用的 AppKey 与 SecretKey".into());
    }

    let state = uuid::Uuid::new_v4().to_string();
    let listener = tokio::net::TcpListener::bind(CALLBACK_ADDR)
        .await
        .map_err(|error| format!("无法监听本机端口 {CALLBACK_ADDR}：{error}"))?;

    let mut authorize = reqwest::Url::parse(AUTHORIZE_URL).map_err(|_| "授权地址无效")?;
    authorize
        .query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &client_id)
        .append_pair("redirect_uri", REDIRECT_URI)
        .append_pair("scope", "basic,netdisk")
        .append_pair("state", &state);

    {
        use tauri_plugin_opener::OpenerExt;
        app.opener()
            .open_url(authorize.as_str(), None::<&str>)
            .map_err(|error| format!("无法打开浏览器：{error}"))?;
    }

    let code = wait_callback(listener, &state).await?;
    let token = exchange_code(&client_id, &client_secret, &code).await?;
    let (username, nickname) = fetch_profile(&token.access_token).await?;

    let label = if request.label.trim().is_empty() {
        nickname
    } else {
        request.label.trim().to_string()
    };
    let cloud_token = CloudToken {
        label: label.clone(),
        username: username.clone(),
        access_token: token.access_token,
        refresh_token: token.refresh_token,
        expires_at: now_ms() + token.expires_in.max(0) * 1000,
        client_id,
        client_secret,
        ..Default::default()
    };
    save_token(&account_id, &cloud_token)?;
    Ok(CloudAccountInfo {
        account_id,
        provider: super::BAIDU_PROTOCOL.into(),
        label,
        username,
    })
}

async fn wait_callback(listener: tokio::net::TcpListener, state: &str) -> Result<String, String> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let accept = tokio::time::timeout(
        Duration::from_secs(AUTH_TIMEOUT_SECS),
        listener.accept(),
    )
    .await
    .map_err(|_| "等待授权超时（3 分钟），请重新登录".to_string())?;
    let (socket, _) = accept.map_err(|error| format!("本机回调失败：{error}"))?;

    let mut reader = BufReader::new(socket);
    let mut request_line = String::new();
    tokio::time::timeout(Duration::from_secs(10), reader.read_line(&mut request_line))
        .await
        .map_err(|_| "读取授权结果超时".to_string())?
        .map_err(|error| format!("读取授权结果失败：{error}"))?;

    let target = request_line.split_whitespace().nth(1).unwrap_or("");
    let query = target.split_once('?').map(|(_, query)| query).unwrap_or("");
    let pairs: HashMap<String, String> = query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(key, value)| (percent_decode(key), percent_decode(value)))
        .collect();

    let success = pairs.contains_key("code");
    let body = if success {
        "<!doctype html><meta charset=\"utf-8\"><title>Stelix 授权成功</title>\
         <body style=\"font-family:system-ui;text-align:center;padding-top:18vh;color:#222\">\
         <h2>授权成功</h2><p>可以关闭此页面，返回 Stelix 继续使用。</p></body>"
    } else {
        "<!doctype html><meta charset=\"utf-8\"><title>Stelix 授权失败</title>\
         <body style=\"font-family:system-ui;text-align:center;padding-top:18vh;color:#222\">\
         <h2>授权未完成</h2><p>请返回 Stelix 重新登录。</p></body>"
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.as_bytes().len(),
        body
    );
    let mut socket = reader.into_inner();
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.shutdown().await;

    if let Some(error) = pairs.get("error") {
        let description = pairs
            .get("error_description")
            .cloned()
            .unwrap_or_default();
        return Err(format!("百度网盘授权被拒绝：{error} {description}"));
    }
    if pairs.get("state").map(String::as_str) != Some(state) {
        return Err("授权状态校验失败，请重新登录".into());
    }
    pairs
        .get("code")
        .cloned()
        .ok_or_else(|| "授权回调缺少 code".to_string())
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let high = (bytes[index + 1] as char).to_digit(16);
                let low = (bytes[index + 2] as char).to_digit(16);
                match (high, low) {
                    (Some(high), Some(low)) => {
                        out.push((high * 16 + low) as u8);
                        index += 3;
                    }
                    _ => {
                        out.push(bytes[index]);
                        index += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

async fn exchange_code(client_id: &str, client_secret: &str, code: &str) -> Result<OAuthToken, String> {
    let response = http_client()
        .get(TOKEN_URL)
        .query(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("redirect_uri", REDIRECT_URI),
        ])
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|error| format!("换取百度网盘令牌失败：{error}"))?;
    let text = response
        .text()
        .await
        .map_err(|error| format!("读取百度网盘令牌失败：{error}"))?;
    let token: OAuthToken = serde_json::from_str(&text)
        .map_err(|_| format!("百度网盘令牌响应无效：{text}"))?;
    if let Some(error) = oauth_error(&token, &text) {
        return Err(error);
    }
    Ok(token)
}

/// 读取昵称，用作账号显示名。
async fn fetch_profile(access_token: &str) -> Result<(String, String), String> {
    let response = http_client()
        .get(format!("{API_BASE}/xpan/nas"))
        .query(&[("method", "uinfo"), ("access_token", access_token)])
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|error| format!("读取百度网盘账号信息失败：{error}"))?;
    let text = response
        .text()
        .await
        .map_err(|error| format!("读取百度网盘账号信息失败：{error}"))?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|_| format!("百度网盘账号信息无效：{text}"))?;
    let errno = value_i64(&value, "errno").unwrap_or(0);
    if errno != 0 {
        return Err(format!(
            "读取百度网盘账号信息失败（{errno}）：{}",
            value_str(&value, "errmsg")
        ));
    }
    let baidu_name = value_str(&value, "baidu_name").to_string();
    let netdisk_name = value_str(&value, "netdisk_name").to_string();
    let label = if !netdisk_name.is_empty() {
        netdisk_name
    } else if !baidu_name.is_empty() {
        baidu_name.clone()
    } else {
        "百度网盘".into()
    };
    Ok((baidu_name, label))
}

/// 一次百度网盘会话：401 类错误自动刷新令牌，后续文件操作复用同一个 access_token。
pub struct BaiduSession {
    http: reqwest::Client,
    account_id: String,
    token: CloudToken,
}

impl BaiduSession {
    pub async fn open(connection: &RemoteConnection) -> Result<Self, String> {
        let account_id = super::account_id(connection)?;
        let token = super::load_token(&account_id)?;
        if token.access_token.is_empty() {
            return Err("百度网盘登录已失效，请重新登录".into());
        }
        let mut session = Self {
            http: http_client(),
            account_id,
            token,
        };
        session.refresh_if_needed().await?;
        Ok(session)
    }

    async fn refresh_if_needed(&mut self) -> Result<(), String> {
        if self.token.expires_at == 0 || self.token.expires_at > now_ms() + 60_000 {
            return Ok(());
        }
        self.refresh().await
    }

    async fn refresh(&mut self) -> Result<(), String> {
        if self.token.refresh_token.is_empty()
            || self.token.client_id.is_empty()
            || self.token.client_secret.is_empty()
        {
            return Err("百度网盘登录已过期，请重新登录".into());
        }
        let response = self
            .http
            .get(TOKEN_URL)
            .query(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", self.token.refresh_token.as_str()),
                ("client_id", self.token.client_id.as_str()),
                ("client_secret", self.token.client_secret.as_str()),
            ])
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|error| format!("刷新百度网盘令牌失败：{error}"))?;
        let text = response
            .text()
            .await
            .map_err(|error| format!("读取百度网盘令牌失败：{error}"))?;
        let token: OAuthToken =
            serde_json::from_str(&text).map_err(|_| format!("百度网盘令牌响应无效：{text}"))?;
        if let Some(error) = oauth_error(&token, &text) {
            return Err(error);
        }
        self.token.access_token = token.access_token;
        if !token.refresh_token.is_empty() {
            self.token.refresh_token = token.refresh_token;
        }
        self.token.expires_at = now_ms() + token.expires_in.max(0) * 1000;
        save_token(&self.account_id, &self.token)?;
        Ok(())
    }

    /// 调用 xpan 接口；令牌过期（errno 111 / -6 / 20016 / 20017）时刷新并重试一次。
    async fn api(
        &mut self,
        method: reqwest::Method,
        path: &str,
        params: &[(&str, String)],
        form: Option<&[(String, String)]>,
    ) -> Result<Value, String> {
        for attempt in 0..2 {
            let mut request = self
                .http
                .request(method.clone(), format!("{API_BASE}{path}"))
                .query(&[("access_token", self.token.access_token.as_str())])
                .query(params)
                .timeout(Duration::from_secs(90));
            if let Some(form) = form {
                request = request.form(form);
            }
            let response = request
                .send()
                .await
                .map_err(|error| format!("百度网盘请求失败：{error}"))?;
            let status = response.status();
            let text = response
                .text()
                .await
                .map_err(|error| format!("读取百度网盘响应失败：{error}"))?;
            let value: Value = serde_json::from_str(&text)
                .unwrap_or_else(|_| json!({ "errno": -1, "errmsg": text.clone() }));
            let errno = value_i64(&value, "errno")
                .or_else(|| value_i64(&value, "error_code"))
                .unwrap_or(0);
            if errno == 0 && status.is_success() {
                return Ok(value);
            }
            if attempt == 0 && matches!(errno, 111 | -6 | 20016 | 20017) {
                self.refresh().await?;
                continue;
            }
            let message = value_str(&value, "errmsg");
            let detail = if message.is_empty() {
                text.trim().to_string()
            } else {
                message.to_string()
            };
            return Err(format!("百度网盘接口错误（{errno}）：{detail}"));
        }
        Err("百度网盘请求失败".into())
    }

    pub async fn list(&mut self, path: &str) -> Result<Vec<CloudEntry>, String> {
        let path = normalize_remote_dir(path);
        let mut entries = Vec::new();
        let mut start = 0usize;
        loop {
            let value = self
                .api(
                    reqwest::Method::GET,
                    "/xpan/file",
                    &[
                        ("method", "list".into()),
                        ("dir", path.clone()),
                        ("web", "web".into()),
                        ("start", start.to_string()),
                        ("limit", PAGE_LIMIT.to_string()),
                    ],
                    None,
                )
                .await?;
            let list = value
                .get("list")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if list.is_empty() {
                break;
            }
            for item in &list {
                let name = value_str(item, "server_filename");
                if name.is_empty() || name == "." || name == ".." {
                    continue;
                }
                let raw_path = value_str(item, "path");
                let entry_path = if raw_path.is_empty() {
                    join_remote(&path, name)
                } else {
                    raw_path.to_string()
                };
                let is_dir = value_i64(item, "isdir").unwrap_or(0) == 1;
                let modified = value_i64(item, "server_mtime")
                    .or_else(|| value_i64(item, "local_mtime"))
                    .unwrap_or(0);
                entries.push(CloudEntry {
                    name: name.to_string(),
                    path: normalize_remote_dir(&entry_path),
                    id: value_i64(item, "fs_id")
                        .map(|id| id.to_string())
                        .unwrap_or_default(),
                    is_dir,
                    size: value_i64(item, "size").unwrap_or(0).max(0) as u64,
                    modified: modified * 1000,
                });
            }
            if list.len() < PAGE_LIMIT {
                break;
            }
            start += PAGE_LIMIT;
        }
        Ok(entries)
    }

    pub async fn lookup(&mut self, path: &str) -> Result<CloudEntry, String> {
        let path = normalize_remote_dir(path);
        if path == "/" {
            return Err("不能下载云端根目录".into());
        }
        let parent = parent_path(&path);
        let name = remote_file_name(&path)?;
        self.list(&parent)
            .await?
            .into_iter()
            .find(|entry| entry.name == name)
            .ok_or_else(|| format!("云端文件不存在：{path}"))
    }

    pub async fn download(&mut self, entry: &CloudEntry, local: &Path) -> Result<u64, String> {
        if entry.id.is_empty() {
            return Err("百度网盘未返回文件 id".into());
        }
        let value = self
            .api(
                reqwest::Method::GET,
                "/xpan/multimedia",
                &[
                    ("method", "filemetas".into()),
                    ("fsids", format!("[{}]", entry.id)),
                    ("dlink", "1".into()),
                ],
                None,
            )
            .await?;
        let dlink = value
            .get("list")
            .and_then(|list| list.get(0))
            .map(|item| value_str(item, "dlink"))
            .unwrap_or("");
        if dlink.is_empty() {
            return Err("百度网盘未返回下载地址（可能没有下载权限）".into());
        }
        let separator = if dlink.contains('?') { '&' } else { '?' };
        let url = format!("{dlink}{separator}access_token={}", self.token.access_token);
        let response = self
            .http
            .get(url)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .send()
            .await
            .map_err(|error| format!("百度网盘下载失败：{error}"))?;
        if !response.status().is_success() {
            return Err(format!(
                "百度网盘下载失败：HTTP {}",
                response.status().as_u16()
            ));
        }
        save_stream(response, local).await
    }

    /// 上传单个文件（百度网盘不允许上传空文件）。
    pub async fn upload(&mut self, local: &Path, remote: &str) -> Result<u64, String> {
        let size = tokio::fs::metadata(local)
            .await
            .map_err(|error| format!("读取 {} 失败：{error}", local.display()))?
            .len();
        if size == 0 {
            return Err("百度网盘不允许上传空文件".into());
        }
        let file_name = local
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("file")
            .to_string();
        let path = normalize_remote_dir(remote);
        let local_buf = local.to_path_buf();
        let (blocks, content_md5, slice_md5) =
            tokio::task::spawn_blocking(move || hash_blocks(&local_buf))
                .await
                .map_err(|error| format!("计算文件校验值失败：{error}"))??;

        let block_json = serde_json::to_string(&blocks).map_err(|error| error.to_string())?;
        let now = chrono::Utc::now().timestamp().to_string();
        let precreate = self
            .api(
                reqwest::Method::POST,
                "/xpan/file",
                &[("method", "precreate".into())],
                Some(&[
                    ("path".into(), path.clone()),
                    ("size".into(), size.to_string()),
                    ("isdir".into(), "0".into()),
                    ("autoinit".into(), "1".into()),
                    ("rtype".into(), "3".into()),
                    ("block_list".into(), block_json.clone()),
                    ("content-md5".into(), content_md5),
                    ("slice-md5".into(), slice_md5),
                    ("local_mtime".into(), now.clone()),
                    ("local_ctime".into(), now),
                ]),
            )
            .await?;

        // return_type = 2：服务端已命中相同 MD5，秒传成功。
        if value_i64(&precreate, "return_type") == Some(2) {
            return Ok(size);
        }
        let upload_id = value_str(&precreate, "uploadid").to_string();
        if upload_id.is_empty() {
            return Err("百度网盘预上传未返回 uploadid".into());
        }
        let parts: Vec<i64> = precreate
            .get("block_list")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|item| {
                        item.as_i64()
                            .or_else(|| item.as_str().and_then(|text| text.parse().ok()))
                    })
                    .collect()
            })
            .unwrap_or_default();

        for partseq in parts {
            if partseq < 0 {
                continue;
            }
            let offset = partseq as u64 * BLOCK_SIZE;
            if offset >= size {
                continue;
            }
            let length = BLOCK_SIZE.min(size - offset);
            let data = read_block(local, offset, length as usize).await?;
            self.upload_slice(&path, &upload_id, partseq, &file_name, data)
                .await?;
        }

        self.api(
            reqwest::Method::POST,
            "/xpan/file",
            &[("method", "create".into())],
            Some(&[
                ("path".into(), path),
                ("size".into(), size.to_string()),
                ("isdir".into(), "0".into()),
                ("rtype".into(), "3".into()),
                ("uploadid".into(), upload_id),
                ("block_list".into(), block_json),
            ]),
        )
        .await?;
        Ok(size)
    }

    async fn upload_slice(
        &self,
        path: &str,
        upload_id: &str,
        partseq: i64,
        file_name: &str,
        data: Vec<u8>,
    ) -> Result<(), String> {
        let part = reqwest::multipart::Part::bytes(data)
            .file_name(file_name.to_string())
            .mime_str("application/octet-stream")
            .map_err(|error| format!("构造上传分片失败：{error}"))?;
        let form = reqwest::multipart::Form::new().part("file", part);
        let partseq_text = partseq.to_string();
        let response = self
            .http
            .post(format!("{UPLOAD_BASE}/rest/2.0/pcs/superfile2"))
            .query(&[
                ("method", "upload"),
                ("access_token", self.token.access_token.as_str()),
                ("type", "tmpfile"),
                ("path", path),
                ("uploadid", upload_id),
                ("partseq", partseq_text.as_str()),
            ])
            .multipart(form)
            .timeout(Duration::from_secs(300))
            .send()
            .await
            .map_err(|error| format!("分片上传失败：{error}"))?;
        let text = response
            .text()
            .await
            .map_err(|error| format!("分片上传响应无效：{error}"))?;
        let value: Value = serde_json::from_str(&text)
            .unwrap_or_else(|_| json!({ "errno": -1, "errmsg": text.clone() }));
        let errno = value_i64(&value, "errno")
            .or_else(|| value_i64(&value, "error_code"))
            .unwrap_or(0);
        if errno != 0 {
            return Err(format!(
                "分片上传失败（{errno}）：{}",
                value_str(&value, "errmsg")
            ));
        }
        Ok(())
    }

    /// 创建目录；目录已存在（errno -8）视为成功。
    pub async fn ensure_dir(&mut self, remote: &str) -> Result<(), String> {
        let path = normalize_remote_dir(remote);
        if path == "/" {
            return Ok(());
        }
        match self
            .api(
                reqwest::Method::POST,
                "/xpan/file",
                &[("method", "create".into())],
                Some(&[
                    ("path".into(), path.clone()),
                    ("size".into(), "0".into()),
                    ("isdir".into(), "1".into()),
                    ("rtype".into(), "3".into()),
                ]),
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(error) if error.contains("（-8）") => Ok(()),
            Err(error) => Err(error),
        }
    }

    pub async fn remove(&mut self, path: &str) -> Result<(), String> {
        let path = normalize_remote_dir(path);
        if path == "/" {
            return Err("不能删除云端根目录".into());
        }
        let filelist =
            serde_json::to_string(&[path.as_str()]).map_err(|error| error.to_string())?;
        self.api(
            reqwest::Method::POST,
            "/xpan/file",
            &[
                ("method", "filemanager".into()),
                ("opera", "delete".into()),
            ],
            Some(&[
                ("async".into(), "0".into()),
                ("filelist".into(), filelist),
            ]),
        )
        .await?;
        Ok(())
    }
}

async fn read_block(local: &Path, offset: u64, length: usize) -> Result<Vec<u8>, String> {
    use tokio::io::AsyncSeekExt;
    let mut file = tokio::fs::File::open(local)
        .await
        .map_err(|error| format!("读取 {} 失败：{error}", local.display()))?;
    file.seek(std::io::SeekFrom::Start(offset))
        .await
        .map_err(|error| format!("读取 {} 失败：{error}", local.display()))?;
    let mut data = vec![0u8; length];
    file.read_exact(&mut data)
        .await
        .map_err(|error| format!("读取 {} 失败：{error}", local.display()))?;
    Ok(data)
}

/// 顺序读取一次文件，计算每个 4MB 分片的 MD5、整文件 MD5 与前 256KB 的 MD5。
fn hash_blocks(path: &Path) -> Result<(Vec<String>, String, String), String> {
    let mut file =
        std::fs::File::open(path).map_err(|error| format!("读取 {} 失败：{error}", path.display()))?;
    let mut content = md5::Context::new();
    let mut slice = md5::Context::new();
    let mut slice_remaining = 256 * 1024usize;
    let mut blocks = Vec::new();
    let mut buffer = vec![0u8; BLOCK_SIZE as usize];

    loop {
        let mut filled = 0usize;
        while filled < buffer.len() {
            let read = file
                .read(&mut buffer[filled..])
                .map_err(|error| format!("读取 {} 失败：{error}", path.display()))?;
            if read == 0 {
                break;
            }
            filled += read;
        }
        if filled == 0 {
            break;
        }
        let block = &buffer[..filled];
        content.consume(block);
        if slice_remaining > 0 {
            let take = filled.min(slice_remaining);
            slice.consume(&block[..take]);
            slice_remaining -= take;
        }
        blocks.push(hex::encode(md5::compute(block).0));
    }
    if blocks.is_empty() {
        blocks.push(hex::encode(md5::compute(b"").0));
    }
    Ok((
        blocks,
        hex::encode(content.finalize().0),
        hex::encode(slice.finalize().0),
    ))
}

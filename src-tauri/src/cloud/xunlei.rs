//! 迅雷云盘（App API）。
//!
//! 登录分两步：`xluser.core.login/v3/login` 拿到 sessionID，再用验证码令牌换取
//! OAuth access / refresh token。云盘接口为 `api-pan.xunlei.com/drive/v1`，
//! 文件按 id 组织，目录路径在本地通过逐级列举解析。
//!
//! 上传需要 S3 分片签名（S3 SigV4），暂不在客户端实现，会返回明确提示。

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};

use super::{
    http_client, now_ms, parent_path, save_stream, save_token, validate_account_id, value_i64,
    value_str, CloudAccountInfo, CloudEntry, CloudToken, XunleiLoginRequest,
    XunleiTokenLoginRequest,
};
use crate::network::{join_remote, normalize_remote_dir, remote_file_name, RemoteConnection};

const API_URL: &str = "https://api-pan.xunlei.com/drive/v1";
const XLUSER_BASE: &str = "https://xluser-ssl.xunlei.com";
const XLUSER_API: &str = "https://xluser-ssl.xunlei.com/v1";

const CLIENT_ID: &str = "Xp6vsxz_7IYVw2BB";
const CLIENT_SECRET: &str = "Xp6vsy4tN9toTVdMSpomVdXpRmES";
const CLIENT_VERSION: &str = "8.31.0.9726";
const PACKAGE_NAME: &str = "com.xunlei.downloadprovider";
const APP_ID: &str = "40";
const APP_KEY: &str = "34a062aaa22f906fca4fefe9fb3a3021";
const USER_AGENT: &str = "ANDROID-com.xunlei.downloadprovider/8.31.0.9726 netWorkType/5G appid/40 deviceName/Xiaomi_M2004j7ac deviceModel/M2004J7AC OSVersion/12 protocolVersion/301 platformVersion/10 sdkVersion/512000 Oauth2Client/0.9 (Linux 4_14_186-perf-gddfs8vbb238b) (JAVA 0)";
const DOWNLOAD_UA: &str = "Dalvik/2.1.0 (Linux; U; Android 12; M2004J7AC Build/SP1A.210812.016)";
const REDIRECT_URI: &str = "xlaccsdk01://xunlei.com/callback?state=harbor";
const SIGN_PROVIDER: &str = "access_end_point_token";
const ALGORITHMS: [&str; 10] = [
    "9uJNVj/wLmdwKrJaVj/omlQ",
    "Oz64Lp0GigmChHMf/6TNfxx7O9PyopcczMsnf",
    "Eb+L7Ce+Ej48u",
    "jKY0",
    "ASr0zCl6v8W4aidjPK5KHd1Lq3t+vBFf41dqv5+fnOd",
    "wQlozdg6r1qxh0eRmt3QgNXOvSZO6q/GXK",
    "gmirk+ciAvIgA/cxUUCema47jr/YToixTT+Q6O",
    "5IiCoM9B1/788ntB",
    "P07JH0h6qoM6TSUAK2aL9T5s2QBVeY9JWvalf",
    "+oK0AN",
];

fn md5_hex(data: &[u8]) -> String {
    hex::encode(md5::compute(data).0)
}

fn sha1_hex(data: &[u8]) -> String {
    use sha1::{Digest, Sha1};
    hex::encode(Sha1::digest(data))
}

fn device_sign(device_id: &str) -> String {
    let base = format!("{device_id}{PACKAGE_NAME}{APP_ID}{APP_KEY}");
    let digest = md5_hex(sha1_hex(base.as_bytes()).as_bytes());
    format!("div101.{device_id}{digest}")
}

fn base_headers(
    request: reqwest::RequestBuilder,
    device_id: &str,
) -> reqwest::RequestBuilder {
    request
        .header("user-agent", USER_AGENT)
        .header("accept", "application/json;charset=UTF-8")
        .header("x-device-id", device_id)
        .header("x-client-id", CLIENT_ID)
        .header("x-client-version", CLIENT_VERSION)
}

fn json_body(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|_| json!({ "error": text }))
}

fn api_error(value: &Value) -> Option<(i64, String)> {
    let code = value_i64(value, "error_code")
        .or_else(|| value_i64(value, "errorCode"))
        .unwrap_or(0);
    let message = value_str(value, "error").to_string();
    let description = value_str(value, "error_description").to_string();
    let failed = code != 0
        || (!message.is_empty() && message != "success")
        || !description.is_empty();
    if failed {
        let detail = if description.is_empty() {
            message
        } else {
            format!("{message}：{description}")
        };
        Some((code, detail))
    } else {
        None
    }
}

async fn xluser_post(
    http: &reqwest::Client,
    url: &str,
    device_id: &str,
    body: Value,
    captcha_token: Option<&str>,
    user_agent: Option<&str>,
) -> Result<Value, String> {
    let mut request = base_headers(http.post(url), device_id)
        .json(&body)
        .timeout(Duration::from_secs(40));
    if let Some(token) = captcha_token {
        request = request.header("X-Captcha-Token", token);
    }
    if let Some(agent) = user_agent {
        request = request.header("user-agent", agent);
    }
    let response = request
        .send()
        .await
        .map_err(|error| format!("迅雷登录请求失败：{error}"))?;
    let text = response
        .text()
        .await
        .map_err(|error| format!("读取迅雷登录响应失败：{error}"))?;
    let value = json_body(&text);
    if let Some((code, detail)) = api_error(&value) {
        return Err(format!("迅雷登录失败（{code}）：{detail}"));
    }
    Ok(value)
}

#[derive(Default)]
struct TokenData {
    access_token: String,
    refresh_token: String,
    expires_in: i64,
    user_id: String,
}

fn parse_token(value: &Value) -> Result<TokenData, String> {
    let access_token = value_str(value, "access_token").to_string();
    if access_token.is_empty() {
        return Err("迅雷登录未返回 access_token".into());
    }
    let user_id = value_str(value, "user_id");
    Ok(TokenData {
        access_token,
        refresh_token: value_str(value, "refresh_token").to_string(),
        expires_in: value_i64(value, "expires_in").unwrap_or(0),
        user_id: if user_id.is_empty() {
            value_str(value, "sub").to_string()
        } else {
            user_id.to_string()
        },
    })
}

fn account_label(request_label: &str, username: &str) -> String {
    if !request_label.trim().is_empty() {
        request_label.trim().to_string()
    } else if !username.is_empty() {
        username.to_string()
    } else {
        "迅雷云盘".into()
    }
}

fn store_token(account_id: &str, token: CloudToken) -> Result<CloudAccountInfo, String> {
    save_token(account_id, &token)?;
    Ok(CloudAccountInfo {
        account_id: account_id.to_string(),
        provider: super::XUNLEI_PROTOCOL.into(),
        label: token.label.clone(),
        username: token.username.clone(),
    })
}

/// 账号密码登录。
pub async fn login(request: XunleiLoginRequest) -> Result<CloudAccountInfo, String> {
    let account_id = validate_account_id(&request.account_id)?;
    let username = request.username.trim().to_string();
    if username.is_empty() || request.password.is_empty() {
        return Err("请填写迅雷账号与密码".into());
    }
    let http = http_client();
    let device_id = md5_hex(format!("{username}{}", request.password).as_bytes());

    let session_id = core_login(&http, &device_id, &username, &request.password, "").await?;
    let mut captcha_token = String::new();
    init_login_captcha(&http, &device_id, &username, &mut captcha_token).await?;
    let token = signin_token(&http, &device_id, &session_id, &captcha_token).await?;

    let label = account_label(&request.label, &username);
    store_token(
        &account_id,
        CloudToken {
            label,
            username,
            access_token: token.access_token,
            refresh_token: token.refresh_token,
            expires_at: now_ms() + token.expires_in.max(0) * 1000,
            device_id,
            user_id: token.user_id,
            captcha_token,
            ..Default::default()
        },
    )
}

/// 使用已有 refresh token 登录（密码登录受风控拦截时的备用方式）。
pub async fn token_login(request: XunleiTokenLoginRequest) -> Result<CloudAccountInfo, String> {
    let account_id = validate_account_id(&request.account_id)?;
    let refresh_token = request.refresh_token.trim().to_string();
    if refresh_token.is_empty() {
        return Err("请填写迅雷 refresh token".into());
    }
    let device_id = md5_hex(refresh_token.as_bytes());
    let http = http_client();
    let token = refresh_request(&http, &device_id, &refresh_token).await?;
    let label = account_label(&request.label, "");
    let refresh_token = if token.refresh_token.is_empty() {
        refresh_token
    } else {
        token.refresh_token.clone()
    };
    store_token(
        &account_id,
        CloudToken {
            label,
            access_token: token.access_token,
            refresh_token,
            expires_at: now_ms() + token.expires_in.max(0) * 1000,
            device_id,
            user_id: token.user_id,
            ..Default::default()
        },
    )
}

async fn core_login(
    http: &reqwest::Client,
    device_id: &str,
    username: &str,
    password: &str,
    credit_key: &str,
) -> Result<String, String> {
    let body = json!({
        "protocolVersion": "301",
        "sequenceNo": "1000012",
        "platformVersion": "10",
        "isCompressed": "0",
        "appid": APP_ID,
        "clientVersion": CLIENT_VERSION,
        "peerID": "00000000000000000000000000000000",
        "appName": "ANDROID-com.xunlei.downloadprovider",
        "sdkVersion": "512000",
        "devicesign": device_sign(device_id),
        "netWorkType": "WIFI",
        "providerName": "NONE",
        "deviceModel": "M2004J7AC",
        "deviceName": "Xiaomi_M2004j7ac",
        "OSVersion": "12",
        "creditkey": credit_key,
        "hl": "zh-CN",
        "userName": username,
        "passWord": password,
        "verifyKey": "",
        "verifyCode": "",
        "isMd5Pwd": "0",
    });
    let value = xluser_post(
        http,
        &format!("{XLUSER_BASE}/xluser.core.login/v3/login"),
        device_id,
        body,
        None,
        Some("android-ok-http-client/xl-acc-sdk/version-5.0.12.512000"),
    )
    .await?;
    let session_id = value_str(&value, "sessionID").to_string();
    if session_id.is_empty() {
        return Err("迅雷登录未返回 sessionID".into());
    }
    Ok(session_id)
}

async fn init_login_captcha(
    http: &reqwest::Client,
    device_id: &str,
    username: &str,
    out: &mut String,
) -> Result<(), String> {
    let meta = if username.contains('@') {
        json!({ "email": username })
    } else if (11..=18).contains(&username.len()) {
        json!({ "phone_number": username })
    } else {
        json!({ "username": username })
    };
    let body = json!({
        "action": "POST:/v1/auth/signin/token",
        "captcha_token": "",
        "client_id": CLIENT_ID,
        "device_id": device_id,
        "meta": meta,
        "redirect_uri": REDIRECT_URI,
    });
    let value = xluser_post(
        http,
        &format!("{XLUSER_API}/shield/captcha/init"),
        device_id,
        body,
        None,
        None,
    )
    .await?;
    let url = value_str(&value, "url");
    if !url.is_empty() {
        return Err(format!("需要完成迅雷人机验证：{url}"));
    }
    let token = value_str(&value, "captcha_token").to_string();
    if token.is_empty() {
        return Err("迅雷验证码初始化失败".into());
    }
    *out = token;
    Ok(())
}

async fn signin_token(
    http: &reqwest::Client,
    device_id: &str,
    session_id: &str,
    captcha_token: &str,
) -> Result<TokenData, String> {
    let body = json!({
        "client_id": CLIENT_ID,
        "client_secret": CLIENT_SECRET,
        "provider": SIGN_PROVIDER,
        "signin_token": session_id,
    });
    let value = xluser_post(
        http,
        &format!("{XLUSER_API}/auth/signin/token"),
        device_id,
        body,
        Some(captcha_token),
        None,
    )
    .await?;
    parse_token(&value)
}

async fn refresh_request(
    http: &reqwest::Client,
    device_id: &str,
    refresh_token: &str,
) -> Result<TokenData, String> {
    let body = json!({
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
        "client_id": CLIENT_ID,
        "client_secret": CLIENT_SECRET,
    });
    let value = xluser_post(
        http,
        &format!("{XLUSER_API}/auth/token"),
        device_id,
        body,
        None,
        None,
    )
    .await?;
    parse_token(&value)
}

/// 一次迅雷云盘会话：access token 过期自动刷新，验证码过期自动重新初始化。
pub struct XunleiSession {
    http: reqwest::Client,
    account_id: String,
    token: CloudToken,
    dir_ids: HashMap<String, String>,
    entries: HashMap<String, CloudEntry>,
}

impl XunleiSession {
    pub async fn open(connection: &RemoteConnection) -> Result<Self, String> {
        let account_id = super::account_id(connection)?;
        let token = super::load_token(&account_id)?;
        if token.access_token.is_empty() {
            return Err("迅雷云盘登录已失效，请重新登录".into());
        }
        let mut session = Self {
            http: http_client(),
            account_id,
            token,
            dir_ids: HashMap::new(),
            entries: HashMap::new(),
        };
        if session.token.expires_at > 0 && session.token.expires_at <= now_ms() + 300_000 {
            session.refresh().await?;
        }
        Ok(session)
    }

    async fn refresh(&mut self) -> Result<(), String> {
        if self.token.refresh_token.is_empty() {
            return Err("迅雷云盘登录已过期，请重新登录".into());
        }
        let token = refresh_request(
            &self.http,
            &self.token.device_id,
            &self.token.refresh_token,
        )
        .await?;
        self.token.access_token = token.access_token;
        if !token.refresh_token.is_empty() {
            self.token.refresh_token = token.refresh_token;
        }
        if !token.user_id.is_empty() {
            self.token.user_id = token.user_id;
        }
        self.token.expires_at = now_ms() + token.expires_in.max(0) * 1000;
        save_token(&self.account_id, &self.token)?;
        Ok(())
    }

    async fn refresh_captcha(&mut self, action: &str) -> Result<(), String> {
        let timestamp = now_ms().to_string();
        let mut text = format!(
            "{CLIENT_ID}{CLIENT_VERSION}{PACKAGE_NAME}{}{timestamp}",
            self.token.device_id
        );
        for algorithm in ALGORITHMS {
            text = md5_hex(format!("{text}{algorithm}").as_bytes());
        }
        let body = json!({
            "action": action,
            "captcha_token": self.token.captcha_token,
            "client_id": CLIENT_ID,
            "device_id": self.token.device_id,
            "meta": {
                "client_version": CLIENT_VERSION,
                "package_name": PACKAGE_NAME,
                "user_id": self.token.user_id,
                "timestamp": timestamp,
                "captcha_sign": format!("1.{text}"),
            },
            "redirect_uri": REDIRECT_URI,
        });
        let value = xluser_post(
            &self.http,
            &format!("{XLUSER_API}/shield/captcha/init"),
            &self.token.device_id,
            body,
            None,
            None,
        )
        .await?;
        let url = value_str(&value, "url");
        if !url.is_empty() {
            return Err(format!("需要完成迅雷人机验证：{url}"));
        }
        let token = value_str(&value, "captcha_token").to_string();
        if token.is_empty() {
            return Err("迅雷验证码刷新失败".into());
        }
        self.token.captcha_token = token;
        save_token(&self.account_id, &self.token)?;
        Ok(())
    }

    /// 云盘接口：4122 / 4121 / 10 / 16 刷新令牌，9 刷新验证码，各自重试。
    async fn drive(
        &mut self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
        query: &[(String, String)],
    ) -> Result<Value, String> {
        let url = format!("{API_URL}{path}");
        let action = reqwest::Url::parse(&url)
            .map(|parsed| format!("{}:{}", method.as_str(), parsed.path()))
            .unwrap_or_else(|_| format!("{}:{path}", method.as_str()));
        for attempt in 0..3 {
            let mut request = self
                .http
                .request(method.clone(), &url)
                .query(query)
                .header("Authorization", format!("Bearer {}", self.token.access_token))
                .header("X-Captcha-Token", &self.token.captcha_token)
                .timeout(Duration::from_secs(60));
            request = base_headers(request, &self.token.device_id);
            if let Some(body) = &body {
                request = request.json(body);
            }
            let response = request
                .send()
                .await
                .map_err(|error| format!("迅雷云盘请求失败：{error}"))?;
            let status = response.status();
            let text = response
                .text()
                .await
                .map_err(|error| format!("读取迅雷云盘响应失败：{error}"))?;
            if text.trim().is_empty() {
                if status.is_success() {
                    return Ok(json!({}));
                }
                return Err(format!("迅雷云盘请求失败：HTTP {}", status.as_u16()));
            }
            let value = json_body(&text);
            match api_error(&value) {
                None => return Ok(value),
                Some((code, detail)) => {
                    if matches!(code, 4122 | 4121 | 10 | 16) && attempt < 2 {
                        self.refresh().await?;
                        continue;
                    }
                    if code == 9 && attempt < 2 {
                        self.refresh_captcha(&action).await?;
                        continue;
                    }
                    return Err(format!("迅雷云盘接口错误（{code}）：{detail}"));
                }
            }
        }
        Err("迅雷云盘请求失败".into())
    }

    async fn list_raw(&mut self, parent_id: &str) -> Result<Vec<CloudEntry>, String> {
        let mut entries = Vec::new();
        let mut page_token = String::new();
        loop {
            let query = vec![
                ("space".to_string(), String::new()),
                ("__type".to_string(), "drive".into()),
                ("refresh".to_string(), "true".into()),
                ("__sync".to_string(), "true".into()),
                ("parent_id".to_string(), parent_id.to_string()),
                ("page_token".to_string(), page_token.clone()),
                ("with_audit".to_string(), "true".into()),
                ("limit".to_string(), "100".into()),
                (
                    "filters".to_string(),
                    r#"{"phase":{"eq":"PHASE_TYPE_COMPLETE"},"trashed":{"eq":false}}"#.into(),
                ),
            ];
            let value = self
                .drive(reqwest::Method::GET, "/files", None, &query)
                .await?;
            let files = value
                .get("files")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for file in &files {
                let name = value_str(file, "name").to_string();
                if name.is_empty() {
                    continue;
                }
                let id = value_str(file, "id").to_string();
                let is_dir = value_str(file, "kind") == "drive#folder";
                let modified = value_str(file, "modified_time");
                let modified = chrono::DateTime::parse_from_rfc3339(modified)
                    .map(|stamp| stamp.timestamp_millis())
                    .unwrap_or(0);
                entries.push(CloudEntry {
                    name,
                    path: String::new(),
                    id,
                    is_dir,
                    size: value_str(file, "size").parse::<u64>().unwrap_or(0),
                    modified,
                });
            }
            page_token = value_str(&value, "next_page_token").to_string();
            if page_token.is_empty() {
                break;
            }
        }
        Ok(entries)
    }

    async fn resolve_dir_id(&mut self, path: &str) -> Result<String, String> {
        let path = normalize_remote_dir(path);
        if path == "/" {
            return Ok(String::new());
        }
        if let Some(id) = self.dir_ids.get(&path) {
            return Ok(id.clone());
        }
        let parent = parent_path(&path);
        let name = remote_file_name(&path)?;
        let parent_id = Box::pin(self.resolve_dir_id(&parent)).await?;
        let entry = self
            .list_raw(&parent_id)
            .await?
            .into_iter()
            .find(|entry| entry.is_dir && entry.name == name)
            .ok_or_else(|| format!("迅雷云盘目录不存在：{path}"))?;
        self.dir_ids.insert(path, entry.id.clone());
        Ok(entry.id)
    }

    pub async fn list(&mut self, path: &str) -> Result<Vec<CloudEntry>, String> {
        let path = normalize_remote_dir(path);
        let parent_id = self.resolve_dir_id(&path).await?;
        let mut entries = self.list_raw(&parent_id).await?;
        for entry in &mut entries {
            entry.path = join_remote(&path, &entry.name);
            if entry.is_dir {
                self.dir_ids
                    .insert(entry.path.clone(), entry.id.clone());
            }
            self.entries.insert(entry.path.clone(), entry.clone());
        }
        Ok(entries)
    }

    pub async fn lookup(&mut self, path: &str) -> Result<CloudEntry, String> {
        let path = normalize_remote_dir(path);
        if path == "/" {
            return Err("不能下载云端根目录".into());
        }
        if let Some(entry) = self.entries.get(&path) {
            return Ok(entry.clone());
        }
        let parent = parent_path(&path);
        let name = remote_file_name(&path)?;
        let parent_id = self.resolve_dir_id(&parent).await?;
        let mut entry = self
            .list_raw(&parent_id)
            .await?
            .into_iter()
            .find(|entry| entry.name == name)
            .ok_or_else(|| format!("迅雷云盘文件不存在：{path}"))?;
        entry.path = path.clone();
        if entry.is_dir {
            self.dir_ids.insert(path.clone(), entry.id.clone());
        }
        self.entries.insert(path, entry.clone());
        Ok(entry)
    }

    pub async fn download(&mut self, entry: &CloudEntry, local: &Path) -> Result<u64, String> {
        if entry.id.is_empty() {
            return Err("迅雷云盘未返回文件 id".into());
        }
        let value = self
            .drive(
                reqwest::Method::GET,
                &format!("/files/{}", entry.id),
                None,
                &[("space".to_string(), String::new())],
            )
            .await?;
        let link = value_str(&value, "web_content_link");
        if link.is_empty() {
            return Err("迅雷云盘未返回下载地址".into());
        }
        let response = self
            .http
            .get(link)
            .header(reqwest::header::USER_AGENT, DOWNLOAD_UA)
            .send()
            .await
            .map_err(|error| format!("迅雷云盘下载失败：{error}"))?;
        if !response.status().is_success() {
            return Err(format!(
                "迅雷云盘下载失败：HTTP {}",
                response.status().as_u16()
            ));
        }
        save_stream(response, local).await
    }

    pub async fn upload(&mut self, _local: &Path, _remote: &str) -> Result<u64, String> {
        Err("迅雷云盘暂不支持在 Stelix 内上传，请使用迅雷客户端".into())
    }

    pub async fn ensure_dir(&mut self, _remote: &str) -> Result<(), String> {
        Err("迅雷云盘暂不支持在 Stelix 内上传，请使用迅雷客户端".into())
    }

    pub async fn remove(&mut self, path: &str) -> Result<(), String> {
        let path = normalize_remote_dir(path);
        if path == "/" {
            return Err("不能删除云端根目录".into());
        }
        let entry = self.lookup(&path).await?;
        self.drive(
            reqwest::Method::PATCH,
            &format!("/files/{}/trash", entry.id),
            Some(json!({})),
            &[("space".to_string(), String::new())],
        )
        .await?;
        self.entries.remove(&path);
        self.dir_ids
            .retain(|cached, _| cached != &path && !cached.starts_with(&format!("{path}/")));
        Ok(())
    }
}

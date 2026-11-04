//! Secure storage for remote connection credentials.
//!
//! 密码与私钥口令分别存放在系统凭据库的不同 service 下：同一主机的两种认证方式
//! 可以并存，切换认证方式时不会互相覆盖。

use once_cell::sync::Lazy;
use parking_lot::Mutex;

const SERVICE: &str = "stelix.remote-host";
const KEY_SERVICE: &str = "stelix.remote-host-key";
/// 云存储（百度网盘 / 迅雷云盘）令牌：同一账号的 access / refresh token 以 JSON 保存。
const CLOUD_SERVICE: &str = "stelix.cloud-account";
static CREDENTIALS_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

fn entry(service: &str, host_id: &str) -> Result<keyring::Entry, String> {
    if host_id.is_empty() || host_id.len() > 256 {
        return Err("主机标识无效".into());
    }
    keyring::Entry::new(service, host_id).map_err(|error| format!("无法访问系统凭据库：{error}"))
}

fn save(service: &str, host_id: &str, secret: &str) -> Result<(), String> {
    let _guard = CREDENTIALS_LOCK.lock();
    entry(service, host_id)?
        .set_password(secret)
        .map_err(|error| format!("无法保存主机凭据：{error}"))
}

fn load(service: &str, host_id: &str) -> Result<Option<String>, String> {
    let _guard = CREDENTIALS_LOCK.lock();
    match entry(service, host_id)?.get_password() {
        Ok(secret) => Ok(Some(secret)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(format!("无法读取主机凭据：{error}")),
    }
}

fn delete(service: &str, host_id: &str) -> Result<(), String> {
    let _guard = CREDENTIALS_LOCK.lock();
    match entry(service, host_id)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(format!("无法删除主机凭据：{error}")),
    }
}

pub fn save_remote_password(host_id: &str, password: &str) -> Result<(), String> {
    save(SERVICE, host_id, password)
}

pub fn get_remote_password(host_id: &str) -> Result<Option<String>, String> {
    load(SERVICE, host_id)
}

pub fn delete_remote_password(host_id: &str) -> Result<(), String> {
    delete(SERVICE, host_id)
}

/// 私钥口令（有密码短语的私钥）：与登录密码分开保存，避免切换认证方式时丢失。
pub fn save_remote_key_passphrase(host_id: &str, passphrase: &str) -> Result<(), String> {
    save(KEY_SERVICE, host_id, passphrase)
}

pub fn get_remote_key_passphrase(host_id: &str) -> Result<Option<String>, String> {
    load(KEY_SERVICE, host_id)
}

pub fn delete_remote_key_passphrase(host_id: &str) -> Result<(), String> {
    delete(KEY_SERVICE, host_id)
}

/// 云存储账号令牌（JSON：access / refresh token、过期时间、OAuth 客户端信息等）。
pub fn save_cloud_token(account_id: &str, token_json: &str) -> Result<(), String> {
    save(CLOUD_SERVICE, account_id, token_json)
}

pub fn get_cloud_token(account_id: &str) -> Result<Option<String>, String> {
    load(CLOUD_SERVICE, account_id)
}

pub fn delete_cloud_token(account_id: &str) -> Result<(), String> {
    delete(CLOUD_SERVICE, account_id)
}

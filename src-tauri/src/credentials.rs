//! Secure storage for remote connection credentials.

use once_cell::sync::Lazy;
use parking_lot::Mutex;

const SERVICE: &str = "file-povit.remote-host";
static CREDENTIALS_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

fn entry(host_id: &str) -> Result<keyring::Entry, String> {
    if host_id.is_empty() || host_id.len() > 256 {
        return Err("主机标识无效".into());
    }
    keyring::Entry::new(SERVICE, host_id).map_err(|error| format!("无法访问系统凭据库：{error}"))
}

pub fn save_remote_password(host_id: &str, password: &str) -> Result<(), String> {
    let _guard = CREDENTIALS_LOCK.lock();
    entry(host_id)?
        .set_password(password)
        .map_err(|error| format!("无法保存主机密码：{error}"))
}

pub fn get_remote_password(host_id: &str) -> Result<Option<String>, String> {
    let _guard = CREDENTIALS_LOCK.lock();
    match entry(host_id)?.get_password() {
        Ok(password) => Ok(Some(password)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(format!("无法读取主机密码：{error}")),
    }
}

pub fn delete_remote_password(host_id: &str) -> Result<(), String> {
    let _guard = CREDENTIALS_LOCK.lock();
    match entry(host_id)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(format!("无法删除主机密码：{error}")),
    }
}

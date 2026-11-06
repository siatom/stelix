use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{Emitter, Manager};

const RELEASE_URL: &str = "https://api.github.com/repos/siatom/stelix/releases/latest";
const MAX_DOWNLOAD_BYTES: u64 = 500 * 1024 * 1024;

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    name: Option<String>,
    body: Option<String>,
    html_url: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<GithubAsset>,
}

#[derive(Deserialize)]
struct GithubAsset {
    name: String,
    size: u64,
    browser_download_url: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    current_version: String,
    version: String,
    available: bool,
    title: String,
    notes: String,
    release_url: String,
    asset_name: Option<String>,
    asset_url: Option<String>,
    asset_size: Option<u64>,
}

pub async fn check() -> Result<UpdateInfo, String> {
    let current = env!("CARGO_PKG_VERSION").to_string();
    let client = reqwest::Client::builder()
        .user_agent("Stelix-File-Manager")
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;
    let release: GithubRelease = client
        .get(RELEASE_URL)
        .send()
        .await
        .map_err(|e| format!("检查 GitHub Release 失败：{e}"))?
        .error_for_status()
        .map_err(|e| format!("读取 GitHub Release 失败：{e}"))?
        .json()
        .await
        .map_err(|e| format!("解析 GitHub Release 失败：{e}"))?;
    if release.draft || release.prerelease {
        return Err("当前 Release 尚未正式发布".into());
    }
    let version = release.tag_name.trim_start_matches('v');
    let latest = parse_version(version).ok_or_else(|| "Release 版本号格式无效".to_string())?;
    let installed = parse_version(&current).ok_or_else(|| "当前版本号格式无效".to_string())?;
    let asset = release
        .assets
        .into_iter()
        .find(|asset| asset_matches(&asset.name));
    let available = compare_versions(&latest, &installed).is_gt();
    Ok(UpdateInfo {
        current_version: current,
        version: version.to_string(),
        available,
        title: release.name.unwrap_or_else(|| format!("Stelix v{version}")),
        notes: release.body.unwrap_or_default(),
        release_url: release.html_url,
        asset_name: asset.as_ref().map(|a| a.name.clone()),
        asset_url: asset.as_ref().map(|a| a.browser_download_url.clone()),
        asset_size: asset.map(|a| a.size),
    })
}

fn parse_version(value: &str) -> Option<Vec<u64>> {
    let core = value.split(['-', '+']).next()?;
    let parts: Vec<u64> = core
        .split('.')
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    (!parts.is_empty()).then_some(parts)
}

fn compare_versions(left: &[u64], right: &[u64]) -> std::cmp::Ordering {
    (0..left.len().max(right.len()))
        .map(|i| {
            left.get(i)
                .copied()
                .unwrap_or(0)
                .cmp(&right.get(i).copied().unwrap_or(0))
        })
        .find(|ordering| !ordering.is_eq())
        .unwrap_or(std::cmp::Ordering::Equal)
}

fn asset_matches(name: &str) -> bool {
    #[cfg(target_os = "windows")]
    {
        name.ends_with("_x64-setup.exe")
    }
    #[cfg(target_os = "macos")]
    {
        if cfg!(target_arch = "aarch64") {
            name.ends_with("_aarch64.dmg")
        } else {
            name.ends_with("_x64.dmg")
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = name;
        false
    }
}

#[tauri::command]
pub async fn updater_check() -> Result<UpdateInfo, String> {
    check().await
}

#[tauri::command]
pub async fn updater_download(
    app: tauri::AppHandle,
    url: String,
    name: String,
) -> Result<String, String> {
    if !url.starts_with("https://github.com/siatom/stelix/releases/download/")
        || !asset_matches(&name)
        || name.contains('/')
        || name.contains('\\')
    {
        return Err("更新包地址或文件名无效".into());
    }
    let dir = app
        .path()
        .temp_dir()
        .map_err(|e| e.to_string())?
        .join("stelix-updates");
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| e.to_string())?;
    let path: PathBuf = dir.join(&name);
    let client = reqwest::Client::builder()
        .user_agent("Stelix-File-Manager")
        .build()
        .map_err(|e| e.to_string())?;
    let mut response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("下载更新失败：{e}"))?
        .error_for_status()
        .map_err(|e| format!("下载更新失败：{e}"))?;
    if response
        .content_length()
        .is_some_and(|n| n > MAX_DOWNLOAD_BYTES)
    {
        return Err("更新包超过大小限制".into());
    }
    let total = response.content_length().unwrap_or(0);
    let mut file = tokio::fs::File::create(&path)
        .await
        .map_err(|e| e.to_string())?;
    let mut downloaded = 0u64;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| format!("下载更新失败：{e}"))?
    {
        downloaded += chunk.len() as u64;
        if downloaded > MAX_DOWNLOAD_BYTES {
            let _ = tokio::fs::remove_file(&path).await;
            return Err("更新包超过大小限制".into());
        }
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk)
            .await
            .map_err(|e| e.to_string())?;
        let _ = app.emit("updater:progress", (downloaded, total));
    }
    tokio::io::AsyncWriteExt::flush(&mut file)
        .await
        .map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().into_owned())
}

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_opener::OpenerExt;

const GOZ_RELEASE_API: &str = "https://api.github.com/repos/mustafaahci/goz/releases/latest";
const VOIDTOOLS_DOWNLOADS: &str = "https://www.voidtools.com/downloads/";

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackendDownloadInfo {
    pub backend: String,
    pub version: String,
    pub file_name: String,
    pub size: u64,
}

struct ResolvedDownload {
    info: BackendDownloadInfo,
    url: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BackendDownloadProgress<'a> {
    backend: &'a str,
    downloaded: u64,
    total: u64,
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
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

pub async fn download_info(backend: &str) -> Result<BackendDownloadInfo, String> {
    resolve_download(backend)
        .await
        .map(|resolved| resolved.info)
}

async fn resolve_download(backend: &str) -> Result<ResolvedDownload, String> {
    if !cfg!(windows) {
        return Err("Search backend downloads are only available on Windows".into());
    }
    if !cfg!(target_arch = "x86_64") {
        return Err(
            "Automatic search backend downloads are currently supported on Windows x64 only".into(),
        );
    }

    let client = reqwest::Client::builder()
        .user_agent("Stelix-File-Manager")
        .timeout(std::time::Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| error.to_string())?;

    match backend {
        "goz" => goz_download_info(&client).await,
        "everything" => everything_download_info(&client).await,
        _ => Err("Unsupported search backend".into()),
    }
}

async fn goz_download_info(client: &reqwest::Client) -> Result<ResolvedDownload, String> {
    let release: GithubRelease = client
        .get(GOZ_RELEASE_API)
        .send()
        .await
        .map_err(|error| format!("Could not check goz releases: {error}"))?
        .error_for_status()
        .map_err(|error| format!("Could not read the latest goz release: {error}"))?
        .json()
        .await
        .map_err(|error| format!("Could not parse the latest goz release: {error}"))?;

    if release.draft || release.prerelease {
        return Err("The latest goz release is not a stable release".into());
    }

    let asset = release
        .assets
        .into_iter()
        .find(|asset| asset.name.starts_with("goz-") && asset.name.ends_with("-windows-x86_64.zip"))
        .ok_or_else(|| "The latest goz release has no Windows x86_64 archive".to_string())?;
    validate_goz_asset_url(&asset.browser_download_url)?;
    let version = release.tag_name.trim_start_matches('v');
    if version.is_empty()
        || !version
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '.' || ch == '-')
    {
        return Err("The latest goz release has an invalid version".into());
    }

    Ok(ResolvedDownload {
        info: BackendDownloadInfo {
            backend: "goz".into(),
            version: version.into(),
            file_name: asset.name,
            size: asset.size,
        },
        url: asset.browser_download_url,
    })
}

async fn everything_download_info(client: &reqwest::Client) -> Result<ResolvedDownload, String> {
    let html = client
        .get(VOIDTOOLS_DOWNLOADS)
        .send()
        .await
        .map_err(|error| format!("Could not check voidtools downloads: {error}"))?
        .error_for_status()
        .map_err(|error| format!("Could not read voidtools downloads: {error}"))?
        .text()
        .await
        .map_err(|error| format!("Could not read voidtools download page: {error}"))?;

    let stable_start = html
        .find("id=\"dl\"")
        .ok_or_else(|| "Could not locate the stable Everything downloads".to_string())?;
    let stable_end = html[stable_start..]
        .find("id=\"dl15\"")
        .map(|offset| stable_start + offset)
        .ok_or_else(|| "Could not locate the end of the stable Everything downloads".to_string())?;
    let stable_html = &html[stable_start..stable_end];
    let href = find_x64_installer_href(stable_html)
        .ok_or_else(|| "The stable Everything x64 installer was not found".to_string())?;
    let url = reqwest::Url::parse(VOIDTOOLS_DOWNLOADS)
        .and_then(|base| base.join(href))
        .map_err(|error| format!("The stable Everything download URL is invalid: {error}"))?;
    validate_voidtools_url(&url)?;

    let file_name = href
        .rsplit('/')
        .next()
        .ok_or_else(|| "The stable Everything installer name is invalid".to_string())?;
    let version = everything_version(file_name)
        .ok_or_else(|| "The stable Everything installer version is invalid".to_string())?;

    Ok(ResolvedDownload {
        info: BackendDownloadInfo {
            backend: "everything".into(),
            version,
            file_name: file_name.into(),
            size: 0,
        },
        url: url.to_string(),
    })
}

#[tauri::command]
pub async fn search_backend_download_info(backend: String) -> Result<BackendDownloadInfo, String> {
    download_info(&backend).await
}

#[tauri::command]
pub async fn download_search_backend(app: AppHandle, backend: String) -> Result<String, String> {
    let resolved = resolve_download(&backend).await?;
    let name = resolved.info.file_name;
    if name.is_empty()
        || !name.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_')
        })
    {
        return Err("The backend download has an invalid file name".into());
    }

    let dir = app
        .path()
        .download_dir()
        .or_else(|_| app.path().temp_dir())
        .map_err(|error| format!("Could not locate a download folder: {error}"))?
        .join("Stelix-Backends");
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|error| format!("Could not create the backend download folder: {error}"))?;
    let name = available_download_name(&dir, &name).await?;

    let client = reqwest::Client::builder()
        .user_agent("Stelix-File-Manager")
        .timeout(std::time::Duration::from_secs(180))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            let url = attempt.url();
            let trusted_host = matches!(
                url.host_str(),
                Some("github.com")
                    | Some("release-assets.githubusercontent.com")
                    | Some("objects.githubusercontent.com")
                    | Some("www.voidtools.com")
            );
            if url.scheme() == "https" && trusted_host && attempt.previous().len() < 5 {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .build()
        .map_err(|error| error.to_string())?;

    let event_backend = backend.clone();
    let path = crate::updater::download_core(
        &client,
        &resolved.url,
        &dir,
        &name,
        crate::updater::MAX_DOWNLOAD_BYTES,
        |downloaded, total| {
            let _ = app.emit(
                "search-backend:download-progress",
                BackendDownloadProgress {
                    backend: &event_backend,
                    downloaded,
                    total,
                },
            );
        },
        crate::updater::open_temp_file,
    )
    .await?;

    app.opener()
        .open_path(&path, None::<&str>)
        .map_err(|error| {
            format!("Downloaded the backend package but could not open it: {error}")
        })?;
    Ok(path)
}

async fn available_download_name(dir: &std::path::Path, name: &str) -> Result<String, String> {
    if tokio::fs::metadata(dir.join(name)).await.is_err() {
        return Ok(name.to_string());
    }
    let path = std::path::Path::new(name);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "The backend download file name is invalid".to_string())?;
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    for index in 1..1000 {
        let candidate = if extension.is_empty() {
            format!("{stem} ({index})")
        } else {
            format!("{stem} ({index}).{extension}")
        };
        if tokio::fs::metadata(dir.join(&candidate)).await.is_err() {
            return Ok(candidate);
        }
    }
    Err("Could not choose a unique backend download file name".into())
}

fn find_x64_installer_href(html: &str) -> Option<&str> {
    let mut remaining = html;
    while let Some(href_start) = remaining.find("href=\"") {
        remaining = &remaining[href_start + 6..];
        let href_end = remaining.find('"')?;
        let href = &remaining[..href_end];
        if href.starts_with("/Everything-")
            && href.ends_with(".x64-Setup.exe")
            && !href.contains(".Lite-")
        {
            return Some(href);
        }
        remaining = &remaining[href_end + 1..];
    }
    None
}

fn everything_version(file_name: &str) -> Option<String> {
    let version = file_name
        .strip_prefix("Everything-")?
        .strip_suffix(".x64-Setup.exe")?;
    if version.is_empty()
        || !version
            .chars()
            .all(|character| character.is_ascii_digit() || character == '.')
    {
        return None;
    }
    Some(version.to_string())
}

fn validate_goz_asset_url(value: &str) -> Result<(), String> {
    let url =
        reqwest::Url::parse(value).map_err(|error| format!("Invalid goz download URL: {error}"))?;
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || url.port().is_some()
        || !url
            .path()
            .starts_with("/mustafaahci/goz/releases/download/")
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("The goz release points to an unexpected download source".into());
    }
    Ok(())
}

fn validate_voidtools_url(url: &reqwest::Url) -> Result<(), String> {
    if url.scheme() != "https"
        || url.host_str() != Some("www.voidtools.com")
        || url.port().is_some()
        || !url.path().starts_with("/Everything-")
        || !url.path().ends_with(".x64-Setup.exe")
        || url.path().contains(".Lite-")
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("The Everything installer points to an unexpected download source".into());
    }
    Ok(())
}

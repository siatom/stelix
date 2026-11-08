use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{Emitter, Manager};
use tokio::io::AsyncWriteExt;

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
    validate_download_request(&url, &name)?;
    let dir = app
        .path()
        .temp_dir()
        .map_err(|e| e.to_string())?
        .join("stelix-updates");
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| e.to_string())?;
    let client = reqwest::Client::builder()
        .user_agent("Stelix-File-Manager")
        .build()
        .map_err(|e| e.to_string())?;
    download_core(
        &client,
        &url,
        &dir,
        &name,
        MAX_DOWNLOAD_BYTES,
        |downloaded, total| {
            let _ = app.emit("updater:progress", (downloaded, total));
        },
        open_temp_file,
    )
    .await
}

fn validate_download_request(url: &str, name: &str) -> Result<(), String> {
    if !url.starts_with("https://github.com/siatom/stelix/releases/download/")
        || !asset_matches(name)
        || name.contains('/')
        || name.contains('\\')
    {
        return Err("更新包地址或文件名无效".into());
    }
    Ok(())
}

/// 追加写入之外还需要落盘，才能保证发布时文件内容真正完整。
trait DownloadSink: tokio::io::AsyncWrite + Unpin {
    async fn sync_all(&mut self) -> std::io::Result<()>;
}

impl DownloadSink for tokio::fs::File {
    async fn sync_all(&mut self) -> std::io::Result<()> {
        tokio::fs::File::sync_all(self).await
    }
}

async fn open_temp_file(path: PathBuf) -> std::io::Result<tokio::fs::File> {
    tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .await
}

/// 下载到同目录的独立临时文件，确认完整后再发布为最终安装包。
///
/// 所有失败路径只删除本次临时文件，不会动已经存在的完整安装包；
/// 临时文件与最终文件同目录，保证 rename 在 Windows / macOS 上都是替换语义。
async fn download_core<W, F, O, Fut>(
    client: &reqwest::Client,
    url: &str,
    dir: &std::path::Path,
    name: &str,
    max_bytes: u64,
    mut on_progress: F,
    mut open: O,
) -> Result<String, String>
where
    W: DownloadSink,
    F: FnMut(u64, u64),
    O: FnMut(PathBuf) -> Fut,
    Fut: std::future::Future<Output = std::io::Result<W>>,
{
    let final_path = dir.join(name);
    let temp_path = dir.join(format!(".{name}.{}.part", uuid::Uuid::new_v4()));
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("下载更新失败：{e}"))?
        .error_for_status()
        .map_err(|e| format!("下载更新失败：{e}"))?;
    let expected = response.content_length();
    if expected.is_some_and(|n| n > max_bytes) {
        return Err("更新包超过大小限制".into());
    }
    let mut sink = open(temp_path.clone())
        .await
        .map_err(|e| format!("创建临时文件失败：{e}"))?;
    if let Err(error) = stream_response(
        &mut response,
        &mut sink,
        expected,
        max_bytes,
        &mut on_progress,
    )
    .await
    {
        drop(sink);
        let _ = tokio::fs::remove_file(&temp_path).await;
        return Err(error);
    }
    if let Err(error) = sink.sync_all().await {
        drop(sink);
        let _ = tokio::fs::remove_file(&temp_path).await;
        return Err(format!("写入更新包失败：{error}"));
    }
    drop(sink);
    if let Err(error) = tokio::fs::rename(&temp_path, &final_path).await {
        let _ = tokio::fs::remove_file(&temp_path).await;
        return Err(format!("发布更新包失败：{error}"));
    }
    Ok(final_path.to_string_lossy().into_owned())
}

async fn stream_response<W, F>(
    response: &mut reqwest::Response,
    sink: &mut W,
    expected: Option<u64>,
    max_bytes: u64,
    on_progress: &mut F,
) -> Result<u64, String>
where
    W: tokio::io::AsyncWrite + Unpin,
    F: FnMut(u64, u64),
{
    let total = expected.unwrap_or(0);
    let mut downloaded = 0u64;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| format!("下载更新失败：{e}"))?
    {
        downloaded += chunk.len() as u64;
        if downloaded > max_bytes {
            return Err("更新包超过大小限制".into());
        }
        sink.write_all(&chunk)
            .await
            .map_err(|e| format!("写入更新包失败：{e}"))?;
        on_progress(downloaded, total);
    }
    if let Some(expected) = expected {
        if downloaded != expected {
            return Err(format!(
                "更新包不完整：预期 {expected} 字节，实际 {downloaded} 字节"
            ));
        }
    }
    sink.flush()
        .await
        .map_err(|e| format!("写入更新包失败：{e}"))?;
    Ok(downloaded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use tokio::io::AsyncReadExt;
    use tokio::net::TcpListener;

    const NAME: &str = "stelix_0.2.3_x64-setup.exe";

    struct MockServer {
        port: u16,
    }

    impl MockServer {
        /// 启动一次性本地 HTTP 服务；`send_bytes` 用于模拟中途断流。
        async fn start(head: String, body: Vec<u8>, send_bytes: Option<usize>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定端口");
            let port = listener.local_addr().expect("读取端口").port();
            tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.expect("接受连接");
                let mut buf = [0u8; 2048];
                loop {
                    let read = socket.read(&mut buf).await.expect("读取请求");
                    if read == 0 || buf[..read].windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                socket.write_all(head.as_bytes()).await.expect("写响应头");
                let count = send_bytes.unwrap_or(body.len()).min(body.len());
                socket.write_all(&body[..count]).await.expect("写响应体");
                let _ = socket.shutdown().await;
            });
            Self { port }
        }

        fn url(&self) -> String {
            format!("http://127.0.0.1:{}/stelix", self.port)
        }
    }

    fn head_with_length(length: usize) -> String {
        format!("HTTP/1.1 200 OK\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n")
    }

    fn head_without_length() -> String {
        "HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_string()
    }

    fn client() -> reqwest::Client {
        reqwest::Client::builder().build().expect("构建客户端")
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "stelix-updater-test-{}-{tag}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).expect("创建测试目录");
        dir
    }

    fn entries(dir: &std::path::Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .expect("读取目录")
            .map(|entry| {
                entry
                    .expect("读取目录项")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        names
    }

    fn payload(len: usize, seed: u8) -> Vec<u8> {
        (0..len)
            .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
            .collect()
    }

    struct FailingSink {
        inner: tokio::fs::File,
        fail_write: bool,
        fail_flush: bool,
    }

    impl tokio::io::AsyncWrite for FailingSink {
        fn poll_write(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            if self.fail_write {
                return Poll::Ready(Err(std::io::Error::other("注入写入失败")));
            }
            Pin::new(&mut self.inner).poll_write(cx, buf)
        }

        fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            if self.fail_flush {
                return Poll::Ready(Err(std::io::Error::other("注入 flush 失败")));
            }
            Pin::new(&mut self.inner).poll_flush(cx)
        }

        fn poll_shutdown(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<std::io::Result<()>> {
            Pin::new(&mut self.inner).poll_shutdown(cx)
        }
    }

    impl DownloadSink for FailingSink {
        async fn sync_all(&mut self) -> std::io::Result<()> {
            self.inner.sync_all().await
        }
    }

    async fn failing_sink(
        path: PathBuf,
        fail_write: bool,
        fail_flush: bool,
    ) -> std::io::Result<FailingSink> {
        let inner = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .await?;
        Ok(FailingSink {
            inner,
            fail_write,
            fail_flush,
        })
    }

    #[tokio::test]
    async fn publishes_complete_file_and_reports_progress() {
        let dir = temp_dir("success");
        let expected = payload(4096, 7);
        let server =
            MockServer::start(head_with_length(expected.len()), expected.clone(), None).await;
        let mut events = Vec::new();
        let returned = download_core(
            &client(),
            &server.url(),
            &dir,
            NAME,
            MAX_DOWNLOAD_BYTES,
            |done, total| events.push((done, total)),
            open_temp_file,
        )
        .await
        .expect("下载成功");

        assert_eq!(PathBuf::from(returned), dir.join(NAME));
        assert_eq!(
            std::fs::read(dir.join(NAME)).expect("读取最终安装包"),
            expected
        );
        assert_eq!(
            events.last().copied(),
            Some((expected.len() as u64, expected.len() as u64)),
            "进度事件应更新到完整字节数"
        );
        assert_eq!(
            entries(&dir),
            vec![NAME.to_string()],
            "成功下载只留下最终安装包"
        );
    }

    #[tokio::test]
    async fn successful_download_replaces_existing_package() {
        let dir = temp_dir("replace");
        std::fs::write(dir.join(NAME), payload(1024, 1)).expect("预置旧安装包");
        let expected = payload(2048, 2);
        let server =
            MockServer::start(head_with_length(expected.len()), expected.clone(), None).await;
        download_core(
            &client(),
            &server.url(),
            &dir,
            NAME,
            MAX_DOWNLOAD_BYTES,
            |_, _| {},
            open_temp_file,
        )
        .await
        .expect("下载成功");
        assert_eq!(
            std::fs::read(dir.join(NAME)).expect("读取最终安装包"),
            expected
        );
        assert_eq!(entries(&dir), vec![NAME.to_string()]);
    }

    #[tokio::test]
    async fn declared_oversize_is_rejected_without_touching_existing_package() {
        let dir = temp_dir("oversize-declared");
        let existing = payload(1024, 3);
        std::fs::write(dir.join(NAME), &existing).expect("预置旧安装包");
        let server = MockServer::start(head_with_length(9), Vec::new(), None).await;
        let error = download_core(
            &client(),
            &server.url(),
            &dir,
            NAME,
            8,
            |_, _| {},
            open_temp_file,
        )
        .await
        .unwrap_err();
        assert!(error.contains("超过大小限制"));
        assert_eq!(
            std::fs::read(dir.join(NAME)).expect("读取旧安装包"),
            existing
        );
        assert_eq!(entries(&dir), vec![NAME.to_string()]);
    }

    #[tokio::test]
    async fn stream_over_limit_is_rejected_and_temp_removed() {
        let dir = temp_dir("oversize-stream");
        let existing = payload(512, 4);
        std::fs::write(dir.join(NAME), &existing).expect("预置旧安装包");
        let server = MockServer::start(head_without_length(), payload(2048, 5), None).await;
        let error = download_core(
            &client(),
            &server.url(),
            &dir,
            NAME,
            1024,
            |_, _| {},
            open_temp_file,
        )
        .await
        .unwrap_err();
        assert!(error.contains("超过大小限制"));
        assert_eq!(
            std::fs::read(dir.join(NAME)).expect("读取旧安装包"),
            existing
        );
        assert_eq!(
            entries(&dir),
            vec![NAME.to_string()],
            "超限失败不留下本次临时文件"
        );
    }

    #[tokio::test]
    async fn truncated_response_fails_and_keeps_existing_package() {
        let dir = temp_dir("truncated");
        let existing = payload(4096, 6);
        std::fs::write(dir.join(NAME), &existing).expect("预置旧安装包");
        let expected = payload(8192, 7);
        let server =
            MockServer::start(head_with_length(expected.len()), expected, Some(1024)).await;
        let error = download_core(
            &client(),
            &server.url(),
            &dir,
            NAME,
            MAX_DOWNLOAD_BYTES,
            |_, _| {},
            open_temp_file,
        )
        .await
        .unwrap_err();
        assert!(error.starts_with("下载更新失败"));
        assert_eq!(
            std::fs::read(dir.join(NAME)).expect("读取旧安装包"),
            existing,
            "中途断流不得破坏上一次的完整安装包"
        );
        assert_eq!(entries(&dir), vec![NAME.to_string()]);
    }

    #[tokio::test]
    async fn write_failure_is_reported_and_temp_removed() {
        let dir = temp_dir("write-failure");
        let existing = payload(1024, 8);
        std::fs::write(dir.join(NAME), &existing).expect("预置旧安装包");
        let server = MockServer::start(head_with_length(4096), payload(4096, 9), None).await;
        let error = download_core(
            &client(),
            &server.url(),
            &dir,
            NAME,
            MAX_DOWNLOAD_BYTES,
            |_, _| {},
            |path| failing_sink(path, true, false),
        )
        .await
        .unwrap_err();
        assert!(error.contains("写入更新包失败"));
        assert_eq!(
            std::fs::read(dir.join(NAME)).expect("读取旧安装包"),
            existing
        );
        assert_eq!(entries(&dir), vec![NAME.to_string()]);
    }

    #[tokio::test]
    async fn flush_failure_is_reported_and_temp_removed() {
        let dir = temp_dir("flush-failure");
        let existing = payload(1024, 10);
        std::fs::write(dir.join(NAME), &existing).expect("预置旧安装包");
        let server = MockServer::start(head_with_length(4096), payload(4096, 11), None).await;
        let error = download_core(
            &client(),
            &server.url(),
            &dir,
            NAME,
            MAX_DOWNLOAD_BYTES,
            |_, _| {},
            |path| failing_sink(path, false, true),
        )
        .await
        .unwrap_err();
        assert!(error.contains("写入更新包失败"));
        assert_eq!(
            std::fs::read(dir.join(NAME)).expect("读取旧安装包"),
            existing
        );
        assert_eq!(entries(&dir), vec![NAME.to_string()]);
    }

    #[test]
    fn rejects_untrusted_urls_and_names() {
        assert!(validate_download_request("https://example.com/pkg", NAME).is_err());
        assert!(validate_download_request(
            "https://github.com/siatom/stelix/releases/download/v1/stelix_0.2.3_x64-setup.exe",
            "../stelix_0.2.3_x64-setup.exe"
        )
        .is_err());
    }
}

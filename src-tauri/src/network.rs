//! FTP/SFTP 目录浏览。凭据仅随本次调用传入 curl 的标准输入，不写入配置文件。
use std::io::Write;
use std::process::{Command, Stdio};

use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteRequest {
    pub protocol: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub path: String,
}

fn quote_config(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r");
    format!("\"{escaped}\"")
}

pub fn list_remote(request: RemoteRequest) -> Result<Vec<String>, String> {
    if request.protocol != "ftp" && request.protocol != "sftp" {
        return Err("仅支持 FTP 和 SFTP".into());
    }
    if request.host.is_empty() || request.host.contains(['/', '\\', '@', ':', '\n', '\r']) {
        return Err("主机地址无效".into());
    }
    if request.port == 0 || request.path.split('/').any(|part| part == "..") {
        return Err("端口或远程路径无效".into());
    }

    let mut url = reqwest::Url::parse(&format!(
        "{}://{}:{}/",
        request.protocol, request.host, request.port
    ))
    .map_err(|_| "主机地址无效")?;
    {
        let mut segments = url.path_segments_mut().map_err(|_| "远程路径无效")?;
        segments.clear();
        for part in request.path.split('/').filter(|part| !part.is_empty()) {
            segments.push(part);
        }
        segments.push("");
    }

    let config = format!(
        "url = {}\nuser = {}\n",
        quote_config(url.as_str()),
        quote_config(&format!("{}:{}", request.username, request.password))
    );
    let mut command = Command::new("curl.exe");
    command.args([
        "--config",
        "-",
        "--silent",
        "--show-error",
        "--fail",
        "--list-only",
        "--max-time",
        "20",
    ]);
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
        .map_err(|error| format!("无法启动 curl：{error}"))?;
    child
        .stdin
        .take()
        .ok_or("无法传递连接参数")?
        .write_all(config.as_bytes())
        .map_err(|error| format!("无法传递连接参数：{error}"))?;
    let output = child
        .wait_with_output()
        .map_err(|error| format!("远程读取失败：{error}"))?;
    if !output.status.success() {
        return Err(format!(
            "连接或目录读取失败：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let mut names: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|name| {
            !name.is_empty() && *name != "." && *name != ".." && !name.contains(['/', '\\'])
        })
        .map(ToOwned::to_owned)
        .collect();
    names.sort_by_key(|name| name.to_lowercase());
    Ok(names)
}

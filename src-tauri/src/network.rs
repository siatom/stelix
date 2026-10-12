//! FTP/SFTP 目录浏览。凭据仅随本次调用传入 curl 的标准输入，不写入配置文件。
//!
//! 列目录取 curl 的**长格式**输出（FTP 的 `LIST`、SFTP 的 readdir 长名），再解析为
//! 带类型 / 大小 / 修改时间的 `DirListing`，供文件窗格浏览远程目录。
//! 解析逻辑全部是纯函数（`parse_listing` 等），便于用固定样本单测覆盖。
use std::io::Write;
use std::process::{Command, Stdio};

use chrono::{Datelike, Local, NaiveDate};
use serde::Deserialize;

use crate::fs_core::path_id;
use crate::types::{DirListing, FileEntry};

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

/// 校验请求并拼出指向该目录（以 `/` 结尾）的远程 URL。
fn build_url(request: &RemoteRequest) -> Result<reqwest::Url, String> {
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
    Ok(url)
}

/// 运行 curl 取目录长格式列表（返回 stdout 原始字节，可能不是 UTF-8）。
///
/// 不带 `--list-only`：FTP 会走 `LIST`、SFTP 走 readdir 长名，从而带上类型与大小；
/// 仅返回名字的 NLST 无法区分目录与文件。
fn run_curl_listing(request: &RemoteRequest) -> Result<Vec<u8>, String> {
    let url = build_url(request)?;
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
    Ok(output.stdout)
}

/// UTF-8 优先，失败时用 lossy 兜底（中文 FTP 服务器的 GBK 文件名会显示为替换字符）。
fn decode(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// 列出远程目录（只读）。
pub fn list_remote_dir(request: RemoteRequest) -> Result<DirListing, String> {
    let raw = run_curl_listing(&request)?;
    let text = decode(&raw);
    Ok(DirListing {
        path: request.path.clone(),
        entries: parse_listing(&text, &request.path),
    })
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
            parsed.push(entry);
        }
    }

    // 兜底：整份输出没有一行能识别（服务器用了非 Unix / 非 DOS 方言）时，
    // 退回旧的 NLST 语义 —— 整行当名字、全部视为目录，至少保住左侧树能展开。
    if parsed.is_empty() && candidates > 0 {
        parsed = raw
            .lines()
            .map(|line| line.trim_end_matches('\r').trim())
            .filter(|line| !line.is_empty() && !is_total_line(line) && !line.contains(['/', '\\']))
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
fn join_remote(base: &str, name: &str) -> String {
    let name = name.trim_start_matches('/');
    let base = base.trim_end_matches('/');
    if base.is_empty() {
        format!("/{name}")
    } else {
        format!("{base}/{name}")
    }
}

fn remote_ext(name: &str) -> Option<String> {
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

/// 解析 Unix 长格式行（FTP `LIST` 与 SFTP 长名通用）。
///
/// 字段：`权限 链接数 属主 [属组] 大小 月份 日 时间|年份 名字…`
/// —— 属组可能缺失，故用「月份字段的位置」定位大小与名字，而非固定下标。
fn parse_unix_line(line: &str, now_year: i32) -> Option<ParsedLine> {
    let tokens = tokens_with_offsets(line);
    let perms = tokens.first()?.0;
    if perms.len() < 10 {
        return None;
    }
    let kind = perms.chars().next()?;
    if !matches!(kind, 'd' | '-' | 'l' | 'b' | 'c' | 'p' | 's' | 'D') {
        return None;
    }
    if !perms
        .chars()
        .skip(1)
        .all(|c| matches!(c, 'r' | 'w' | 'x' | '-' | 's' | 'S' | 't' | 'T'))
    {
        return None;
    }

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
        let request = RemoteRequest {
            protocol: "http".into(),
            host: "example.com".into(),
            port: 80,
            username: String::new(),
            password: String::new(),
            path: "/".into(),
        };
        assert!(build_url(&request).is_err());

        let request = RemoteRequest {
            protocol: "sftp".into(),
            host: "example.com".into(),
            port: 22,
            username: String::new(),
            password: String::new(),
            path: "/../etc".into(),
        };
        assert!(build_url(&request).is_err());
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

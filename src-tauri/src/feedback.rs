//! 意见反馈：调用本机 GitHub CLI (`gh`) 直接把反馈创建为 GitHub issue。
//!
//! 设计要点（都是踩过的坑）：
//! - **必须显式 `--repo <owner>/<repo>`**：CLI 默认从当前工作目录的 git remote
//!   推断仓库，打包成安装版后应用的工作目录任意（甚至不在任何 git 仓库内），
//!   不指定必然失败。
//! - **必须 `GH_PROMPT_DISABLED=1`**：应用内没有 TTY，CLI 若进入交互式提示会挂住。
//! - **正文走 `--body-file -` + stdin**：正文含换行 / 引号 / 中文，也避免超长参数
//!   触发 Windows 命令行长度限制。
//! - **`CREATE_NO_WINDOW`**：GUI 进程启动控制台程序不加会让系统闪一个终端窗口。
//! - GUI 进程的 PATH 可能不包含新装的 gh，额外探测常见安装目录。

use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use crate::types::{FeedbackAuth, FeedbackCreated, FeedbackRequest};

/// 反馈目标仓库。打包后 cwd 不可靠，所以写死而不是依赖 CLI 推断。
const FEEDBACK_REPO: &str = "siatom/stelix";

/// 安装 GitHub CLI 的提示（CLI 不存在时展示给用户）。
#[cfg(windows)]
const INSTALL_HINT: &str =
    "未检测到 GitHub CLI (gh)。请先安装并登录：\n  1. winget install --id GitHub.cli\n  2. gh auth login";
#[cfg(target_os = "macos")]
const INSTALL_HINT: &str =
    "未检测到 GitHub CLI (gh)。请先安装并登录：\n  1. brew install gh\n  2. gh auth login";
#[cfg(all(not(windows), not(target_os = "macos")))]
const INSTALL_HINT: &str = "未检测到 GitHub CLI (gh)。请先安装并登录：\n  1. 参考 https://cli.github.com 安装\n  2. gh auth login";

/// gh 可执行文件。
struct GhCli {
    path: PathBuf,
}

impl GhCli {
    /// 按 args 运行 CLI 并收集输出（不经过 shell）。
    ///
    /// `stdin_body` 为 `Some` 时把正文写入子进程 stdin（配合 `--body-file -`）。
    fn run(&self, args: &[&str], stdin_body: Option<&str>) -> std::io::Result<Output> {
        let mut command = Command::new(&self.path);
        command
            .args(args)
            // 应用内没有 TTY，禁用 gh 的所有交互式提示。
            .env("GH_PROMPT_DISABLED", "1")
            .env("GH_NO_UPDATE_NOTIFIER", "1")
            .env("NO_COLOR", "1")
            .env("CLICOLOR", "0")
            .stdin(if stdin_body.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            // ⚠ `spawn()` 不会自动接管 stdout/stderr，必须显式 piped，
            // 否则 `wait_with_output()` 拿到的是空输出（登录状态会误判为未登录）。
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW：避免 GUI 进程拉起控制台子进程时闪出终端窗口。
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = command.spawn()?;
        if let Some(body) = stdin_body {
            use std::io::Write;
            if let Some(mut stdin) = child.stdin.take() {
                // 写入失败也继续等待，避免留下僵尸进程；错误在 wait 后由调用方处理。
                let _ = stdin.write_all(body.as_bytes());
            }
        }
        child.wait_with_output()
    }
}

#[cfg(windows)]
fn find_gh() -> Option<GhCli> {
    // 1) PATH 中的原生可执行文件（winget / scoop / 手动安装）。
    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            for name in ["gh.exe", "gh.com"] {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    return Some(GhCli { path: candidate });
                }
            }
        }
    }

    // 2) 常见安装目录：GUI 进程启动时 PATH 可能尚未包含新安装的 gh。
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(program_files) = std::env::var_os("ProgramFiles") {
        roots.push(PathBuf::from(program_files).join("GitHub CLI"));
    }
    if let Some(program_files_x86) = std::env::var_os("ProgramFiles(x86)") {
        roots.push(PathBuf::from(program_files_x86).join("GitHub CLI"));
    }
    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
        roots.push(
            PathBuf::from(local_app_data)
                .join("Programs")
                .join("GitHub CLI"),
        );
    }
    if let Some(profile) = std::env::var_os("USERPROFILE") {
        roots.push(PathBuf::from(profile).join("scoop").join("shims"));
    }
    for root in roots {
        let candidate = root.join("gh.exe");
        if candidate.is_file() {
            return Some(GhCli { path: candidate });
        }
    }
    None
}

#[cfg(not(windows))]
fn find_gh() -> Option<GhCli> {
    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join("gh");
            if candidate.is_file() {
                return Some(GhCli { path: candidate });
            }
        }
    }
    // macOS 从 Finder / Dock 启动时 PATH 往往只有系统目录。
    for dir in ["/usr/local/bin", "/opt/homebrew/bin", "/usr/bin"] {
        let candidate = PathBuf::from(dir).join("gh");
        if candidate.is_file() {
            return Some(GhCli { path: candidate });
        }
    }
    None
}

/// 读取 GitHub CLI 当前登录状态，供反馈表单提示用。
pub fn auth_status() -> FeedbackAuth {
    let Some(gh) = find_gh() else {
        return FeedbackAuth {
            cli_found: false,
            logged_in: false,
            user: None,
            message: INSTALL_HINT.to_string(),
        };
    };

    // 优先用 JSON 输出（gh 2.40+），避免解析会随版本变化的文案。
    if let Ok(output) = gh.run(&["auth", "status", "--json", "hosts"], None) {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if let Some((logged_in, user)) = parse_auth_json(&stdout) {
                return auth_result(logged_in, user);
            }
        }
    }

    // 回退：旧版 gh 没有 `--json`，解析文本输出（可能写到 stderr）。
    match gh.run(&["auth", "status"], None) {
        Ok(output) => {
            let text = format!(
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            let (logged_in, user) = parse_auth_text(&text);
            auth_result(logged_in, user)
        }
        Err(error) => FeedbackAuth {
            cli_found: true,
            logged_in: false,
            user: None,
            message: format!("无法运行 GitHub CLI：{error}"),
        },
    }
}

/// 把探测结果转成前端展示的状态。
fn auth_result(logged_in: bool, user: Option<String>) -> FeedbackAuth {
    FeedbackAuth {
        cli_found: true,
        logged_in,
        user: user.clone(),
        message: if logged_in {
            match user {
                Some(name) => format!("已登录 GitHub CLI（{name}）"),
                None => "已登录 GitHub CLI".to_string(),
            }
        } else {
            "GitHub CLI 尚未登录。请在终端执行：gh auth login".to_string()
        },
    }
}

/// 提交一条反馈，成功时返回新建 issue 的编号与链接。
pub fn submit(request: &FeedbackRequest) -> Result<FeedbackCreated, String> {
    let title = request.title.trim();
    if title.is_empty() {
        return Err("请填写反馈标题".to_string());
    }
    let description = request.description.trim();
    if description.is_empty() {
        return Err("请填写详细描述".to_string());
    }

    let gh = find_gh().ok_or_else(|| INSTALL_HINT.to_string())?;
    let full_title = format!("[{}] {}", request.kind.trim(), title);
    let body = build_body(request, description);

    let output = gh
        .run(
            &[
                "issue",
                "create",
                "--repo",
                FEEDBACK_REPO,
                "--title",
                &full_title,
                "--body-file",
                "-",
            ],
            Some(&body),
        )
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => INSTALL_HINT.to_string(),
            _ => format!("无法运行 GitHub CLI：{error}"),
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail = if stderr.trim().is_empty() {
            first_line(&stdout)
        } else {
            first_line(&stderr)
        };
        let lowered = detail.to_ascii_lowercase();
        if lowered.contains("auth login")
            || lowered.contains("not logged")
            || lowered.contains("authentication")
        {
            return Err(format!(
                "GitHub CLI 尚未登录，请先在终端执行 gh auth login。\n{detail}"
            ));
        }
        return Err(format!("提交失败：{detail}"));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_created(&stdout)
        .ok_or_else(|| format!("已提交，但无法解析 gh 的返回结果：{}", first_line(&stdout)))
}

/// 组装 issue 正文：反馈类型 + 用户描述 + 运行环境。
fn build_body(request: &FeedbackRequest, description: &str) -> String {
    let os = match std::env::consts::OS {
        "windows" => "Windows",
        "macos" => "macOS",
        "linux" => "Linux",
        other => other,
    };
    let version = if request.app_version.trim().is_empty() {
        "未知".to_string()
    } else {
        request.app_version.trim().to_string()
    };
    let commit = if request.commit.trim().is_empty() {
        "未知".to_string()
    } else {
        request.commit.trim().to_string()
    };
    format!(
        "## 反馈类型\n\n{kind}\n\n## 详细描述\n\n{description}\n\n---\n\n- 应用：Stelix 星索\n- 版本：{version}\n- Commit：{commit}\n- 系统：{os} ({arch})",
        kind = request.kind.trim(),
        description = description,
        version = version,
        commit = commit,
        os = os,
        arch = std::env::consts::ARCH,
    )
}

/// 解析 `gh auth status --json hosts` 的 JSON，取 github.com 的登录状态。
fn parse_auth_json(json: &str) -> Option<(bool, Option<String>)> {
    let value: serde_json::Value = serde_json::from_str(slice_json(json)?).ok()?;
    let hosts = value.get("hosts")?.as_object()?;
    let Some(entries) = hosts.get("github.com").and_then(|item| item.as_array()) else {
        return Some((false, None));
    };
    for entry in entries {
        let state = entry
            .get("state")
            .and_then(|item| item.as_str())
            .unwrap_or("");
        if !state.eq_ignore_ascii_case("success") {
            continue;
        }
        let user = entry
            .get("login")
            .and_then(|item| item.as_str())
            .filter(|name| !name.is_empty())
            .map(str::to_string);
        return Some((true, user));
    }
    Some((false, None))
}

/// 解析旧版 `gh auth status` 的文本输出。
fn parse_auth_text(text: &str) -> (bool, Option<String>) {
    for line in text.lines() {
        let Some(index) = line.find("Logged in to github.com") else {
            continue;
        };
        let tail = line[index + "Logged in to github.com".len()..].trim_start();
        // 新版本是 `account <login>`，旧版本是 `as <login>`。
        let tail = tail
            .strip_prefix("account ")
            .or_else(|| tail.strip_prefix("as "))
            .unwrap_or(tail);
        let user = tail
            .split_whitespace()
            .next()
            .filter(|name| !name.is_empty())
            .map(str::to_string);
        return (true, user);
    }
    (false, None)
}

/// 解析 `gh issue create` 的成功输出（stdout 即 issue 网页链接）。
fn parse_created(stdout: &str) -> Option<FeedbackCreated> {
    let url = stdout
        .lines()
        .map(str::trim)
        .find(|line| {
            line.starts_with("https://")
                && line.contains("/issues/")
                && !line.contains(char::is_whitespace)
        })?
        .trim_end_matches('/')
        .to_string();
    let number = url.rsplit('/').next()?;
    if number.is_empty() || !number.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    Some(FeedbackCreated {
        number: number.to_string(),
        url,
    })
}

/// 截取字符串中最外层的 JSON 对象，容忍 CLI 在 JSON 前后输出的额外日志行。
fn slice_json(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    (end > start).then(|| &text[start..=end])
}

fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("未知错误")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_child_stdout() {
        // 回归测试：`spawn()` + `wait_with_output()` 必须显式 piped stdout/stderr，
        // 否则 CLI 输出会被继承到父进程终端，程序内部拿到空字符串。
        #[cfg(windows)]
        let (program, args) = (
            std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".to_string()),
            vec!["/C", "echo hello"],
        );
        #[cfg(not(windows))]
        let (program, args) = ("sh", vec!["-c", "echo hello"]);

        let cli = GhCli {
            path: PathBuf::from(program),
        };
        let output = cli.run(&args, None).expect("应当能运行测试命令");
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("hello"));
    }

    #[test]
    fn parses_logged_in_json() {
        let json = r#"{"hosts":{"github.com":[{"state":"success","active":true,"host":"github.com","login":"siatom"}]}}"#;
        assert_eq!(
            parse_auth_json(json),
            Some((true, Some("siatom".to_string())))
        );
    }

    #[test]
    fn parses_empty_hosts_json() {
        assert_eq!(parse_auth_json(r#"{"hosts":{}}"#), Some((false, None)));
    }

    #[test]
    fn ignores_failed_host_state() {
        let json = r#"{"hosts":{"github.com":[{"state":"error","login":"siatom"}]}}"#;
        assert_eq!(parse_auth_json(json), Some((false, None)));
    }

    #[test]
    fn tolerates_extra_log_lines() {
        let text = "warning: something\n{\"hosts\":{\"github.com\":[{\"state\":\"success\",\"login\":\"u\"}]}}\n";
        assert_eq!(parse_auth_json(text), Some((true, Some("u".to_string()))));
    }

    #[test]
    fn parses_legacy_auth_text() {
        let text = "github.com\n  ✓ Logged in to github.com account siatom (keyring)\n";
        assert_eq!(parse_auth_text(text), (true, Some("siatom".to_string())));
    }

    #[test]
    fn parses_legacy_auth_text_with_as() {
        let text = "  ✓ Logged in to github.com as siatom (oauth_token)\n";
        assert_eq!(parse_auth_text(text), (true, Some("siatom".to_string())));
    }

    #[test]
    fn detects_logged_out_text() {
        let text = "You are not logged into any GitHub hosts. To log in, run: gh auth login\n";
        assert_eq!(parse_auth_text(text), (false, None));
    }

    #[test]
    fn parses_created_issue() {
        let stdout = "https://github.com/siatom/stelix/issues/42\n";
        let created = parse_created(stdout).expect("应当解析成功");
        assert_eq!(created.number, "42");
        assert_eq!(created.url, "https://github.com/siatom/stelix/issues/42");
    }

    #[test]
    fn rejects_non_issue_output() {
        assert!(parse_created("Creating issue...").is_none());
    }

    #[test]
    fn builds_body_with_environment() {
        let request = FeedbackRequest {
            kind: "Bug".to_string(),
            title: "标题".to_string(),
            description: "描述".to_string(),
            app_version: "0.1.0".to_string(),
            commit: "abc1234".to_string(),
        };
        let body = build_body(&request, "描述");
        assert!(body.contains("## 反馈类型\n\nBug"));
        assert!(body.contains("## 详细描述\n\n描述"));
        assert!(body.contains("- 版本：0.1.0"));
        assert!(body.contains("- Commit：abc1234"));
        assert!(body.contains(&format!("({})", std::env::consts::ARCH)));
        assert!(body.contains("\n- 系统："));
    }
}

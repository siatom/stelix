//! 意见反馈：调用本机 `gitee` CLI 直接把反馈创建为 Gitee issue。
//!
//! 设计要点（都是踩过的坑）：
//! - **必须显式 `-R <owner>/<repo>`**：CLI 默认从当前工作目录的 git remote 推断仓库，
//!   打包成安装版后应用的工作目录任意（甚至不在任何 git 仓库内），不指定必然失败。
//! - **必须 `--no-tui`**：应用内没有 TTY，CLI 默认会进入交互式 TUI（选 label）而挂住。
//! - **直接 `Command::args(...)` 传参，不拼 shell**：正文含换行 / 引号 / 中文，经 cmd 会
//!   出现转义与编码问题（实测 `gitee.cmd` 会把带换行的参数截断，故 npm 安装形态优先走
//!   `node <…>/@gitee/gitee-cli/bin/index.js`，由 node 直接转发参数）。
//! - **`CREATE_NO_WINDOW`**：GUI 进程启动控制台程序不加会让系统闪一个终端窗口。

use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use crate::types::{FeedbackAuth, FeedbackCreated, FeedbackRequest};

/// 反馈目标仓库。打包后 cwd 不可靠，所以写死而不是依赖 CLI 推断。
const FEEDBACK_REPO: &str = "silicon-atom/stelix";

/// 安装 gitee CLI 的提示（CLI 不存在时展示给用户）。
const INSTALL_HINT: &str =
    "未检测到 gitee CLI。请先安装并登录：\n  1. npm i -g @gitee/gitee-cli\n  2. gitee auth login";

/// gitee CLI 的一种可执行调用方式。
enum GiteeCli {
    /// 原生可执行文件（`gitee.exe`），直接调用。
    Binary(PathBuf),
    /// npm 全局安装的形态：`gitee.cmd` 只是转发脚本，改用
    /// `node <…>/node_modules/@gitee/gitee-cli/bin/index.js` 调用，避免 cmd 参数截断。
    NodeScript { node: PathBuf, script: PathBuf },
}

impl GiteeCli {
    /// 按 args 运行 CLI 并收集输出（不经过 shell）。
    fn run(&self, args: &[&str]) -> std::io::Result<Output> {
        let mut command = match self {
            GiteeCli::Binary(path) => Command::new(path),
            GiteeCli::NodeScript { node, script } => {
                let mut command = Command::new(node);
                command.arg(script);
                command
            }
        };
        command.args(args).stdin(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW：避免 GUI 进程拉起控制台子进程时闪出终端窗口。
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        command.output()
    }
}

#[cfg(windows)]
fn find_gitee() -> Option<GiteeCli> {
    let path_var = std::env::var_os("PATH")?;
    let dirs: Vec<PathBuf> = std::env::split_paths(&path_var).collect();

    // 1) 原生可执行文件（scoop / winget / 手动下载等形态）。
    //    ⚠ 只认 .exe/.com：npm 全局安装会在同目录生成 `gitee.cmd`，若把 .cmd 当可执行
    //    文件直接 spawn，cmd 转发多行参数时会报 "batch file arguments are invalid"
    //    并把正文截断。.cmd/.bat 一律走下面的 node 直调分支。
    for dir in &dirs {
        for ext in [".exe", ".com"] {
            let candidate = dir.join(format!("gitee{ext}"));
            if candidate.is_file() {
                return Some(GiteeCli::Binary(candidate));
            }
        }
    }

    // 2) npm 全局安装：由同目录的 `gitee.cmd` 反推 node 脚本入口
    for dir in &dirs {
        if !dir.join("gitee.cmd").is_file() {
            continue;
        }
        let script = dir
            .join("node_modules")
            .join("@gitee")
            .join("gitee-cli")
            .join("bin")
            .join("index.js");
        if !script.is_file() {
            continue;
        }
        if let Some(node) = find_node(&dirs) {
            return Some(GiteeCli::NodeScript { node, script });
        }
    }
    None
}

#[cfg(not(windows))]
fn find_gitee() -> Option<GiteeCli> {
    Some(GiteeCli::Binary(PathBuf::from("gitee")))
}

#[cfg(windows)]
fn find_node(dirs: &[PathBuf]) -> Option<PathBuf> {
    for dir in dirs {
        for name in ["node.exe", "node.cmd"] {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// 读取 gitee CLI 当前登录状态，供反馈表单提示用。
pub fn auth_status() -> FeedbackAuth {
    let Some(cli) = find_gitee() else {
        return FeedbackAuth {
            cli_found: false,
            logged_in: false,
            user: None,
            message: INSTALL_HINT.to_string(),
        };
    };
    match cli.run(&["auth", "status", "--no-tui", "-j"]) {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            match parse_auth(&stdout) {
                Some((true, user)) => FeedbackAuth {
                    cli_found: true,
                    logged_in: true,
                    user: user.clone(),
                    message: match user {
                        Some(name) => format!("已登录 gitee CLI（{name}）"),
                        None => "已登录 gitee CLI".to_string(),
                    },
                },
                Some((false, _)) => FeedbackAuth {
                    cli_found: true,
                    logged_in: false,
                    user: None,
                    message: "gitee CLI 尚未登录。请在终端执行：gitee auth login".to_string(),
                },
                None => FeedbackAuth {
                    cli_found: true,
                    logged_in: false,
                    user: None,
                    message: "无法解析 gitee CLI 的登录状态，请在终端执行 gitee auth status 确认。"
                        .to_string(),
                },
            }
        }
        Ok(output) => FeedbackAuth {
            cli_found: true,
            logged_in: false,
            user: None,
            message: format!(
                "读取 gitee CLI 登录状态失败：{}",
                first_line(&String::from_utf8_lossy(&output.stderr))
            ),
        },
        Err(error) => FeedbackAuth {
            cli_found: true,
            logged_in: false,
            user: None,
            message: format!("无法运行 gitee CLI：{error}"),
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

    let cli = find_gitee().ok_or_else(|| INSTALL_HINT.to_string())?;
    let full_title = format!("[{}] {}", request.kind.trim(), title);
    let body = build_body(request, description);

    let output = cli
        .run(&[
            "issue",
            "create",
            "--no-tui",
            "-j",
            "-R",
            FEEDBACK_REPO,
            "-t",
            &full_title,
            "-b",
            &body,
        ])
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => INSTALL_HINT.to_string(),
            _ => format!("无法运行 gitee CLI：{error}"),
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail = if stderr.trim().is_empty() {
            first_line(&stdout)
        } else {
            first_line(&stderr)
        };
        if detail.contains("not logged in") || detail.contains("auth login") {
            return Err(format!("gitee CLI 尚未登录，请先在终端执行 gitee auth login。\n{detail}"));
        }
        return Err(format!("提交失败：{detail}"));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_created(&stdout)
        .ok_or_else(|| format!("已提交，但无法解析 gitee CLI 的返回结果：{}", first_line(&stdout)))
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

/// 解析 `gitee auth status -j` 的 JSON：`{"status":"logged in","user":"…"}`。
fn parse_auth(json: &str) -> Option<(bool, Option<String>)> {
    let value: serde_json::Value = serde_json::from_str(slice_json(json)?).ok()?;
    let status = value.get("status")?.as_str()?.trim().to_ascii_lowercase();
    // ⚠ 不能用 contains("logged in")：未登录时状态是 `not logged in`，会被误判为已登录。
    let logged_in = status == "logged in" || status.starts_with("logged in ");
    let user = value
        .get("user")
        .and_then(|item| item.as_str())
        .filter(|name| !name.is_empty())
        .map(str::to_string);
    Some((logged_in, user))
}

/// 解析 `gitee issue create -j` 的 JSON，取 issue 编号与链接。
fn parse_created(json: &str) -> Option<FeedbackCreated> {
    let value: serde_json::Value = serde_json::from_str(slice_json(json)?).ok()?;
    let number = value.get("number")?.as_str()?.to_string();
    let url = value.get("html_url")?.as_str()?.to_string();
    Some(FeedbackCreated { number, url })
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
    fn parses_logged_in_status() {
        let json = r#"{"host":"gitee.com","name":"硅原子","status":"logged in","user":"silicon-atom"}"#;
        assert_eq!(parse_auth(json), Some((true, Some("silicon-atom".to_string()))));
    }

    #[test]
    fn parses_logged_out_status() {
        let json = r#"{"host":"gitee.com","status":"not logged in"}"#;
        assert_eq!(parse_auth(json), Some((false, None)));
    }

    #[test]
    fn tolerates_extra_log_lines() {
        let text = "warning: something\n{\"status\":\"logged in\",\"user\":\"u\"}\n";
        assert_eq!(parse_auth(text), Some((true, Some("u".to_string()))));
    }

    #[test]
    fn parses_created_issue() {
        let json = r#"{"id":1,"number":"IKL8XX","html_url":"https://gitee.com/o/r/issues/IKL8XX"}"#;
        let created = parse_created(json).expect("应当解析成功");
        assert_eq!(created.number, "IKL8XX");
        assert_eq!(created.url, "https://gitee.com/o/r/issues/IKL8XX");
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

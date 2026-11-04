//! 单次操作提权（Windows UAC）。
//!
//! 主进程始终以普通权限运行。当某次文件操作因「拒绝访问」失败时，把该 `FsOp`
//! 序列化到临时文件，用 `ShellExecuteExW(verb = "runas")` 拉起**本程序自身**的一个
//! 提权子进程；子进程只执行这一次操作并把结果写成 JSON，主进程读取后清理。
//!
//! 设计要点：
//! - **子进程入口必须在 `tauri::Builder` / single-instance 插件之前拦截**
//!   （见 `main.rs`）：否则会创建单实例互斥体、窗口与 WebView2，与正在运行的主实例冲突。
//! - **防递归**：`run_elevated_fs_op` 首行判 `is_elevated()`；子进程只调
//!   `fs_core::execute`（它本身不提权），因此不可能无限弹 UAC。
//! - **路径经命令行传递时逐个加引号**：临时目录可能含空格，否则子进程 argv 被拆断。
//! - 临时文件放在 `%TEMP%\stelix-elevate`，用 uuid 命名 + `create_new(true)` 写入。

use std::path::{Path, PathBuf};

use crate::types::{FsOp, FsOpResult};

/// 提权子进程的命令行开关。
const FLAG_OP: &str = "--elevated-fs-op";
const FLAG_RESULT: &str = "--elevated-result";
/// 临时文件目录（`%TEMP%\stelix-elevate`），随用随清。
const TEMP_SUBDIR: &str = "stelix-elevate";
/// 残留临时文件的保留时长：超过它就在启动时清扫。
const STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

fn temp_dir() -> PathBuf {
    std::env::temp_dir().join(TEMP_SUBDIR)
}

/// 从 args 中取 `--flag <value>` 的 value。
fn arg_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == flag)
        .and_then(|index| args.get(index + 1))
        .cloned()
}

// ---------------------------------------------------------------- 子进程入口

/// 若命令行带 `--elevated-fs-op`，执行一次文件操作并写结果文件，返回退出码；
/// 否则返回 `None`（= 普通启动，应继续走 `run()`）。
///
/// 退出码只表示「提权子进程本身是否跑完」（是否写出了结果文件），操作本身的成败
/// 由结果文件里的 `FsOpResult.ok` 表达。
pub fn try_run_elevated_helper() -> Option<i32> {
    let args: Vec<String> = std::env::args().collect();
    let op_path = arg_value(&args, FLAG_OP)?;
    let result_path =
        arg_value(&args, FLAG_RESULT).unwrap_or_else(|| format!("{op_path}.result.json"));
    let wrote = run_helper(Path::new(&op_path), Path::new(&result_path));
    Some(if wrote { 0 } else { 1 })
}

/// 读 op JSON → `fs_core::execute` → 写结果 JSON。返回是否成功写出结果文件。
fn run_helper(op_path: &Path, result_path: &Path) -> bool {
    let result = std::fs::read_to_string(op_path)
        .ok()
        .and_then(|text| serde_json::from_str::<FsOp>(&text).ok())
        .map(|op| crate::fs_core::execute(&op, &crate::fs_core::OpContext::noop()))
        .unwrap_or_else(|| FsOpResult {
            ok: false,
            errors: vec!["提权子进程无法读取操作参数".to_string()],
            denied: Vec::new(),
            ..Default::default()
        });
    match serde_json::to_string(&result) {
        Ok(text) => std::fs::write(result_path, text).is_ok(),
        Err(_) => false,
    }
}

// ---------------------------------------------------------------- 主进程侧

/// 当前进程是否已以管理员身份运行。
#[cfg(windows)]
pub fn is_elevated() -> bool {
    unsafe { windows_sys::Win32::UI::Shell::IsUserAnAdmin() != 0 }
}

#[cfg(not(windows))]
pub fn is_elevated() -> bool {
    false
}

/// 提权执行一次因权限不足失败的文件操作。
#[cfg(not(windows))]
pub fn run_elevated_fs_op(_op: &FsOp) -> Result<FsOpResult, String> {
    Err("当前平台不支持提权执行".to_string())
}

#[cfg(windows)]
pub fn run_elevated_fs_op(op: &FsOp) -> Result<FsOpResult, String> {
    use std::io::Write;

    // 已是管理员仍失败 → 提权无济于事；同时天然阻断「子进程再提权」的递归。
    if is_elevated() {
        return Err("当前已是管理员权限，仍无法完成该操作".to_string());
    }

    let exe = std::env::current_exe().map_err(|e| format!("无法定位程序路径：{e}"))?;
    let dir = temp_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("无法创建临时目录：{e}"))?;

    let id = uuid::Uuid::new_v4();
    let op_path = dir.join(format!("{id}.op.json"));
    let result_path = dir.join(format!("{id}.result.json"));

    let json = serde_json::to_vec(op).map_err(|e| format!("序列化操作失败：{e}"))?;
    // create_new(true)：避免命中预先存在的同名文件（本地 TOCTOU）。
    {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&op_path)
            .map_err(|e| format!("无法写入临时参数文件：{e}"))?;
        file.write_all(&json)
            .map_err(|e| format!("无法写入临时参数文件：{e}"))?;
    }

    let outcome = launch_elevated(&exe, &op_path, &result_path);

    // 成功 / 取消 / 失败都清理临时文件（进程被杀的残留由启动清扫兜底）。
    let _ = std::fs::remove_file(&op_path);
    let _ = std::fs::remove_file(&result_path);
    outcome
}

#[cfg(windows)]
fn launch_elevated(exe: &Path, op_path: &Path, result_path: &Path) -> Result<FsOpResult, String> {
    use std::ffi::OsStr;
    use std::mem::size_of;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_CANCELLED};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, WaitForSingleObject, INFINITE,
    };
    use windows_sys::Win32::UI::Shell::{
        ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
    };

    fn wide(value: &OsStr) -> Vec<u16> {
        value.encode_wide().chain(std::iter::once(0)).collect()
    }

    let verb = wide(OsStr::new("runas"));
    let file = wide(exe.as_os_str());
    // 路径可能含空格 → 必须逐个加引号，否则子进程 argv 会被拆断。
    let params = wide(OsStr::new(&format!(
        "{FLAG_OP} \"{}\" {FLAG_RESULT} \"{}\"",
        op_path.to_string_lossy(),
        result_path.to_string_lossy(),
    )));

    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = params.as_ptr();
    info.nShow = 0; // SW_HIDE：提权子进程不显示窗口

    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        let code = unsafe { GetLastError() };
        return if code == ERROR_CANCELLED {
            // 用户在 UAC 弹窗点了「否」。
            Err("已取消提权".to_string())
        } else {
            Err(format!("无法发起提权（Windows 错误码 {code}）"))
        };
    }

    // 等待子进程结束（调用方在 spawn_blocking 内执行，不阻塞 UI 线程）。
    unsafe {
        WaitForSingleObject(info.hProcess, INFINITE);
        let mut exit_code: u32 = 0;
        let _ = GetExitCodeProcess(info.hProcess, &mut exit_code);
        let _ = CloseHandle(info.hProcess);
    }

    let text = std::fs::read_to_string(result_path)
        .map_err(|_| "提权操作未返回结果（可能被系统安全策略阻止）".to_string())?;
    serde_json::from_str::<FsOpResult>(&text).map_err(|e| format!("解析提权结果失败：{e}"))
}

/// 清扫 `%TEMP%\stelix-elevate` 下超过 `STALE_AFTER` 的残留文件（best-effort）。
///
/// 主进程若在 UAC 弹窗期间被杀，op / result 文件会留在临时目录。
pub fn sweep_stale_temp() {
    let Ok(entries) = std::fs::read_dir(temp_dir()) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let is_stale = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > STALE_AFTER);
        if is_stale {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::FsOp;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("stelix-elevate-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn arg_value_extracts_flag() {
        let args = vec![
            "app.exe".to_string(),
            FLAG_OP.to_string(),
            "C:\\tmp\\a.json".to_string(),
            FLAG_RESULT.to_string(),
            "C:\\tmp\\b.json".to_string(),
        ];
        assert_eq!(arg_value(&args, FLAG_OP).as_deref(), Some("C:\\tmp\\a.json"));
        assert_eq!(
            arg_value(&args, FLAG_RESULT).as_deref(),
            Some("C:\\tmp\\b.json")
        );
        assert_eq!(arg_value(&args, "--nope"), None);
        // flag 出现在末尾（无 value）时不应 panic。
        assert_eq!(arg_value(&["a".to_string(), FLAG_OP.to_string()], FLAG_OP), None);
    }

    #[test]
    fn helper_executes_op_and_writes_result() {
        let dir = temp_dir("run");
        let target = dir.join("made");
        let op_path = dir.join("op.json");
        let result_path = dir.join("result.json");
        let op = FsOp::Mkdir {
            targets: vec![target.to_string_lossy().to_string()],
        };
        std::fs::write(&op_path, serde_json::to_vec(&op).unwrap()).unwrap();

        assert!(run_helper(&op_path, &result_path));
        let result: FsOpResult =
            serde_json::from_str(&std::fs::read_to_string(&result_path).unwrap()).unwrap();
        assert!(result.ok, "{:?}", result.errors);
        assert!(target.is_dir());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn helper_reports_unreadable_op_file() {
        let dir = temp_dir("bad");
        let op_path = dir.join("missing.json");
        let result_path = dir.join("result.json");

        // 参数读不到时仍写出结果文件（ok=false），主进程据此报错。
        assert!(run_helper(&op_path, &result_path));
        let result: FsOpResult =
            serde_json::from_str(&std::fs::read_to_string(&result_path).unwrap()).unwrap();
        assert!(!result.ok);
        assert!(!result.errors.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

//! Stelix File Manager（星索）后端入口。
//!
//! ⚠️ 本文件（含 command 注册表）由 Phase 0 拥有并冻结。
//! 各 workstream 只实现自己模块目录下的逻辑，**不要修改 command 签名**。

mod ai_agent;
mod app_core;
mod clipboard;
mod credentials;
mod dir_size;
#[cfg(windows)]
mod drive_monitor;
mod elevate;
mod feedback;
mod fs_core;
mod index_engine;
mod network;
mod persistence;
mod schedule;
mod search_engine;
mod sftp;
mod shell_icon;
#[cfg(windows)]
mod shell_menu;
mod smb_discovery;
mod ssh;
mod state;
mod types;
mod watcher;
mod wsl;

use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, State, WindowEvent,
};

use state::AppState;
use types::*;

use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

// ---------------------------------------------------------------- 文件系统

#[tauri::command]
async fn list_dir(path: String) -> Result<DirListing, String> {
    tauri::async_runtime::spawn_blocking(move || fs_core::list_dir(&path))
        .await
        .map_err(|error| format!("目录读取任务失败: {error}"))?
}

#[tauri::command]
fn get_roots() -> Vec<DriveInfo> {
    fs_core::get_roots()
}

/// 列出 FTP / SFTP 目录，条目带类型 / 大小 / 修改时间。
///
/// SFTP 走内置 russh 实现（不依赖 curl 对主机密钥算法的支持），FTP 仍走 curl。
#[tauri::command]
async fn list_remote_dir(request: network::RemoteRequest) -> Result<DirListing, String> {
    if request.connection.protocol == "sftp" {
        return sftp::list_remote_dir(request).await;
    }
    tauri::async_runtime::spawn_blocking(move || network::list_remote_dir(request))
        .await
        .map_err(|error| format!("远程读取任务失败: {error}"))?
}

/// 在本地与 FTP / SFTP 之间上传或下载（支持目录递归）。
///
/// SFTP 在 russh 连接内传输；FTP 交给后台线程运行 curl。每开始一个顶层条目就发一次
/// `remote:transfer` 事件，供状态栏显示当前进度。
#[tauri::command]
async fn remote_transfer(
    app: tauri::AppHandle,
    request: network::RemoteTransferRequest,
) -> Result<network::RemoteTransferResult, String> {
    use tauri::Emitter;
    if request.connection.protocol == "sftp" {
        let mut emit = |progress| {
            let _ = app.emit(app_core::events::REMOTE_TRANSFER, progress);
        };
        return Ok(sftp::transfer_remote(request, &mut emit).await);
    }
    tauri::async_runtime::spawn_blocking(move || {
        network::transfer_remote(request, &mut |progress| {
            let _ = app.emit(app_core::events::REMOTE_TRANSFER, progress);
        })
    })
    .await
    .map_err(|error| format!("远程传输任务失败: {error}"))
}

#[tauri::command]
async fn discover_smb_neighbors() -> Result<Vec<smb_discovery::SmbNeighbor>, String> {
    tauri::async_runtime::spawn_blocking(smb_discovery::discover)
        .await
        .map_err(|error| format!("局域网扫描任务失败: {error}"))?
}

#[tauri::command]
async fn list_smb_shares(server: String) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || smb_discovery::list_shares(&server))
        .await
        .map_err(|error| format!("共享枚举任务失败: {error}"))?
}

#[tauri::command]
async fn list_wsl_distros() -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(wsl::list_distros)
        .await
        .map_err(|error| format!("WSL 枚举任务失败: {error}"))?
}

#[tauri::command]
async fn save_remote_password(host_id: String, password: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        credentials::save_remote_password(&host_id, &password)
    })
    .await
    .map_err(|error| format!("密码保存任务失败: {error}"))?
}

#[tauri::command]
async fn get_remote_password(host_id: String) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || credentials::get_remote_password(&host_id))
        .await
        .map_err(|error| format!("密码读取任务失败: {error}"))?
}

#[tauri::command]
async fn delete_remote_password(host_id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || credentials::delete_remote_password(&host_id))
        .await
        .map_err(|error| format!("密码删除任务失败: {error}"))?
}

#[tauri::command]
async fn save_remote_key_passphrase(host_id: String, passphrase: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        credentials::save_remote_key_passphrase(&host_id, &passphrase)
    })
    .await
    .map_err(|error| format!("私钥口令保存任务失败: {error}"))?
}

#[tauri::command]
async fn get_remote_key_passphrase(host_id: String) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || credentials::get_remote_key_passphrase(&host_id))
        .await
        .map_err(|error| format!("私钥口令读取任务失败: {error}"))?
}

#[tauri::command]
async fn delete_remote_key_passphrase(host_id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        credentials::delete_remote_key_passphrase(&host_id)
    })
    .await
    .map_err(|error| format!("私钥口令删除任务失败: {error}"))?
}

// ---------------------------------------------------------------- SSH 终端

#[tauri::command]
async fn ssh_connect(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    request: ssh::SshConnectRequest,
) -> Result<ssh::SshSessionInfo, String> {
    let manager = state.ssh.clone();
    manager.connect(app, request).await
}

#[tauri::command]
fn ssh_write(state: State<'_, AppState>, session_id: String, data: String) -> Result<(), String> {
    state.ssh.write(&session_id, data)
}

#[tauri::command]
fn ssh_resize(state: State<'_, AppState>, session_id: String, cols: u32, rows: u32) {
    state.ssh.resize(&session_id, cols, rows);
}

#[tauri::command]
fn ssh_close(state: State<'_, AppState>, session_id: String) {
    state.ssh.close(&session_id);
}

/// 执行一次文件操作（复制 / 移动 / 删除 / 重命名 / 新建）。
///
/// 递归复制、移动、删除都是重负载阻塞操作，走后台线程执行，避免卡死 UI。
#[tauri::command]
async fn fs_operation(op: FsOp) -> FsOpResult {
    tauri::async_runtime::spawn_blocking(move || fs_core::execute(&op))
        .await
        .unwrap_or_else(|error| FsOpResult {
            ok: false,
            errors: vec![format!("文件操作任务失败: {error}")],
            denied: Vec::new(),
        })
}

/// 提权执行一次因权限不足失败的文件操作（Windows UAC）。
///
/// 前端在 `fs_operation` 返回非空 `denied` 时调用：内部用 `ShellExecuteExW(runas)`
/// 拉起本程序的一个提权子进程完成操作，主程序始终保持普通权限。
/// `async` + `spawn_blocking`：等待提权子进程时不会阻塞 UI 线程。
#[tauri::command]
async fn fs_operation_elevated(op: FsOp) -> Result<FsOpResult, String> {
    tauri::async_runtime::spawn_blocking(move || elevate::run_elevated_fs_op(&op))
        .await
        .map_err(|error| format!("提权任务失败: {error}"))?
}

/// 取文件 / 文件夹的 Windows 系统图标，返回 `data:image/png;base64,...`。
///
/// 走后台线程：`SHGetFileInfoW` 属慢调用，且结果按图标种类缓存在 `AppState`。
/// 提取失败（非 Windows / 路径不可用）返回 `None`，前端回退到内置矢量图标。
#[tauri::command]
async fn get_file_icon(
    state: State<'_, AppState>,
    path: String,
    is_dir: bool,
    use_path_icon: bool,
) -> Result<Option<String>, String> {
    let cache = state.icons.clone();
    tauri::async_runtime::spawn_blocking(move || cache.get_or_load(&path, is_dir, use_path_icon))
        .await
        .map_err(|error| format!("图标提取任务失败: {error}"))
}

#[tauri::command]
fn set_file_clipboard(paths: Vec<String>, move_files: bool) -> Result<(), String> {
    clipboard::set_files(&paths, move_files)
}

#[tauri::command]
fn get_file_clipboard() -> Result<Option<FileClipboard>, String> {
    clipboard::get_files()
        .map(|value| value.map(|(paths, move_files)| FileClipboard { paths, move_files }))
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct FileClipboard {
    paths: Vec<String>,
    move_files: bool,
}

#[tauri::command]
fn path_is_file(path: String) -> bool {
    std::path::Path::new(&path).is_file()
}

/// 展开地址栏输入里的 Windows 环境变量（`%APPDATA%`）与主目录简写（`~`）。
#[tauri::command]
fn expand_path(path: String) -> String {
    fs_core::expand_path(&path)
}

/// 在系统配置的默认终端中打开指定文件夹。
#[tauri::command]
fn open_in_terminal(path: String) -> Result<(), String> {
    let directory = std::path::Path::new(&path);
    if !directory.is_dir() {
        return Err(format!("文件夹不存在或不可访问: {path}"));
    }

    #[cfg(windows)]
    {
        // 显式启动 Windows Terminal，避免 GUI 子进程创建控制台后立即退出。
        std::process::Command::new("wt.exe")
            .arg("-d")
            .arg(directory)
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("无法启动 Windows Terminal（wt.exe）: {error}"))
    }

    #[cfg(not(windows))]
    {
        let _ = directory;
        Err("当前平台暂不支持打开系统默认终端".to_string())
    }
}

/// 使用 Windows Shell 显示文件或文件夹的系统属性对话框。
#[tauri::command]
fn show_item_properties(path: String) -> Result<(), String> {
    let item = std::path::Path::new(&path);
    if !item.exists() {
        return Err(format!("文件或文件夹不存在: {path}"));
    }

    #[cfg(windows)]
    {
        use std::{mem::size_of, os::windows::ffi::OsStrExt};
        use windows_sys::Win32::UI::Shell::{
            ShellExecuteExW, SEE_MASK_INVOKEIDLIST, SHELLEXECUTEINFOW,
        };

        let verb: Vec<u16> = std::ffi::OsStr::new("properties")
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let file: Vec<u16> = item
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
        info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_INVOKEIDLIST;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.nShow = 1; // SW_SHOWNORMAL

        if unsafe { ShellExecuteExW(&mut info) } != 0 {
            Ok(())
        } else {
            Err(format!(
                "无法打开系统属性窗口（Windows 错误码 {}）",
                unsafe { windows_sys::Win32::Foundation::GetLastError() }
            ))
        }
    }

    #[cfg(not(windows))]
    {
        let _ = item;
        Err("当前平台不支持 Windows 属性窗口".to_string())
    }
}

/// 弹出 Windows 系统「打开方式」窗口，由系统列出已注册的应用与「选择其他应用」。
///
/// 只接受真实存在的本地文件（文件夹请走「在终端打开」等入口）。对话框是模态阻塞
/// 调用，放到后台线程执行，避免卡住 WebView IPC / 主线程。
#[tauri::command]
async fn show_open_with(path: String) -> Result<(), String> {
    let file = std::path::PathBuf::from(&path);
    if !file.is_file() {
        return Err(format!("文件不存在或不是普通文件: {path}"));
    }

    #[cfg(windows)]
    {
        tauri::async_runtime::spawn_blocking(move || show_open_with_windows(&file))
            .await
            .map_err(|error| format!("打开方式任务失败: {error}"))?
    }

    #[cfg(not(windows))]
    {
        let _ = file;
        Err("当前平台暂不支持系统「打开方式」窗口".to_string())
    }
}

/// `SHOpenWithDialog` 的 Windows 实现。
///
/// 用 Shell 官方 API 而不是自行枚举注册表：应用列表、图标、「始终使用此应用」
/// 选项都由系统维护，避免与应用商店 / 传统 Win32 应用的注册方式产生偏差。
#[cfg(windows)]
fn show_open_with_windows(file: &std::path::Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::Com::{
        CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED,
    };
    use windows_sys::Win32::UI::Shell::{
        SHOpenWithDialog, OAIF_ALLOW_REGISTRATION, OAIF_EXEC, OPENASINFO,
    };

    let file_wide: Vec<u16> = file
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let info = OPENASINFO {
        pcszFile: file_wide.as_ptr(),
        pcszClass: std::ptr::null(),
        oaifInFlags: OAIF_ALLOW_REGISTRATION | OAIF_EXEC,
    };

    // spawn_blocking 线程默认没有初始化 COM；RPC_E_CHANGED_MODE 表示线程已处于
    // 其他套间模型，此时不负责反初始化，仍继续弹窗。
    let init_hr = unsafe { CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) };
    let initialized_here = init_hr >= 0;
    let hr = unsafe { SHOpenWithDialog(std::ptr::null_mut(), &info) };
    if initialized_here {
        unsafe { CoUninitialize() };
    }

    // 用户取消不是错误：HRESULT_FROM_WIN32(ERROR_CANCELLED)。
    const CANCELLED: i32 = 0x8007_04C7u32 as i32;
    if hr >= 0 || hr == CANCELLED {
        Ok(())
    } else {
        Err(format!(
            "无法打开系统「打开方式」窗口（HRESULT 0x{:08X}）",
            hr as u32
        ))
    }
}

/// 弹出未筛选且不含 Stelix 专属项的完整 Windows Shell 菜单。
#[tauri::command]
fn show_native_shell_menu(
    window: tauri::Window,
    path: String,
    anchor: Option<ShellMenuAnchor>,
) -> Result<bool, String> {
    #[cfg(windows)]
    {
        let hwnd = window
            .hwnd()
            .map_err(|error| format!("无法获取窗口句柄: {error}"))?;
        shell_menu::show_native(
            hwnd,
            &path,
            anchor.map(|value| shell_menu::Anchor {
                x: value.x,
                y: value.y,
                scale: value.scale,
            }),
        )
    }

    #[cfg(not(windows))]
    {
        let _ = (window, path, anchor);
        Err("当前平台暂不支持系统右键菜单".to_string())
    }
}

/// “更多”按钮在 WebView 客户端坐标系中的锚点（CSS 像素 + DPI 缩放）。
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShellMenuAnchor {
    x: f64,
    y: f64,
    scale: f64,
}

// ---------------------------------------------------------------- 索引与检索

#[tauri::command]
fn start_index(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    roots: Vec<String>,
) -> Result<(), String> {
    persistence::save_index_roots(&roots)?;
    state.index.start_scan(app, roots);
    Ok(())
}

/// 仅保存备用内存索引的根目录，供 Everything 不可用时由用户手动启用。
#[tauri::command]
fn set_index_roots(state: State<'_, AppState>, roots: Vec<String>) -> Result<(), String> {
    persistence::save_index_roots(&roots)?;
    state.index.set_roots(roots);
    Ok(())
}

/// 重新等待本机 Everything IPC 就绪，不触发本地目录扫描。
#[tauri::command]
fn retry_everything_index(app: tauri::AppHandle, state: State<'_, AppState>) {
    #[cfg(windows)]
    {
        let roots = state.index.status().roots;
        std::sync::Arc::clone(&state.index).wait_for_everything(app, roots);
    }
    #[cfg(not(windows))]
    let _ = (app, state);
}

/// 默认索引根：用户主目录（其中的 AppData 等缓存目录由索引器的跳过列表排除）。
///
/// 不再默认索引整盘 —— 全盘索引会让条目数达到数百万，内存与扫描时间成倍上涨。
fn default_index_roots() -> Vec<String> {
    dirs::home_dir()
        .map(|home| vec![home.to_string_lossy().into_owned()])
        .unwrap_or_default()
}

#[tauri::command]
fn index_status(state: State<'_, AppState>) -> IndexStatus {
    state.index.status()
}

/// 在索引中检索文件。
///
/// 检索会遍历全量索引，标注 `async` 后由 Tauri 交给线程池执行，避免阻塞主线程。
#[tauri::command(async)]
fn search_files(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    query: SearchQuery,
) -> Vec<SearchHit> {
    // 集成点：搜索框里可以直接写语法，例如
    //   `ext:pdf size:>100mb modified:today dir: 报告`
    // parse_query 把语法 token 解析为结构化条件，剩余 token 作为关键词。
    // 前端显式传入的结构化字段优先（未被语法覆盖）。
    let parsed = search_engine::parse_query(&query.text);
    let merged = SearchQuery {
        text: parsed.text,
        scope: query.scope.or(parsed.scope),
        is_dir: query.is_dir.or(parsed.is_dir),
        ext: if query.ext.is_empty() {
            parsed.ext
        } else {
            query.ext
        },
        min_size: query.min_size.or(parsed.min_size),
        max_size: query.max_size.or(parsed.max_size),
        modified_after: query.modified_after.or(parsed.modified_after),
        limit: query.limit,
    };
    state.index.search_with_events(Some(&app), &merged)
}

// ---------------------------------------------------------------- AI

#[tauri::command]
async fn ai_chat(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    request_id: String,
    messages: Vec<AiMessage>,
    context: AiContext,
) -> Result<AiReply, String> {
    let ai = state.ai.clone();
    ai.chat(app, request_id, messages, context).await
}

#[tauri::command]
fn ai_chat_cancel(state: State<'_, AppState>, id: String) {
    state.ai.cancel(&id);
}

#[tauri::command]
async fn ai_list_models(provider: AiProvider) -> Result<Vec<AiModel>, String> {
    ai_agent::list_models(provider).await
}

#[tauri::command]
async fn ai_execute_tool(
    state: State<'_, AppState>,
    call: AiToolCall,
    approved: bool,
) -> Result<ToolOutcome, String> {
    let index = state.index.clone();
    tauri::async_runtime::spawn_blocking(move || ai_agent::execute_tool(&call, approved, &index))
        .await
        .map_err(|error| format!("AI 工具执行失败: {error}"))?
}

#[tauri::command]
fn ai_config_get(state: State<'_, AppState>) -> AiConfig {
    state.ai.config()
}

#[tauri::command]
fn ai_config_set(state: State<'_, AppState>, config: AiConfig) -> Result<(), String> {
    state.ai.set_config(config)
}

#[tauri::command]
async fn ai_test_connection(config: AiConfig) -> Result<String, String> {
    ai_agent::test_connection(config).await
}

#[tauri::command]
fn ai_conversations_list() -> Result<Vec<AiConversation>, String> {
    persistence::ai_history::list()
}

#[tauri::command]
fn ai_conversation_save(conversation: AiConversation) -> Result<(), String> {
    persistence::ai_history::save(&conversation)
}

#[tauri::command]
fn ai_conversation_delete(id: String) -> Result<(), String> {
    persistence::ai_history::delete(&id)
}

/// 用大模型把自然语言需求规划为一条定时任务草稿（离线时走规则解析兜底）。
#[tauri::command]
async fn ai_plan_task(
    state: State<'_, AppState>,
    prompt: String,
    context: AiContext,
) -> Result<TaskPlanReply, String> {
    let ai = state.ai.clone();
    Ok(ai.plan_task(prompt, context).await)
}

// ---------------------------------------------------------------- 定时任务

#[tauri::command]
fn task_list(state: State<'_, AppState>) -> Vec<ScheduledTask> {
    state.tasks.list()
}

#[tauri::command]
fn task_save(state: State<'_, AppState>, task: ScheduledTask) -> Result<ScheduledTask, String> {
    state.tasks.save(task)
}

#[tauri::command]
fn task_delete(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.tasks.delete(&id)
}

#[tauri::command]
fn task_set_enabled(state: State<'_, AppState>, id: String, enabled: bool) -> Result<(), String> {
    state.tasks.set_enabled(&id, enabled)
}

/// 立即运行一次任务，返回中文结果摘要。
///
/// 任务执行可能遍历整棵目录树，走后台线程避免阻塞 UI。
#[tauri::command]
async fn task_run_now(state: State<'_, AppState>, id: String) -> Result<String, String> {
    let tasks = state.tasks.clone();
    tauri::async_runtime::spawn_blocking(move || tasks.run_now(&id))
        .await
        .map_err(|error| format!("任务执行失败: {error}"))?
}

/// 预演（dry-run）：统计将命中的文件，不产生任何副作用。
///
/// 预演要遍历目录树，标注 `async` 后由 Tauri 交给线程池执行，避免阻塞主线程。
#[tauri::command(async)]
fn task_preview(state: State<'_, AppState>, action: TaskAction) -> TaskPreview {
    state.tasks.preview(&action)
}

// ---------------------------------------------------------------- 目录变更监听

/// 全量设置需要监听的目录集合（前端每次上报完整集合，幂等）。
///
/// 只接受本地盘符目录；网络 / 远程路径在 `watcher::WatchService` 内被过滤。
/// 变更经 250ms 防抖后以 `fs:changed` 事件上报。
#[tauri::command]
fn watch_dirs_set(state: State<'_, AppState>, paths: Vec<String>) {
    state.watcher.set_dirs(&paths);
}

// ---------------------------------------------------------------- 目录大小统计

/// 请求一批目录的递归内部大小（前端只上报详情视图当前可见的目录）。
///
/// 结果经 `fs:dir-size` 事件异步回填；已缓存且未过期的目录会立即回填。
#[tauri::command]
fn dir_size_request(app: tauri::AppHandle, state: State<'_, AppState>, paths: Vec<String>) {
    state.dir_sizes.request(&app, paths);
}

/// 使一批目录的大小缓存失效（目录内容变化时调用），下次请求会重新遍历。
#[tauri::command]
fn dir_size_invalidate(state: State<'_, AppState>, paths: Vec<String>) {
    state.dir_sizes.invalidate(&paths);
}

// ---------------------------------------------------------------- 意见反馈

/// 检查本机 GitHub CLI 是否可用并已登录（供反馈表单提示）。
#[tauri::command]
async fn feedback_auth_status() -> Result<FeedbackAuth, String> {
    tauri::async_runtime::spawn_blocking(feedback::auth_status)
        .await
        .map_err(|error| format!("反馈状态检查任务失败: {error}"))
}

/// 通过本机 GitHub CLI 把反馈提交为 GitHub issue，返回 issue 编号与链接。
#[tauri::command]
async fn feedback_submit(request: FeedbackRequest) -> Result<FeedbackCreated, String> {
    tauri::async_runtime::spawn_blocking(move || feedback::submit(&request))
        .await
        .map_err(|error| format!("反馈提交任务失败: {error}"))?
}

// ---------------------------------------------------------------- 开机自启动

/// 开机自启动（Windows 走 `HKCU\...\Run` 注册表项，由 tauri-plugin-autostart 管理）。
#[tauri::command]
fn autostart_get(app: tauri::AppHandle) -> Result<bool, String> {
    app.autolaunch().is_enabled().map_err(|e| e.to_string())
}

#[tauri::command]
fn autostart_set(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    let manager = app.autolaunch();
    if enabled {
        manager.enable()
    } else {
        manager.disable()
    }
    .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------- 系统托盘

/// 主窗口标签，与 tauri.conf.json 中窗口配置对应。
const MAIN_WINDOW_LABEL: &str = "main";

/// 显示并聚焦主窗口（从托盘恢复）。
fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// 切换主窗口显示 / 隐藏（托盘左键单击）。
fn toggle_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        if window.is_visible().unwrap_or(false) {
            let _ = window.hide();
        } else {
            let _ = window.show();
            let _ = window.unminimize();
            let _ = window.set_focus();
        }
    }
}

/// 构建系统托盘图标与菜单。
///
/// 交互约定：
/// - 左键单击：切换主窗口显示 / 隐藏。
/// - 右键菜单：「显示主窗口」恢复窗口，「退出」真正结束进程。
///
/// 图标复用应用默认窗口图标（bundle 的 `icon.ico`），无需额外资源。
fn setup_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    let show_item = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show_item, &quit_item])?;

    let mut builder = TrayIconBuilder::with_id("main-tray")
        .tooltip("Stelix File Manager 星索")
        .menu(&menu)
        // 菜单只在右键弹出，左键留给「切换窗口」
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main_window(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                toggle_main_window(tray.app_handle());
            }
        });

    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }

    builder.build(app)?;
    Ok(())
}

// ---------------------------------------------------------------- 入口

/// 提权子进程入口：命令行含提权开关时执行一次文件操作并写结果，返回退出码。
///
/// 返回 `None` 表示普通启动，应继续走 [`run`]。**必须在 `run()` 之前调用**，
/// 否则提权子进程会创建单实例互斥体 / 窗口 / WebView2，与主实例冲突。
pub fn try_run_elevated_helper() -> Option<i32> {
    elevate::try_run_elevated_helper()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let context = tauri::generate_context!();
    // 数据 / 配置目录按 bundle identifier 派生（如 `%LOCALAPPDATA%\com.stelix.desktop`），
    // 与 NSIS 安装目录（`%LOCALAPPDATA%\Stelix`）区分开。必须在 `AppState::new()`
    // （会读取 AI 配置）之前初始化；dev 版 identifier 不同 → 数据天然隔离。
    persistence::init_dirs(&context.config().identifier);

    tauri::Builder::default()
        // single-instance 必须最先注册：在其余插件与 setup 之前拦截第二个实例，
        // 并让回调在「已运行的那个实例」里执行（Windows 上它会先交接前台权，
        // 使 set_focus 生效）。重复启动时把隐藏于托盘的窗口重新唤到前台。
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            show_main_window(app);
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_drag::init())
        // 开机自启动：注册时带上 `--autostart` 参数，供下方 setup 判断启动来源。
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec!["--autostart"]),
        ))
        .manage(AppState::new())
        .setup(|app| {
            #[cfg(windows)]
            drive_monitor::start(app.handle().clone());
            setup_tray(app.handle())?;
            // 清扫提权临时目录里上次遗留的残留文件（进程被杀时可能留下）。
            elevate::sweep_stale_temp();
            // 窗口以 visible:false 创建：先切到最大化态再显示，
            // 避免出现「先默认尺寸、后最大化」的可见跳变。
            // 由开机自启拉起时（带 --autostart）不显示窗口，静默驻留托盘。
            let launched_by_autostart = std::env::args().any(|arg| arg == "--autostart");
            if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
                if !launched_by_autostart {
                    let _ = window.maximize();
                    let _ = window.show();
                }
            }
            // Windows 上等待 Everything 完成启动，期间不扫描目录；本地内存索引
            // 仅在用户主动启用时建立。其他平台沿用原有后台内存索引。
            {
                let roots = persistence::load_index_roots().unwrap_or_else(default_index_roots);
                persistence::remove_index_cache_files();
                let index = std::sync::Arc::clone(&app.state::<AppState>().index);
                #[cfg(windows)]
                index.wait_for_everything(app.handle().clone(), roots);
                #[cfg(not(windows))]
                {
                    let handle = app.handle().clone();
                    std::thread::spawn(move || index.start_scan(handle, roots));
                }
            }
            // 定时任务调度线程：每 20s 检查一次到期任务，跨平台。
            let tasks = std::sync::Arc::clone(&app.state::<AppState>().tasks);
            tasks.start_loop(app.handle().clone());
            // 目录变更监听：把 notify 事件防抖后以 `fs:changed` 上报给前端。
            let watcher = std::sync::Arc::clone(&app.state::<AppState>().watcher);
            watcher.start_flush_loop(app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            // 关闭窗口默认最小化到系统托盘：阻止退出，改为隐藏窗口。
            // 真正退出走托盘菜单的「退出」（app.exit(0)），不走这里。
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            list_dir,
            get_roots,
            list_remote_dir,
            remote_transfer,
            discover_smb_neighbors,
            list_smb_shares,
            list_wsl_distros,
            save_remote_password,
            get_remote_password,
            delete_remote_password,
            save_remote_key_passphrase,
            get_remote_key_passphrase,
            delete_remote_key_passphrase,
            ssh_connect,
            ssh_write,
            ssh_resize,
            ssh_close,
            fs_operation,
            fs_operation_elevated,
            get_file_icon,
            set_file_clipboard,
            get_file_clipboard,
            path_is_file,
            expand_path,
            open_in_terminal,
            show_item_properties,
            show_open_with,
            show_native_shell_menu,
            start_index,
            set_index_roots,
            retry_everything_index,
            index_status,
            search_files,
            ai_chat,
            ai_chat_cancel,
            ai_list_models,
            ai_execute_tool,
            ai_config_get,
            ai_config_set,
            ai_test_connection,
            ai_conversations_list,
            ai_conversation_save,
            ai_conversation_delete,
            ai_plan_task,
            task_list,
            task_save,
            task_delete,
            task_set_enabled,
            task_run_now,
            task_preview,
            watch_dirs_set,
            dir_size_request,
            dir_size_invalidate,
            feedback_auth_status,
            feedback_submit,
            autostart_get,
            autostart_set,
        ])
        .run(context)
        .expect("error while running tauri application");
}

//! Stelix File Manager（星索）后端入口。
//!
//! ⚠️ 本文件（含 command 注册表）由 Phase 0 拥有并冻结。
//! 各 workstream 只实现自己模块目录下的逻辑，**不要修改 command 签名**。

mod ai_agent;
mod app_core;
mod clipboard;
mod credentials;
mod fs_core;
mod index_engine;
mod network;
mod persistence;
mod search_engine;
mod shell_icon;
mod smb_discovery;
mod state;
mod types;
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

#[tauri::command]
async fn list_remote(request: network::RemoteRequest) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || network::list_remote(request))
        .await
        .map_err(|error| format!("远程读取任务失败: {error}"))?
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
fn fs_operation(op: FsOp) -> FsOpResult {
    fs_core::execute(&op)
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
) -> Result<Option<String>, String> {
    let cache = state.icons.clone();
    tauri::async_runtime::spawn_blocking(move || cache.get_or_load(&path, is_dir))
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

// ---------------------------------------------------------------- 索引与检索

#[tauri::command]
fn start_index(app: tauri::AppHandle, state: State<'_, AppState>, roots: Vec<String>) {
    state.index.start_scan(app, roots);
}

#[tauri::command]
fn index_status(state: State<'_, AppState>) -> IndexStatus {
    state.index.status()
}

#[tauri::command]
fn search_files(state: State<'_, AppState>, query: SearchQuery) -> Vec<SearchHit> {
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
    state.index.search(&merged)
}

// ---------------------------------------------------------------- AI

#[tauri::command]
async fn ai_chat(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    messages: Vec<AiMessage>,
    context: AiContext,
) -> Result<AiReply, String> {
    let ai = state.ai.clone();
    ai.chat(app, messages, context).await
}

#[tauri::command]
fn ai_execute_tool(
    state: State<'_, AppState>,
    call: AiToolCall,
    approved: bool,
) -> Result<ToolOutcome, String> {
    ai_agent::execute_tool(&call, approved, &state.index)
}

#[tauri::command]
fn ai_config_get(state: State<'_, AppState>) -> AiConfig {
    state.ai.config()
}

#[tauri::command]
fn ai_config_set(state: State<'_, AppState>, config: AiConfig) {
    state.ai.set_config(config);
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // single-instance 必须最先注册：在其余插件与 setup 之前拦截第二个实例，
        // 并让回调在「已运行的那个实例」里执行（Windows 上它会先交接前台权，
        // 使 set_focus 生效）。重复启动时把隐藏于托盘的窗口重新唤到前台。
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            show_main_window(app);
        }))
        .plugin(tauri_plugin_opener::init())
        // 开机自启动：注册时带上 `--autostart` 参数，供下方 setup 判断启动来源。
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec!["--autostart"]),
        ))
        .manage(AppState::new())
        .setup(|app| {
            setup_tray(app.handle())?;
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
            // 全局搜索使用内存索引；索引服务会优先恢复本地缓存并重放 NTFS 变更日志，
            // 缓存不可用时再扫描本机卷。非 NTFS 卷由目录遍历数据源处理。
            #[cfg(windows)]
            {
                let roots = fs_core::get_roots()
                    .into_iter()
                    .map(|drive| drive.mount)
                    .collect();
                let index = std::sync::Arc::clone(&app.state::<AppState>().index);
                let handle = app.handle().clone();
                std::thread::spawn(move || index.start_service(handle, roots));
            }
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
            list_remote,
            discover_smb_neighbors,
            list_smb_shares,
            list_wsl_distros,
            save_remote_password,
            get_remote_password,
            delete_remote_password,
            fs_operation,
            get_file_icon,
            set_file_clipboard,
            get_file_clipboard,
            path_is_file,
            open_in_terminal,
            show_item_properties,
            start_index,
            index_status,
            search_files,
            ai_chat,
            ai_execute_tool,
            ai_config_get,
            ai_config_set,
            autostart_get,
            autostart_set,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

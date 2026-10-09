//! File Povit 后端入口。
//!
//! ⚠️ 本文件（含 command 注册表）由 Phase 0 拥有并冻结。
//! 各 workstream 只实现自己模块目录下的逻辑，**不要修改 command 签名**。

mod ai_agent;
mod app_core;
mod fs_core;
mod index_engine;
mod persistence;
mod search_engine;
mod state;
mod types;

use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, State, WindowEvent,
};

use state::AppState;
use types::*;

// ---------------------------------------------------------------- 文件系统

#[tauri::command]
fn list_dir(path: String) -> Result<DirListing, String> {
    fs_core::list_dir(&path)
}

#[tauri::command]
fn get_roots() -> Vec<DriveInfo> {
    fs_core::get_roots()
}

#[tauri::command]
fn fs_operation(op: FsOp) -> FsOpResult {
    fs_core::execute(&op)
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
        .tooltip("File Povit")
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
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::new())
        .setup(|app| {
            setup_tray(app.handle())?;
            // 窗口以 visible:false 创建：先切到最大化态再显示，
            // 避免出现「先默认尺寸、后最大化」的可见跳变。
            if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
                let _ = window.maximize();
                let _ = window.show();
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
            fs_operation,
            start_index,
            index_status,
            search_files,
            ai_chat,
            ai_execute_tool,
            ai_config_get,
            ai_config_set,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

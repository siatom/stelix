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

use tauri::State;

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

// ---------------------------------------------------------------- 入口

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::new())
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

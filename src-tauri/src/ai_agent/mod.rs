//! AI 对话与工具编排 —— 由 **WS-B (feat/fs-ai)** 实现。
//!
//! 契约（Phase 0 冻结）：
//! - `AiService::new / config / set_config / chat`
//! - 自由函数 `execute_tool(call, approved, index)` 供 command 层调用
//! - 无 API key 时必须走离线规则解析器，保证演示不依赖外部服务
//! - 写操作（Move/Delete/Rename/Mkdir）必须经 `AiReply::pending_confirm` 二次确认
//!
//! 本模块实现两条路径：
//! 1. **真实 LLM**（OpenAI 兼容 `/chat/completions`，非流式）—— 仅当
//!    `config.enabled && !api_key.is_empty()`；只读工具就地执行并回灌，
//!    写工具**不执行**，转成 `pending_confirm` 交给前端确认。
//! 2. **离线规则解析器** —— 无 key / 关闭时启用，用关键词识别中英文指令。

use std::fs;
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use parking_lot::RwLock;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::fs_core;
use crate::index_engine::IndexStore;
use crate::persistence;
use crate::state::AppState;
use crate::types::{
    AiConfig, AiContext, AiMessage, AiReply, AiToolCall, FileEntry, FsOp, FsOpKind, SearchHit,
    SearchQuery, TaskAction, TaskDraft, TaskFilters, TaskPlanReply, TaskSchedule, ToolOutcome,
};

pub struct AiService {
    config: RwLock<AiConfig>,
}

impl AiService {
    pub fn new() -> Self {
        Self {
            config: RwLock::new(persistence::load_ai_config()),
        }
    }

    pub fn config(&self) -> AiConfig {
        self.config.read().clone()
    }

    pub fn set_config(&self, cfg: AiConfig) {
        *self.config.write() = cfg.clone();
        let _ = persistence::save_ai_config(&cfg);
    }

    /// 发起一轮对话。
    ///
    /// - 若 `config.enabled` 且有 `api_key` → 走 OpenAI 兼容接口；
    ///   若请求失败则**自动回退**到离线解析器，并在回复头部标注错误原因。
    /// - 否则 → 离线规则解析器。
    pub async fn chat(
        &self,
        app: AppHandle,
        messages: Vec<AiMessage>,
        context: AiContext,
    ) -> Result<AiReply, String> {
        let cfg = self.config();
        // 索引由 WS-A 提供；这里借用 AppState 取出句柄（只在函数内使用）。
        let index = app.state::<AppState>().index.clone();

        if cfg.enabled && !cfg.api_key.trim().is_empty() {
            match chat_llm(&cfg, &messages, &context, &index).await {
                Ok(reply) => return Ok(reply),
                Err(e) => {
                    let mut r = offline_reply(&messages, &context, &index);
                    r.text = format!("⚠️ LLM 调用失败，已回退到离线模式：{e}\n\n{}", r.text);
                    return Ok(r);
                }
            }
        }
        Ok(offline_reply(&messages, &context, &index))
    }
}

impl Default for AiService {
    fn default() -> Self {
        Self::new()
    }
}

// ================================================================ 真实 LLM 路径

/// 写工具集合（需要二次确认，绝不直接执行）。
fn is_write_tool(name: &str) -> bool {
    matches!(
        name,
        "move_files" | "delete_files" | "rename_file" | "mkdir"
    )
}

/// 注入给模型的 system prompt。
fn system_prompt(context: &AiContext) -> String {
    let active = context
        .active_path
        .clone()
        .unwrap_or_else(|| "（未知）".to_string());
    let selected = if context.selected.is_empty() {
        "（无）".to_string()
    } else {
        context.selected.join("、")
    };
    format!(
        "你是 Stelix File Manager（星索）的文件管理器智能助手，运行在 Windows 上。\n\
         当前活动目录：{active}\n\
         当前选中项：{selected}\n\
         可用工具：\n\
         - 只读（可直接调用）：search_files、list_dir、get_metadata\n\
         - 写操作（调用后系统会先请用户确认，不会立即执行）：move_files、delete_files、rename_file、mkdir\n\
         规则：\n\
         1. 涉及写操作时，先用一句话说明将要做什么，再调用对应工具；不要声称已经执行。\n\
         2. 只读工具返回结果后，用简洁的中文总结，并列出关键文件（名称 + 大小 + 路径，最多 20 条）。\n\
         3. 用户用中文提问时用中文回答。\n\
         4. 路径一律使用绝对路径；未指定范围时以“当前活动目录”为搜索范围。"
    )
}

/// OpenAI 兼容的工具（function calling）JSON Schema。
fn tools_schema() -> Value {
    json!([
        {"type":"function","function":{
            "name":"search_files",
            "description":"在指定目录范围内按关键词/扩展名/大小/时间检索文件或目录（只读）",
            "parameters":{"type":"object","properties":{
                "query":{"type":"string","description":"文件名关键词，可为空串"},
                "scope":{"type":"string","description":"搜索根目录绝对路径，默认当前活动目录"},
                "ext":{"type":"array","items":{"type":"string"},"description":"扩展名白名单，小写且不含点，如 [\"pdf\"]"},
                "is_dir":{"type":"boolean","description":"true 只返回目录，false 只返回文件"},
                "min_size":{"type":"integer","description":"最小字节数"},
                "modified_after":{"type":"integer","description":"仅返回该 Unix 毫秒时间之后修改的条目"},
                "limit":{"type":"integer","description":"返回条数上限"}
            },"required":["query"]}
        }},
        {"type":"function","function":{
            "name":"list_dir",
            "description":"列出某个目录的直接子项（只读）",
            "parameters":{"type":"object","properties":{
                "path":{"type":"string","description":"目录绝对路径"}
            },"required":["path"]}
        }},
        {"type":"function","function":{
            "name":"get_metadata",
            "description":"获取若干路径的类型、大小与修改时间（只读）",
            "parameters":{"type":"object","properties":{
                "paths":{"type":"array","items":{"type":"string"}}
            },"required":["paths"]}
        }},
        {"type":"function","function":{
            "name":"move_files",
            "description":"把若干文件/目录移动到目标目录（写操作，需用户确认）",
            "parameters":{"type":"object","properties":{
                "sources":{"type":"array","items":{"type":"string"}},
                "target":{"type":"string","description":"目标目录绝对路径"}
            },"required":["sources","target"]}
        }},
        {"type":"function","function":{
            "name":"delete_files",
            "description":"删除若干文件/目录（写操作，需用户确认）",
            "parameters":{"type":"object","properties":{
                "paths":{"type":"array","items":{"type":"string"}}
            },"required":["paths"]}
        }},
        {"type":"function","function":{
            "name":"rename_file",
            "description":"重命名单个文件/目录（写操作，需用户确认）",
            "parameters":{"type":"object","properties":{
                "path":{"type":"string","description":"原路径绝对路径"},
                "new_name":{"type":"string","description":"新的名称（不含路径）"}
            },"required":["path","new_name"]}
        }},
        {"type":"function","function":{
            "name":"mkdir",
            "description":"新建目录（写操作，需用户确认）",
            "parameters":{"type":"object","properties":{
                "path":{"type":"string","description":"新目录绝对路径"}
            },"required":["path"]}
        }}
    ])
}

/// 调 OpenAI 兼容接口完成一轮（或多轮）工具编排，返回自然语言总结或待确认写操作。
async fn chat_llm(
    cfg: &AiConfig,
    messages: &[AiMessage],
    context: &AiContext,
    index: &IndexStore,
) -> Result<AiReply, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| format!("初始化 HTTP 客户端失败：{e}"))?;
    let url = format!("{}/chat/completions", cfg.base_url.trim_end_matches('/'));

    // 注入 system prompt，跳过历史里的 system，避免重复。
    let mut api_messages: Vec<Value> = Vec::new();
    api_messages.push(json!({"role": "system", "content": system_prompt(context)}));
    for m in messages {
        if m.role == "system" {
            continue;
        }
        api_messages.push(json!({"role": m.role, "content": m.content}));
    }

    let tools = tools_schema();
    let mut last_text = String::new();

    // 最多 3 轮，防止模型反复调用工具导致死循环。
    for _round in 0..3 {
        let body = json!({
            "model": cfg.model,
            "messages": api_messages,
            "tools": tools,
            "tool_choice": "auto",
            "stream": false
        });
        let resp = client
            .post(&url)
            .bearer_auth(cfg.api_key.trim())
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("请求 LLM 失败：{e}"))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            let snippet: String = text.chars().take(500).collect();
            return Err(format!("LLM 返回错误 {status}：{snippet}"));
        }
        let data: Value = resp
            .json()
            .await
            .map_err(|e| format!("解析 LLM 响应失败：{e}"))?;
        let msg = data
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .cloned()
            .ok_or_else(|| "LLM 响应缺少 choices[0].message".to_string())?;

        let content = msg
            .get("content")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();
        if !content.is_empty() {
            last_text = content.clone();
        }

        let tool_calls = msg
            .get("tool_calls")
            .and_then(|t| t.as_array())
            .cloned()
            .unwrap_or_default();
        if tool_calls.is_empty() {
            return Ok(AiReply {
                text: if content.is_empty() {
                    last_text
                } else {
                    content
                },
                tool_calls: Vec::new(),
                pending_confirm: None,
            });
        }

        // 把 assistant 的 tool_calls 消息回灌，随后逐条追加 tool 结果。
        api_messages.push(msg.clone());

        let mut pending: Option<FsOp> = None;
        let mut pending_desc = String::new();
        let mut parsed_calls: Vec<AiToolCall> = Vec::new();

        for tc in &tool_calls {
            let name = tc
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string();
            let id = tc
                .get("id")
                .and_then(|i| i.as_str())
                .unwrap_or("")
                .to_string();
            let args_str = tc
                .get("function")
                .and_then(|f| f.get("arguments"))
                .and_then(|a| a.as_str())
                .unwrap_or("{}");
            let args: Value = serde_json::from_str(args_str).unwrap_or_else(|_| json!({}));
            let call = AiToolCall {
                id: id.clone(),
                name: name.clone(),
                args,
            };
            parsed_calls.push(call.clone());

            if is_write_tool(&name) {
                match build_fs_op(&call) {
                    Ok((op, desc)) => {
                        // 写操作：不执行，构造 pending_confirm。
                        pending = Some(op);
                        pending_desc = desc;
                    }
                    Err(e) => {
                        api_messages.push(json!({
                            "role": "tool", "tool_call_id": id,
                            "content": format!("构造操作失败：{e}")
                        }));
                    }
                }
            } else {
                // 只读工具：就地执行并回灌结果。
                let result = match execute_tool(&call, true, index) {
                    Ok(o) => outcome_to_text(&o),
                    Err(e) => format!("工具执行失败：{e}"),
                };
                api_messages.push(json!({
                    "role": "tool", "tool_call_id": id, "content": result
                }));
            }
        }

        if let Some(op) = pending {
            let head = if last_text.is_empty() {
                "好的，我来处理。".to_string()
            } else {
                last_text.clone()
            };
            let text = format!("{head}\n\n我将执行以下操作，请确认后执行：\n{pending_desc}");
            return Ok(AiReply {
                text,
                tool_calls: parsed_calls,
                pending_confirm: Some(op),
            });
        }
        // 只有只读工具 → 继续下一轮，让模型基于结果生成总结。
    }

    Ok(AiReply {
        text: if last_text.is_empty() {
            "（已达到工具调用轮次上限，请换个说法再试）".to_string()
        } else {
            last_text
        },
        tool_calls: Vec::new(),
        pending_confirm: None,
    })
}

// ================================================================ execute_tool

/// 执行一次工具调用。
///
/// - 只读工具（search_files / list_dir / get_metadata）直接执行；
/// - 写工具（move/delete/rename/mkdir）必须 `approved == true`，否则返回 `Err("需要用户确认")`。
pub fn execute_tool(
    call: &AiToolCall,
    approved: bool,
    index: &IndexStore,
) -> Result<ToolOutcome, String> {
    match call.name.as_str() {
        "search_files" => {
            let a = &call.args;
            let query = SearchQuery {
                text: a
                    .get("query")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                scope: a
                    .get("scope")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string()),
                is_dir: a.get("is_dir").and_then(|v| v.as_bool()),
                ext: str_array(a, "ext"),
                min_size: a.get("min_size").and_then(|v| v.as_u64()),
                max_size: None,
                modified_after: a.get("modified_after").and_then(|v| v.as_i64()),
                limit: a
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .map(|n| n as usize)
                    .unwrap_or(200),
            };
            Ok(ToolOutcome::Search(search_with_fallback(index, &query)))
        }
        "list_dir" => {
            let path = call.args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            if path.trim().is_empty() {
                return Err("list_dir 缺少 path".into());
            }
            Ok(ToolOutcome::Listing(fs_core::list_dir(path)?))
        }
        "get_metadata" => {
            let paths = str_array(&call.args, "paths");
            if paths.is_empty() {
                return Err("get_metadata 缺少 paths".into());
            }
            let mut out = String::new();
            for p in &paths {
                match fs::metadata(p) {
                    Ok(m) => {
                        let kind = if m.is_dir() { "目录" } else { "文件" };
                        let size = if m.is_dir() { 0 } else { m.len() };
                        let modified = m
                            .modified()
                            .ok()
                            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                            .map(|d| d.as_millis() as i64)
                            .unwrap_or(0);
                        out.push_str(&format!(
                            "- {p}：{kind}，{}，修改于 {}\n",
                            human_size(size),
                            fmt_time(modified)
                        ));
                    }
                    Err(e) => out.push_str(&format!("- {p}：读取失败 {e}\n")),
                }
            }
            Ok(ToolOutcome::Text(out))
        }
        "move_files" | "delete_files" | "rename_file" | "mkdir" => {
            if !approved {
                return Err("需要用户确认".into());
            }
            let (op, _desc) = build_fs_op(call)?;
            Ok(ToolOutcome::Fs(fs_core::execute(&op)))
        }
        other => Err(format!("未知工具：{other}")),
    }
}

/// 把写工具调用转换为 `FsOp`（同时给出中文描述用于确认文案）。
fn build_fs_op(call: &AiToolCall) -> Result<(FsOp, String), String> {
    let a = &call.args;
    match call.name.as_str() {
        "move_files" => {
            let sources = str_array(a, "sources");
            let target = a
                .get("target")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if sources.is_empty() || target.is_empty() {
                return Err("move_files 缺少 sources 或 target".into());
            }
            let desc = format!("移动 {} 项到 {target}", sources.len());
            Ok((
                FsOp {
                    op: FsOpKind::Move,
                    sources,
                    target: Some(target),
                },
                desc,
            ))
        }
        "delete_files" => {
            let paths = str_array(a, "paths");
            if paths.is_empty() {
                return Err("delete_files 缺少 paths".into());
            }
            let desc = format!("删除 {} 项：{}", paths.len(), paths.join("、"));
            Ok((
                FsOp {
                    op: FsOpKind::Delete,
                    sources: paths,
                    target: None,
                },
                desc,
            ))
        }
        "rename_file" => {
            let path = a
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let new_name = a
                .get("new_name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if path.is_empty() || new_name.is_empty() {
                return Err("rename_file 缺少 path 或 new_name".into());
            }
            let parent = Path::new(&path).parent().map(|p| p.to_path_buf());
            let target = match parent {
                Some(p) => p.join(&new_name).to_string_lossy().to_string(),
                None => new_name.clone(),
            };
            let desc = format!("把 {path} 重命名为 {new_name}");
            Ok((
                FsOp {
                    op: FsOpKind::Rename,
                    sources: vec![path],
                    target: Some(target),
                },
                desc,
            ))
        }
        "mkdir" => {
            let path = a
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if path.is_empty() {
                return Err("mkdir 缺少 path".into());
            }
            let desc = format!("新建目录 {path}");
            Ok((
                FsOp {
                    op: FsOpKind::Mkdir,
                    sources: vec![],
                    target: Some(path),
                },
                desc,
            ))
        }
        other => Err(format!("未知写工具：{other}")),
    }
}

/// 把 `ToolOutcome` 转成回灌给模型的文本。
fn outcome_to_text(o: &ToolOutcome) -> String {
    match o {
        ToolOutcome::Search(hits) => {
            if hits.is_empty() {
                return "没有找到匹配项".to_string();
            }
            let mut s = format!("找到 {} 项：\n", hits.len());
            for h in hits.iter().take(20) {
                s.push_str(&format!(
                    "- {} ({}) {}\n",
                    h.entry.name,
                    human_size(h.entry.size),
                    h.entry.path
                ));
            }
            s
        }
        ToolOutcome::Listing(l) => {
            let mut s = format!("目录 {} 共 {} 项：\n", l.path, l.entries.len());
            for e in l.entries.iter().take(50) {
                if e.is_dir {
                    s.push_str(&format!("- [目录] {}\n", e.name));
                } else {
                    s.push_str(&format!("- {} ({})\n", e.name, human_size(e.size)));
                }
            }
            s
        }
        ToolOutcome::Fs(r) => {
            if r.ok {
                "操作成功".to_string()
            } else {
                format!("操作失败：{}", r.errors.join("；"))
            }
        }
        ToolOutcome::Text(t) => t.clone(),
    }
}

// ================================================================ 离线规则解析器

/// 离线模式入口：根据最后一条用户消息识别意图并直接产出回复 / 待确认操作。
fn offline_reply(messages: &[AiMessage], context: &AiContext, index: &IndexStore) -> AiReply {
    let input = messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .map(|m| m.content.trim().to_string())
        .unwrap_or_default();
    if input.is_empty() {
        return AiReply {
            text: "请告诉我你想做什么，例如：\n\
                   · 找出最大的 10 个文件\n\
                   · 所有图片 / .pdf 文件\n\
                   · 最近 7 天修改的\n\
                   · 有多少个文件\n\
                   · 删除 a.txt / 把 a.txt 移动到 D:\\归档 / 新建文件夹 报告"
                .to_string(),
            ..Default::default()
        };
    }
    let lower = input.to_lowercase();

    // 1) 写操作优先（删除 / 移动 / 复制 / 重命名 / 新建）
    if let Some(r) = try_write_ops(&input, &lower, context, index) {
        return r;
    }

    // 2) 统计 / 计数
    if contains_any(&lower, &["多少", "统计", "数量", "count", "how many"]) {
        if let Some(exts) = detect_exts(&lower) {
            return count_ext_reply(context, index, &exts);
        }
        return stats_reply(context, index);
    }

    // 3) 最大的 N 个
    if contains_any(
        &lower,
        &["最大", "largest", "biggest", "前几大", "占用最大"],
    ) {
        let n = extract_number(&lower).unwrap_or(10).clamp(1, 100);
        return largest_reply(context, index, n);
    }

    // 4) 按扩展名 / 类别
    if let Some(exts) = detect_exts(&lower) {
        return ext_reply(context, index, &exts);
    }

    // 5) 按修改时间
    if contains_any(
        &lower,
        &[
            "最近修改",
            "最近更改",
            "最近更新",
            "modified",
            "recent",
            "天前",
            "天内",
            "本周",
            "today",
            "今天",
            "最新",
        ],
    ) {
        let days = extract_number(&lower).unwrap_or(7).clamp(1, 3650) as i64;
        return modified_reply(context, index, days);
    }

    // 6) 兜底：把用户输入当作关键词检索
    keyword_reply(context, index, &input)
}

/// 识别并生成写操作的 `pending_confirm`。命中写意图即返回 `Some`。
fn try_write_ops(
    input: &str,
    lower: &str,
    context: &AiContext,
    index: &IndexStore,
) -> Option<AiReply> {
    // ---- 新建目录 ----
    if contains_any(
        lower,
        &["新建文件夹", "新建目录", "创建文件夹", "创建目录", "mkdir"],
    ) {
        if let Some(rest) = strip_any(
            input,
            &["新建文件夹", "新建目录", "创建文件夹", "创建目录", "mkdir"],
        ) {
            let name = clean_token(rest);
            if !name.is_empty() {
                let path = resolve_path(&name, context);
                return Some(confirm_reply(
                    format!("将新建目录：{path}"),
                    FsOp {
                        op: FsOpKind::Mkdir,
                        sources: vec![],
                        target: Some(path),
                    },
                ));
            }
        }
        return Some(AiReply {
            text: "请告诉我要新建的目录名称，例如：新建文件夹 报告".to_string(),
            ..Default::default()
        });
    }

    // ---- 重命名（中文：把 X 重命名为 Y / 英文：rename X to Y）----
    if contains_any(lower, &["重命名为", "改名为", "重命名成"]) {
        if let Some((left, right)) = split_on(input, &["重命名为", "改名为", "重命名成"])
        {
            let src = clean_token(strip_prefix_words(left));
            let new_name = clean_token(right);
            if !src.is_empty() && !new_name.is_empty() {
                let full = resolve_path(&src, context);
                if let Some(op) = rename_op(&full, &new_name) {
                    return Some(confirm_reply(format!("把 {full} 重命名为 {new_name}"), op));
                }
            }
        }
        return Some(AiReply {
            text: "请用「把 旧文件名 重命名为 新文件名」的格式告诉我。".to_string(),
            ..Default::default()
        });
    }
    if lower.starts_with("rename ") {
        let rest = &input[7..];
        if let Some((a, b)) = split_on(rest, &[" to ", " as "]) {
            let src = clean_token(a);
            let new_name = clean_token(b);
            if !src.is_empty() && !new_name.is_empty() {
                let full = resolve_path(&src, context);
                if let Some(op) = rename_op(&full, &new_name) {
                    return Some(confirm_reply(format!("把 {full} 重命名为 {new_name}"), op));
                }
            }
        }
    }

    // ---- 移动 / 复制 ----
    let move_seps = ["移动到", "移动至", "移到", "挪到", "移入", "移动到"];
    if let Some((left, right)) = split_on(input, &move_seps) {
        let src = strip_prefix_words(left);
        let dst = clean_token(right);
        if !src.is_empty() && !dst.is_empty() {
            let sources = split_sources(&src)
                .into_iter()
                .map(|s| resolve_path(&s, context))
                .collect::<Vec<_>>();
            let target = resolve_path(&dst, context);
            let desc = format!("移动 {} 项到 {target}", sources.len());
            return Some(confirm_reply(
                desc,
                FsOp {
                    op: FsOpKind::Move,
                    sources,
                    target: Some(target),
                },
            ));
        }
    }
    let copy_seps = ["复制到", "复制至", "拷贝到", "复制进"];
    if let Some((left, right)) = split_on(input, &copy_seps) {
        let src = strip_prefix_words(left);
        let dst = clean_token(right);
        if !src.is_empty() && !dst.is_empty() {
            let sources = split_sources(&src)
                .into_iter()
                .map(|s| resolve_path(&s, context))
                .collect::<Vec<_>>();
            let target = resolve_path(&dst, context);
            let desc = format!("复制 {} 项到 {target}", sources.len());
            return Some(confirm_reply(
                desc,
                FsOp {
                    op: FsOpKind::Copy,
                    sources,
                    target: Some(target),
                },
            ));
        }
    }

    // ---- 删除 ----
    if contains_any(
        lower,
        &["删除", "删掉", "移除", "delete", "remove", "trash"],
    ) {
        // 先尝试「删除 X」
        let mut target = strip_any(
            input,
            &["删除", "删掉", "移除", "delete", "remove", "trash"],
        )
        .map(clean_token)
        .unwrap_or_default();
        // 再尝试「把 X 删除」
        if target.is_empty() {
            if let Some((left, _)) = split_on(input, &["删除", "删掉", "移除", "delete", "remove"])
            {
                target = clean_token(strip_prefix_words(left));
            }
        }
        if target.is_empty() {
            return Some(AiReply {
                text: "请告诉我要删除什么，例如：删除 a.txt".to_string(),
                ..Default::default()
            });
        }
        // 「删除最大的 N 个文件」这类：先算出目标再确认。
        let tl = target.to_lowercase();
        if contains_any(&tl, &["最大", "largest", "biggest"]) {
            let n = extract_number(&tl).unwrap_or(5).clamp(1, 100);
            let files = largest_files(context, index, n);
            if files.is_empty() {
                return Some(AiReply {
                    text: "在当前目录范围内没有找到可删除的文件。".to_string(),
                    ..Default::default()
                });
            }
            let paths: Vec<String> = files.iter().map(|f| f.path.clone()).collect();
            let desc = format!("删除最大的 {} 个文件：\n{}", paths.len(), paths.join("\n"));
            return Some(confirm_reply(
                desc,
                FsOp {
                    op: FsOpKind::Delete,
                    sources: paths,
                    target: None,
                },
            ));
        }
        let sources = split_sources(&target)
            .into_iter()
            .map(|s| resolve_path(&s, context))
            .collect::<Vec<_>>();
        let desc = format!("删除 {} 项：\n{}", sources.len(), sources.join("\n"));
        return Some(confirm_reply(
            desc,
            FsOp {
                op: FsOpKind::Delete,
                sources,
                target: None,
            },
        ));
    }

    None
}

fn rename_op(full_path: &str, new_name: &str) -> Option<FsOp> {
    let p = Path::new(full_path);
    let parent = p.parent();
    let target = match parent {
        Some(dir) => dir.join(new_name).to_string_lossy().to_string(),
        None => new_name.to_string(),
    };
    Some(FsOp {
        op: FsOpKind::Rename,
        sources: vec![full_path.to_string()],
        target: Some(target),
    })
}

fn confirm_reply(text: String, op: FsOp) -> AiReply {
    AiReply {
        text: format!("{text}\n\n这是写操作，请确认后我再执行。"),
        tool_calls: Vec::new(),
        pending_confirm: Some(op),
    }
}

// ---------------------------------------------------------------- 离线：只读查询

fn stats_reply(context: &AiContext, index: &IndexStore) -> AiReply {
    match context.active_path.as_deref().map(fs_core::list_dir) {
        Some(Ok(l)) => {
            let dirs = l.entries.iter().filter(|e| e.is_dir).count();
            let files = l.entries.iter().filter(|e| !e.is_dir).count();
            let total: u64 = l.entries.iter().map(|e| e.size).sum();
            AiReply {
                text: format!(
                    "目录 {} 共 {} 项：{} 个文件夹、{} 个文件，文件合计 {}。",
                    l.path,
                    l.entries.len(),
                    dirs,
                    files,
                    human_size(total)
                ),
                ..Default::default()
            }
        }
        _ => {
            let st = index.status();
            AiReply {
                text: format!("当前索引中共 {} 个条目。", st.total),
                ..Default::default()
            }
        }
    }
}

fn count_ext_reply(context: &AiContext, index: &IndexStore, exts: &[String]) -> AiReply {
    let files = collect_all_in_scope(context, index, exts, None, Some(false));
    AiReply {
        text: format!(
            "在当前目录范围内共有 {} 个 {} 文件。",
            files.len(),
            exts.join("/")
        ),
        ..Default::default()
    }
}

/// 取范围内最大的 N 个文件。
///
/// 刻意**不走 `index.search`**：text 为空时其排序退化为按名称，且 limit 截断会
/// 丢掉尾部的大文件。改用全量收集（`collect_all_in_scope`）后自行按 size 排序。
fn largest_files(context: &AiContext, index: &IndexStore, n: usize) -> Vec<FileEntry> {
    let mut files = collect_all_in_scope(context, index, &[], None, Some(false));
    files.sort_by(|a, b| b.size.cmp(&a.size));
    files.truncate(n);
    files
}

/// 全量收集范围内的条目（**不做任何截断**）。
///
/// 供"最大的 N 个"、扩展名、修改时间、计数等需要完整集合的查询使用：
/// - 索引非空 → `index.snapshot()` 全量快照，按 scope/ext/时间/is_dir 过滤（只克隆命中项，不整表复制）；
/// - 索引为空 → `fs_core::collect_entries` 遍历当前目录兜底。
///
/// 与 `search_files_offline`（关键词检索，允许 limit 截断）的区别就在于"不截断"。
fn collect_all_in_scope(
    context: &AiContext,
    index: &IndexStore,
    exts: &[String],
    modified_after: Option<i64>,
    is_dir: Option<bool>,
) -> Vec<FileEntry> {
    let scope = context.active_path.as_deref();
    let snapshot = index.snapshot();
    if !snapshot.is_empty() {
        return filter_entries(snapshot.iter(), scope, exts, modified_after, is_dir);
    }
    // 索引尚未建立 → walkdir 遍历当前目录兜底。
    let Some(s) = scope.filter(|s| !s.is_empty()) else {
        return Vec::new();
    };
    let entries = fs_core::collect_entries(s, 6, 50_000);
    filter_entries(entries.iter(), scope, exts, modified_after, is_dir)
}

/// 纯过滤：scope 前缀 + 扩展名白名单 + 修改时间 + 目录/文件，**不做任何截断**。
fn filter_entries<'a, I>(
    entries: I,
    scope: Option<&str>,
    exts: &[String],
    modified_after: Option<i64>,
    is_dir: Option<bool>,
) -> Vec<FileEntry>
where
    I: IntoIterator<Item = &'a FileEntry>,
{
    entries
        .into_iter()
        .filter(|e| in_scope(e, scope))
        .filter(|e| ext_ok(e, exts))
        .filter(|e| is_dir.map_or(true, |d| e.is_dir == d))
        .filter(|e| modified_after.map_or(true, |t| e.modified >= t))
        .cloned()
        .collect()
}

fn ext_ok(e: &FileEntry, exts: &[String]) -> bool {
    if exts.is_empty() {
        return true;
    }
    let ext = e.ext.clone().unwrap_or_default();
    exts.iter().any(|x| x.eq_ignore_ascii_case(&ext))
}

/// 条目路径是否位于 `scope` 之下（大小写不敏感，Windows 语义）。
fn in_scope(e: &FileEntry, scope: Option<&str>) -> bool {
    let Some(s) = scope.filter(|s| !s.is_empty()) else {
        return true;
    };
    let path = e.path.to_lowercase();
    let root = s.trim_end_matches(|c| c == '\\' || c == '/').to_lowercase();
    if root.is_empty() {
        return true;
    }
    path == root || path.starts_with(&format!("{root}\\")) || path.starts_with(&format!("{root}/"))
}

fn largest_reply(context: &AiContext, index: &IndexStore, n: usize) -> AiReply {
    let files = largest_files(context, index, n);
    let scope = context
        .active_path
        .clone()
        .unwrap_or_else(|| "（未指定范围）".to_string());
    if files.is_empty() {
        return AiReply {
            text: format!("在 {scope} 范围内没有找到文件。"),
            ..Default::default()
        };
    }
    AiReply {
        text: format!(
            "在 {scope} 范围内最大的 {} 个文件：\n{}",
            files.len(),
            list_entries(&files)
        ),
        ..Default::default()
    }
}

fn ext_reply(context: &AiContext, index: &IndexStore, exts: &[String]) -> AiReply {
    let mut files = collect_all_in_scope(context, index, exts, None, Some(false));
    files.sort_by(|a, b| b.size.cmp(&a.size));
    if files.is_empty() {
        return AiReply {
            text: format!("没有找到 {} 类型的文件。", exts.join("/")),
            ..Default::default()
        };
    }
    AiReply {
        text: format!(
            "找到 {} 个 {} 文件：\n{}",
            files.len(),
            exts.join("/"),
            list_entries(&files)
        ),
        ..Default::default()
    }
}

fn modified_reply(context: &AiContext, index: &IndexStore, days: i64) -> AiReply {
    let now = chrono::Utc::now().timestamp_millis();
    let after = now - days * 24 * 3600 * 1000;
    let mut files = collect_all_in_scope(context, index, &[], Some(after), Some(false));
    files.sort_by(|a, b| b.modified.cmp(&a.modified));
    if files.is_empty() {
        return AiReply {
            text: format!("最近 {days} 天内没有修改过的文件。"),
            ..Default::default()
        };
    }
    AiReply {
        text: format!(
            "最近 {} 天内修改过的 {} 项：\n{}",
            days,
            files.len(),
            list_entries(&files)
        ),
        ..Default::default()
    }
}

fn keyword_reply(context: &AiContext, index: &IndexStore, input: &str) -> AiReply {
    let files = search_files_offline(context, index, input);
    if files.is_empty() {
        AiReply {
            text: format!(
                "没有找到与「{input}」匹配的文件。可以试试：\n\
                 · 最大的 10 个文件\n\
                 · 所有图片 / .pdf 文件\n\
                 · 最近 7 天修改的"
            ),
            ..Default::default()
        }
    } else {
        AiReply {
            text: format!(
                "与「{input}」匹配的 {} 项：\n{}",
                files.len(),
                list_entries(&files)
            ),
            ..Default::default()
        }
    }
}

/// 关键词检索：走索引（复用 WS-A 的过滤 / 排序 / limit 截断），索引为空则遍历兜底。
///
/// 关键词检索不需要全量集合，这里的 limit 截断是**合理**的 —— 与 `collect_all_in_scope`
/// （"最大的 N 个"等需要完整集合的场景）明确区分开，不要混用。
fn search_files_offline(context: &AiContext, index: &IndexStore, text: &str) -> Vec<FileEntry> {
    // 复用 WS-A 的查询解析器：支持 ext:pdf / size:>100mb / modified:7d 等高级语法。
    let mut q = crate::search_engine::parse_query(text);
    q.scope = context.active_path.clone();
    if q.limit == 0 {
        q.limit = 200;
    }
    search_with_fallback(index, &q)
        .into_iter()
        .map(|h| h.entry)
        .collect()
}

// ---------------------------------------------------------------- 检索与格式化

/// 优先走索引；索引为空时直接遍历 scope 目录兜底（保证离线演示可用）。
fn search_with_fallback(index: &IndexStore, query: &SearchQuery) -> Vec<SearchHit> {
    let hits = index.search(query);
    if !hits.is_empty() {
        return hits;
    }
    let scope = query
        .scope
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "C:\\".to_string());
    let entries = fs_core::collect_entries(&scope, 6, 50_000);
    let mut hits: Vec<SearchHit> = entries
        .into_iter()
        .filter(|e| matches_query(e, query))
        .map(|e| {
            let score = crate::search_engine::score(&e, query);
            SearchHit { entry: e, score }
        })
        .collect();
    hits.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| b.entry.size.cmp(&a.entry.size))
    });
    let limit = if query.limit == 0 { 500 } else { query.limit };
    hits.truncate(limit);
    hits
}

/// 在 `search_engine::matches` 的基础上补充 ext / size / 时间过滤。
fn matches_query(e: &FileEntry, q: &SearchQuery) -> bool {
    if !crate::search_engine::matches(e, q) {
        return false;
    }
    if !q.ext.is_empty() {
        let ext = e.ext.clone().unwrap_or_default();
        if !q.ext.iter().any(|x| x.eq_ignore_ascii_case(&ext)) {
            return false;
        }
    }
    if let Some(min) = q.min_size {
        if e.size < min {
            return false;
        }
    }
    if let Some(max) = q.max_size {
        if e.size > max {
            return false;
        }
    }
    if let Some(after) = q.modified_after {
        if e.modified < after {
            return false;
        }
    }
    true
}

fn list_entries(entries: &[FileEntry]) -> String {
    entries
        .iter()
        .take(20)
        .map(|e| {
            if e.is_dir {
                format!("- [目录] {} — {}", e.name, e.path)
            } else {
                format!("- {} ({}) — {}", e.name, human_size(e.size), e.path)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut i = 0usize;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{bytes} B")
    } else {
        format!("{:.1} {}", v, UNITS[i])
    }
}

fn fmt_time(ms: i64) -> String {
    use chrono::{Local, TimeZone};
    match Local.timestamp_millis_opt(ms).single() {
        Some(dt) => dt.format("%Y-%m-%d %H:%M").to_string(),
        None => "-".to_string(),
    }
}

// ---------------------------------------------------------------- 解析小工具

fn contains_any(s: &str, kws: &[&str]) -> bool {
    kws.iter().any(|k| s.contains(k))
}

/// 从 JSON 对象中取出字符串数组字段。
fn str_array(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|i| i.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// 取字符串中的第一个连续数字。
fn extract_number(s: &str) -> Option<usize> {
    let mut cur = String::new();
    for ch in s.chars() {
        if ch.is_ascii_digit() {
            cur.push(ch);
        } else if !cur.is_empty() {
            break;
        }
    }
    cur.parse().ok()
}

/// 在 `s` 中查找任一关键词（大小写不敏感），返回关键词之后的剩余部分。
fn strip_any<'a>(s: &'a str, kws: &[&str]) -> Option<&'a str> {
    let ls = s.to_lowercase();
    for k in kws {
        let kl = k.to_lowercase();
        if let Some(i) = ls.find(&kl) {
            let start = i + kl.len();
            if start <= s.len() {
                return Some(s[start..].trim());
            }
        }
    }
    None
}

/// 按分隔符切分「A 分隔符 B」，返回 (左, 右)。
fn split_on<'a>(s: &'a str, seps: &[&str]) -> Option<(&'a str, &'a str)> {
    for sep in seps {
        if let Some(i) = s.find(sep) {
            return Some((&s[..i], &s[i + sep.len()..]));
        }
    }
    None
}

/// 去掉「把 / 将 / 请 / 帮我」等前缀词。
fn strip_prefix_words(s: &str) -> &str {
    let mut t = s.trim();
    loop {
        let mut changed = false;
        for p in ["请帮我", "帮我", "请", "把", "将", "对"] {
            if let Some(rest) = t.strip_prefix(p) {
                t = rest.trim();
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    t
}

/// 清洗 token：去引号、去尾部标点。
fn clean_token(s: &str) -> String {
    s.trim()
        .trim_matches(|c: char| {
            c == '"' || c == '\'' || c == '“' || c == '”' || c == '‘' || c == '’'
        })
        .trim_end_matches(|c: char| {
            c == '。'
                || c == '.'
                || c == '，'
                || c == ','
                || c == '!'
                || c == '！'
                || c == '？'
                || c == '?'
        })
        .trim()
        .to_string()
}

/// 把「a、b, c」拆成多个源路径；无分隔符时按单个处理（内部空格再拆分）。
fn split_sources(s: &str) -> Vec<String> {
    let cleaned = clean_token(s);
    if cleaned.is_empty() {
        return Vec::new();
    }
    let mut parts: Vec<String> = cleaned
        .split(|c| c == '、' || c == ',' || c == '，')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect();
    if parts.len() == 1 && parts[0].contains(' ') {
        parts = parts[0].split_whitespace().map(|p| p.to_string()).collect();
    }
    parts
}

/// 把可能是相对路径的目标解析为绝对路径（相对当前活动目录）。
fn resolve_path(target: &str, context: &AiContext) -> String {
    let t = clean_token(target);
    if t.is_empty() {
        return t;
    }
    let p = Path::new(&t);
    if p.is_absolute() {
        return t;
    }
    match context.active_path.as_deref() {
        Some(base) if !base.is_empty() => Path::new(base).join(&t).to_string_lossy().to_string(),
        _ => t,
    }
}

/// 识别指令中的扩展名（类别关键词优先，其次显式扩展名）。
fn detect_exts(lower: &str) -> Option<Vec<String>> {
    const IMAGE: [&str; 7] = ["png", "jpg", "jpeg", "gif", "bmp", "webp", "svg"];
    const VIDEO: [&str; 5] = ["mp4", "mkv", "avi", "mov", "wmv"];
    const DOC: [&str; 9] = [
        "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "txt", "md",
    ];
    const AUDIO: [&str; 4] = ["mp3", "wav", "flac", "m4a"];

    if contains_any(
        lower,
        &["图片", "照片", "image", "photo", "picture", "截图"],
    ) {
        return Some(to_vec(&IMAGE));
    }
    if contains_any(lower, &["视频", "影片", "video", "movie"]) {
        return Some(to_vec(&VIDEO));
    }
    if contains_any(lower, &["文档", "document", "办公文件"]) {
        return Some(to_vec(&DOC));
    }
    if contains_any(lower, &["音频", "音乐", "audio", "music", "song"]) {
        return Some(to_vec(&AUDIO));
    }

    const KNOWN: [&str; 31] = [
        "pdf", "docx", "doc", "xlsx", "xls", "pptx", "ppt", "txt", "md", "png", "jpg", "jpeg",
        "gif", "bmp", "webp", "svg", "mp4", "mkv", "avi", "mov", "wmv", "mp3", "wav", "flac",
        "m4a", "zip", "rar", "7z", "exe", "json", "csv",
    ];
    let mut found: Vec<String> = Vec::new();
    for k in KNOWN {
        if lower.contains(&format!(".{k}"))
            || lower.contains(&format!("{k}文件"))
            || lower.contains(&format!("{k} 文件"))
            || lower.contains(&format!("扩展名{k}"))
            || lower.contains(&format!("扩展名为{k}"))
            || lower.contains(&format!("{k}格式"))
        {
            found.push(k.to_string());
        }
    }
    if found.is_empty() {
        None
    } else {
        Some(found)
    }
}

fn to_vec(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

// ================================================================ 定时任务规划

impl AiService {
    /// 把自然语言需求规划为一条定时任务草稿。
    ///
    /// - 已启用且配置了 api_key → 走 LLM（要求只输出 JSON 规格）；
    /// - 否则 / 调用失败 → 离线规则解析兜底。
    ///
    /// 注意：规划只在**创建时**发生；运行时由 `schedule` 模块按规格确定性执行。
    pub async fn plan_task(&self, prompt: String, context: AiContext) -> TaskPlanReply {
        let cfg = self.config();
        if cfg.enabled && !cfg.api_key.trim().is_empty() {
            match plan_task_llm(&cfg, &prompt, &context).await {
                Ok(reply) => return reply,
                Err(e) => {
                    let mut reply = offline_plan_task(&prompt, &context);
                    reply.text = format!("⚠️ LLM 调用失败，已回退到离线解析：{e}\n\n{}", reply.text);
                    return reply;
                }
            }
        }
        offline_plan_task(&prompt, &context)
    }
}

/// 调 LLM 生成任务规格（要求只输出 JSON 对象）。
async fn plan_task_llm(
    cfg: &AiConfig,
    prompt: &str,
    context: &AiContext,
) -> Result<TaskPlanReply, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| format!("初始化 HTTP 客户端失败：{e}"))?;
    let url = format!("{}/chat/completions", cfg.base_url.trim_end_matches('/'));
    let active = context
        .active_path
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "（未知）".to_string());
    let system = format!(
        "你是 Stelix File Manager（星索）的定时任务规划助手，运行在 Windows 上。\n\
         当前活动目录：{active}\n\
         把用户的中文需求转换为一条**文件清理/整理**定时任务的 JSON 规格。\n\
         只输出一个 JSON 对象，不要输出解释或 Markdown 代码围栏。JSON 结构：\n\
         {{\n\
           \"name\": \"简短中文任务名\",\n\
           \"schedule\": {{\"kind\":\"interval|daily|weekly|monthly\",\"intervalMinutes\":null,\"hour\":null,\"minute\":null,\"weekday\":null,\"dayOfMonth\":null}},\n\
           \"action\": {{\"kind\":\"delete|move\",\"root\":\"绝对路径\",\"recursive\":true,\"moveTarget\":null,\"filters\":{{\"extensions\":[],\"nameContains\":null,\"minAgeDays\":null,\"minSizeBytes\":null,\"maxSizeBytes\":null}}}}\n\
         }}\n\
         约定：\n\
         1. interval 填 intervalMinutes（每 N 分钟）；daily/weekly/monthly 填 hour/minute；weekly 另填 weekday（0=周日…6=周六）；monthly 另填 dayOfMonth（1-28）。\n\
         2. action.kind 默认 delete（清理）；若用户说「移动到 / 归档到」则用 move 并填 moveTarget。\n\
         3. filters.extensions 用不带点的小写扩展名；不限制则空数组。\n\
         4. 未指定目录时使用「当前活动目录」。\n\
         5. 必须至少给出一个筛选条件（extensions / nameContains / minAgeDays / minSizeBytes / maxSizeBytes），否则会被系统拒绝。"
    );
    let body = json!({
        "model": cfg.model,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": prompt}
        ],
        "stream": false
    });
    let resp = client
        .post(&url)
        .bearer_auth(cfg.api_key.trim())
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("请求 LLM 失败：{e}"))?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        let snippet: String = text.chars().take(500).collect();
        return Err(format!("LLM 返回错误 {status}：{snippet}"));
    }
    let data: Value = resp
        .json()
        .await
        .map_err(|e| format!("解析 LLM 响应失败：{e}"))?;
    let content = data
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();
    match extract_draft(&content) {
        Some(draft) => Ok(TaskPlanReply {
            text: format!("已生成任务草稿「{}」，请确认或调整后保存。", draft.name),
            draft: Some(draft),
        }),
        None => Ok(TaskPlanReply {
            text: format!(
                "未能从模型输出中解析出任务规格，请手动填写，或换个说法重试。\n\n模型原始输出：\n{content}"
            ),
            draft: None,
        }),
    }
}

/// 从模型输出里截取第一个 JSON 对象并解析为任务草稿。
fn extract_draft(content: &str) -> Option<TaskDraft> {
    let start = content.find('{')?;
    let end = content.rfind('}')?;
    if end <= start {
        return None;
    }
    let draft: TaskDraft = serde_json::from_str(&content[start..=end]).ok()?;
    if draft.action.root.trim().is_empty() || draft.schedule.kind.trim().is_empty() {
        return None;
    }
    Some(draft)
}

// ---------------------------------------------------------------- 离线：定时任务规划

fn offline_plan_task(prompt: &str, context: &AiContext) -> TaskPlanReply {
    let input = prompt.trim();
    if input.is_empty() {
        return TaskPlanReply {
            text: "请描述你的定时任务，例如：\n\
                   · 每天 9:00 清理下载文件夹里超过 30 天的 .tmp 文件\n\
                   · 每周一 8:30 把下载目录里的安装包移动到 D:\\归档"
                .to_string(),
            draft: None,
        };
    }
    let lower = input.to_lowercase();
    let Some(schedule) = parse_schedule(&lower) else {
        return TaskPlanReply {
            text: "未能识别触发时间，请说明频率，例如「每天 9:00」「每 6 小时」「每周一 8:30」「每月 1 日 3:00」。"
                .to_string(),
            draft: None,
        };
    };

    let move_keys = ["移动到", "移到", "归档到", "移入", "挪到"];
    let (kind, move_target) = if contains_any(&lower, &move_keys) {
        let target = strip_any(input, &move_keys)
            .map(clean_token)
            .filter(|s| !s.is_empty())
            .map(|s| resolve_path(&s, context));
        ("move", target)
    } else {
        ("delete", None)
    };

    let extensions = detect_task_exts(&lower);
    let min_age_days = extract_days(&lower);
    if extensions.is_empty() && min_age_days.is_none() {
        return TaskPlanReply {
            text: "请补充筛选条件（扩展名，或「超过 N 天」），例如：清理 D:\\下载 里超过 30 天的 .tmp 文件。"
                .to_string(),
            draft: None,
        };
    }

    let root = extract_task_root(input, context);
    let draft = TaskDraft {
        name: build_task_name(&schedule, kind, &extensions, min_age_days),
        schedule,
        action: TaskAction {
            kind: kind.to_string(),
            root,
            recursive: true,
            move_target,
            filters: TaskFilters {
                extensions,
                name_contains: None,
                min_age_days,
                min_size_bytes: None,
                max_size_bytes: None,
            },
        },
    };
    TaskPlanReply {
        text: "已根据描述生成任务草稿（离线解析），请确认或调整后保存。".to_string(),
        draft: Some(draft),
    }
}

/// 解析触发频率（简单预设）。
fn parse_schedule(lower: &str) -> Option<TaskSchedule> {
    if contains_any(lower, &["每", "每隔"]) {
        if let Some(idx) = lower.find("分钟") {
            if let Some(n) = trailing_number(&lower[..idx]) {
                return Some(TaskSchedule {
                    kind: "interval".into(),
                    interval_minutes: Some(n.max(1)),
                    ..Default::default()
                });
            }
        }
        if let Some(idx) = lower.find("小时") {
            let n = trailing_number(&lower[..idx]).unwrap_or(1).max(1);
            return Some(TaskSchedule {
                kind: "interval".into(),
                interval_minutes: Some(n * 60),
                ..Default::default()
            });
        }
    }
    let (hour, minute) = extract_clock(lower);
    if contains_any(lower, &["每天", "每日", "每晚", "daily", "every day"]) {
        return Some(TaskSchedule {
            kind: "daily".into(),
            hour: Some(hour),
            minute: Some(minute),
            ..Default::default()
        });
    }
    if let Some(weekday) = extract_weekday(lower) {
        return Some(TaskSchedule {
            kind: "weekly".into(),
            hour: Some(hour),
            minute: Some(minute),
            weekday: Some(weekday),
            ..Default::default()
        });
    }
    if let Some(day) = extract_month_day(lower) {
        return Some(TaskSchedule {
            kind: "monthly".into(),
            hour: Some(hour),
            minute: Some(minute),
            day_of_month: Some(day),
            ..Default::default()
        });
    }
    None
}

/// 从文本里取时间（HH:MM 优先，其次「N 点 / N 时」），缺省 0:00。
fn extract_clock(s: &str) -> (u8, u8) {
    let bytes = s.as_bytes();
    for i in 0..bytes.len() {
        if bytes[i] != b':' {
            continue;
        }
        let mut start = i;
        while start > 0 && bytes[start - 1].is_ascii_digit() {
            start -= 1;
        }
        let mut end = i + 1;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        if let (Ok(hour), Ok(minute)) = (s[start..i].parse::<u8>(), s[i + 1..end].parse::<u8>()) {
            if hour < 24 && minute < 60 {
                return (hour, minute);
            }
        }
    }
    for unit in ["点", "时"] {
        if let Some(idx) = s.find(unit) {
            if let Some(hour) = trailing_number(&s[..idx]) {
                if hour < 24 {
                    let after = &s[idx + unit.len()..];
                    let minute = leading_number(after).unwrap_or(0).min(59);
                    return (hour as u8, minute as u8);
                }
            }
        }
    }
    (0, 0)
}

fn extract_weekday(lower: &str) -> Option<u8> {
    for prefix in ["周", "星期"] {
        if let Some(idx) = lower.find(prefix) {
            if let Some(ch) = lower[idx + prefix.len()..].chars().next() {
                if let Some(day) = cn_weekday(ch) {
                    return Some(day);
                }
            }
        }
    }
    None
}

fn cn_weekday(ch: char) -> Option<u8> {
    match ch {
        '一' => Some(1),
        '二' => Some(2),
        '三' => Some(3),
        '四' => Some(4),
        '五' => Some(5),
        '六' => Some(6),
        '日' | '天' => Some(0),
        _ => None,
    }
}

fn extract_month_day(lower: &str) -> Option<u8> {
    if !contains_any(lower, &["每月", "每个月"]) {
        return None;
    }
    for unit in ["号", "日"] {
        if let Some(idx) = lower.find(unit) {
            if let Some(n) = trailing_number(&lower[..idx]) {
                if (1..=28).contains(&n) {
                    return Some(n as u8);
                }
            }
        }
    }
    Some(1)
}

/// 识别「超过 N 天 / N 天前」中的天数（跳过「每天」这类无数字的「天」）。
fn extract_days(lower: &str) -> Option<u32> {
    let mut offset = 0usize;
    while let Some(rel) = lower[offset..].find('天') {
        let abs = offset + rel;
        if let Some(n) = trailing_number(&lower[..abs]) {
            return Some(n.min(3650) as u32);
        }
        offset = abs + '天'.len_utf8();
    }
    None
}

fn detect_task_exts(lower: &str) -> Vec<String> {
    let mut exts: Vec<String> = Vec::new();
    if contains_any(lower, &["临时", "tmp", "temp"]) {
        exts.push("tmp".into());
    }
    if contains_any(lower, &["日志", "log"]) {
        exts.push("log".into());
    }
    if contains_any(lower, &["缓存", "cache"]) {
        exts.push("cache".into());
    }
    if contains_any(lower, &["安装包", "安装程序"]) {
        exts.push("exe".into());
        exts.push("msi".into());
    }
    if let Some(more) = detect_exts(lower) {
        exts.extend(more);
    }
    exts.sort();
    exts.dedup();
    exts
}

/// 定位任务根目录：引号路径 > 盘符路径 > 当前活动目录。
fn extract_task_root(input: &str, context: &AiContext) -> String {
    for (open, close) in [('"', '"'), ('“', '”'), ('‘', '’')] {
        if let Some(start) = input.find(open) {
            let after = start + open.len_utf8();
            if let Some(rel_end) = input[after..].find(close) {
                let candidate = &input[after..after + rel_end];
                if Path::new(candidate).is_absolute() {
                    return candidate.to_string();
                }
            }
        }
    }
    if let Some(found) = find_windows_path(input) {
        return found;
    }
    context
        .active_path
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_default()
}

fn find_windows_path(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    for i in 0..bytes.len().saturating_sub(2) {
        if bytes[i].is_ascii_alphabetic()
            && bytes[i + 1] == b':'
            && (bytes[i + 2] == b'\\' || bytes[i + 2] == b'/')
        {
            let rest = &s[i..];
            let end = path_end(rest);
            let candidate = rest[..end].trim();
            if candidate.len() > 3 {
                return Some(candidate.replace('/', "\\"));
            }
        }
    }
    None
}

/// 判断 Windows 路径在文本中的结束位置：遇中文标点即停；
/// 遇空白时，若其后紧跟中日韩文字也视为结束（避免把中文句子并进路径）。
fn path_end(s: &str) -> usize {
    for (idx, ch) in s.char_indices() {
        if matches!(
            ch,
            '，' | '。' | '、' | '；' | ';' | ',' | '"' | '”' | '“' | '\n'
        ) {
            return idx;
        }
        if ch.is_whitespace() {
            let after = idx + ch.len_utf8();
            match s[after..].chars().next() {
                Some(next) if !is_cjk(next) => {}
                _ => return idx,
            }
        }
    }
    s.len()
}

fn is_cjk(ch: char) -> bool {
    matches!(ch, '\u{3000}'..='\u{303F}' | '\u{4E00}'..='\u{9FFF}' | '\u{FF00}'..='\u{FFEF}')
}

fn build_task_name(
    schedule: &TaskSchedule,
    kind: &str,
    exts: &[String],
    days: Option<u32>,
) -> String {
    let verb = if kind == "move" { "归档" } else { "清理" };
    let mut parts: Vec<String> = Vec::new();
    if !exts.is_empty() {
        let list = exts
            .iter()
            .map(|e| format!(".{e}"))
            .collect::<Vec<_>>()
            .join("/");
        parts.push(format!("{list} 文件"));
    }
    if let Some(d) = days {
        parts.push(format!("超过 {d} 天"));
    }
    let what = if parts.is_empty() {
        "文件".to_string()
    } else {
        parts.join("、")
    };
    let when = match schedule.kind.as_str() {
        "interval" => format!("每 {} 分钟", schedule.interval_minutes.unwrap_or(0)),
        "daily" => format!(
            "每天 {:02}:{:02}",
            schedule.hour.unwrap_or(0),
            schedule.minute.unwrap_or(0)
        ),
        "weekly" => format!(
            "每周{} {:02}:{:02}",
            weekday_cn(schedule.weekday.unwrap_or(0)),
            schedule.hour.unwrap_or(0),
            schedule.minute.unwrap_or(0)
        ),
        "monthly" => format!(
            "每月 {} 日 {:02}:{:02}",
            schedule.day_of_month.unwrap_or(1),
            schedule.hour.unwrap_or(0),
            schedule.minute.unwrap_or(0)
        ),
        _ => String::new(),
    };
    format!("{when} {verb}{what}").trim().to_string()
}

fn weekday_cn(day: u8) -> &'static str {
    match day {
        1 => "一",
        2 => "二",
        3 => "三",
        4 => "四",
        5 => "五",
        6 => "六",
        _ => "日",
    }
}

/// 取字符串末尾的整数（跳过末尾非数字，如「每 6 」→ 6）。
fn trailing_number(s: &str) -> Option<u64> {
    let digits: String = s
        .chars()
        .rev()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        return None;
    }
    digits.chars().rev().collect::<String>().parse().ok()
}

/// 取字符串开头的整数（跳过前导非数字，如「30 天」→ 30）。
fn leading_number(s: &str) -> Option<u64> {
    let digits: String = s
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        None
    } else {
        digits.parse().ok()
    }
}

// ================================================================ 测试

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(path: &str) -> AiContext {
        AiContext {
            active_path: Some(path.to_string()),
            selected: vec![],
        }
    }

    fn user(text: &str) -> Vec<AiMessage> {
        vec![AiMessage {
            role: "user".to_string(),
            content: text.to_string(),
        }]
    }

    fn setup_dir(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("file_povit_test_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn extract_number_basic() {
        assert_eq!(extract_number("最大的 5 个文件"), Some(5));
        assert_eq!(extract_number("largest 12 files"), Some(12));
        assert_eq!(extract_number("没有数字"), None);
    }

    #[test]
    fn detect_category_exts() {
        let img = detect_exts("所有图片").unwrap();
        assert!(img.contains(&"png".to_string()));
        assert!(img.contains(&"jpg".to_string()));

        let vid = detect_exts("视频").unwrap();
        assert!(vid.contains(&"mp4".to_string()));

        let doc = detect_exts("找出 .pdf 文件").unwrap();
        assert!(doc.contains(&"pdf".to_string()));
    }

    #[test]
    fn offline_largest_lists_biggest() {
        let dir = setup_dir("largest");
        std::fs::write(dir.join("small.txt"), b"a").unwrap();
        std::fs::write(dir.join("big.bin"), vec![0u8; 8192]).unwrap();
        let index = IndexStore::new();
        let reply = offline_reply(
            &user("最大的 2 个文件"),
            &ctx(&dir.to_string_lossy()),
            &index,
        );
        assert!(reply.text.contains("big.bin"), "text={}", reply.text);
        assert!(reply.pending_confirm.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn offline_ext_search() {
        let dir = setup_dir("ext");
        std::fs::write(dir.join("a.pdf"), b"x").unwrap();
        std::fs::write(dir.join("b.txt"), b"y").unwrap();
        let index = IndexStore::new();
        let reply = offline_reply(
            &user("找出 .pdf 文件"),
            &ctx(&dir.to_string_lossy()),
            &index,
        );
        assert!(reply.text.contains("a.pdf"), "text={}", reply.text);
        assert!(!reply.text.contains("b.txt"), "text={}", reply.text);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn offline_delete_needs_confirm() {
        let index = IndexStore::new();
        let reply = offline_reply(&user("删除 a.txt"), &ctx("C:\\tmp"), &index);
        let op = reply.pending_confirm.expect("应生成待确认操作");
        assert_eq!(op.op, FsOpKind::Delete);
        assert!(op.sources.iter().any(|s| s.ends_with("a.txt")));
    }

    #[test]
    fn offline_move_needs_confirm() {
        let index = IndexStore::new();
        let reply = offline_reply(&user("把 a.txt 移动到 D:\\归档"), &ctx("C:\\tmp"), &index);
        let op = reply.pending_confirm.expect("应生成待确认操作");
        assert_eq!(op.op, FsOpKind::Move);
        assert_eq!(op.target.as_deref(), Some("D:\\归档"));
    }

    #[test]
    fn offline_mkdir_needs_confirm() {
        let index = IndexStore::new();
        let reply = offline_reply(&user("新建文件夹 报告"), &ctx("C:\\tmp"), &index);
        let op = reply.pending_confirm.expect("应生成待确认操作");
        assert_eq!(op.op, FsOpKind::Mkdir);
        assert!(op.target.unwrap().ends_with("报告"));
    }

    #[test]
    fn offline_rename_needs_confirm() {
        let index = IndexStore::new();
        let reply = offline_reply(&user("把 a.txt 重命名为 b.txt"), &ctx("C:\\tmp"), &index);
        let op = reply.pending_confirm.expect("应生成待确认操作");
        assert_eq!(op.op, FsOpKind::Rename);
        assert!(op.target.unwrap().ends_with("b.txt"));
    }

    #[test]
    fn largest_not_lost_to_truncation() {
        // 造 105_000 条（多于旧实现的 10 万截断阈值），最大的那条文件名排序在最后。
        const N: usize = 105_000;
        let entries: Vec<FileEntry> = (0..N)
            .map(|i| FileEntry {
                id: i as u64,
                name: format!("f{i:06}.bin"),
                path: format!("C:\\scope\\f{i:06}.bin"),
                is_dir: false,
                size: if i == N - 1 { 999_999_999 } else { i as u64 },
                modified: 0,
                ext: Some("bin".into()),
                hidden: false,
                system: false,
            })
            .collect();

        // 旧路径的效果：先按 limit 截断到 10 万，再按 size 排序 → 最大值已被丢掉。
        let mut old = entries[..100_000].to_vec();
        old.sort_by(|a, b| b.size.cmp(&a.size));
        assert_ne!(old[0].size, 999_999_999, "前提：旧实现确实会丢掉最大值");

        // 新路径（collect_all_in_scope 的核心）：全量过滤、不截断 → 拿到真正最大的。
        let mut new = filter_entries(entries.iter(), Some("C:\\scope"), &[], None, Some(false));
        assert_eq!(new.len(), N, "全量收集不应被截断");
        new.sort_by(|a, b| b.size.cmp(&a.size));
        assert_eq!(new[0].size, 999_999_999);
        assert_eq!(new[0].name, "f104999.bin");
    }

    #[test]
    fn in_scope_matches_subpaths() {
        let e = FileEntry {
            id: 1,
            name: "a.txt".into(),
            path: "C:\\tmp\\sub\\a.txt".into(),
            is_dir: false,
            size: 1,
            modified: 0,
            ext: Some("txt".into()),
            hidden: false,
            system: false,
        };
        assert!(in_scope(&e, Some("C:\\tmp")));
        assert!(in_scope(&e, Some("c:\\TMP\\")));
        assert!(!in_scope(&e, Some("C:\\other")));
        assert!(in_scope(&e, None));
    }

    #[test]
    fn execute_tool_write_requires_approval() {
        let index = IndexStore::new();
        let call = AiToolCall {
            id: "1".into(),
            name: "delete_files".into(),
            args: json!({"paths": ["C:\\tmp\\a.txt"]}),
        };
        assert!(execute_tool(&call, false, &index).is_err());
    }

    #[test]
    fn parse_schedule_daily_with_clock() {
        let s = parse_schedule("每天 9:00 清理").unwrap();
        assert_eq!(s.kind, "daily");
        assert_eq!(s.hour, Some(9));
        assert_eq!(s.minute, Some(0));
    }

    #[test]
    fn parse_schedule_interval_hours() {
        let s = parse_schedule("每 6 小时清理一次").unwrap();
        assert_eq!(s.kind, "interval");
        assert_eq!(s.interval_minutes, Some(360));
    }

    #[test]
    fn parse_schedule_weekly() {
        let s = parse_schedule("每周一 8:30 归档").unwrap();
        assert_eq!(s.kind, "weekly");
        assert_eq!(s.weekday, Some(1));
        assert_eq!(s.hour, Some(8));
        assert_eq!(s.minute, Some(30));
    }

    #[test]
    fn extract_days_skips_meitian() {
        assert_eq!(extract_days("每天清理超过 30 天的文件"), Some(30));
        assert_eq!(extract_days("每天清理文件"), None);
    }

    #[test]
    fn offline_plan_task_builds_draft() {
        let reply = offline_plan_task("每天 9:00 清理 D:\\下载 里超过 30 天的 .tmp 文件", &ctx("C:\\"));
        let draft = reply.draft.expect("应生成草稿");
        assert_eq!(draft.schedule.kind, "daily");
        assert_eq!(draft.action.kind, "delete");
        assert_eq!(draft.action.root, "D:\\下载");
        assert_eq!(draft.action.filters.extensions, vec!["tmp".to_string()]);
        assert_eq!(draft.action.filters.min_age_days, Some(30));
    }

    #[test]
    fn offline_plan_task_requires_filter() {
        let reply = offline_plan_task("每天清理文件", &ctx("C:\\"));
        assert!(reply.draft.is_none());
    }
}

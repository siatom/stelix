//! AI 对话与工具编排 —— 由 **WS-B (feat/fs-ai)** 实现。
//!
//! 契约（Phase 0 冻结）：
//! - `AiService::new / config / set_config / chat`
//! - 自由函数 `execute_tool(call, approved, index)` 供 command 层调用
//! - 无 API key 时必须走离线规则解析器，保证演示不依赖外部服务
//! - 写操作（Move/Delete/Rename/Mkdir）必须经 `AiReply::pending_confirm` 二次确认

use parking_lot::RwLock;
use tauri::AppHandle;

use crate::index_engine::IndexStore;
use crate::persistence;
use crate::types::{AiConfig, AiContext, AiMessage, AiReply, AiToolCall, ToolOutcome};

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

    /// 发起一轮对话。TODO(WS-B):
    /// 1. 若 config.enabled 且有 api_key → 调 OpenAI 兼容 /chat/completions，
    ///    带 tools 定义；解析 tool_calls；只读工具就地执行后回灌，写操作返回 pending_confirm。
    /// 2. 否则 → 离线规则解析器（正则识别"最大的 N 个文件""扩展名为 x 的文件"等）。
    /// 3. 流式 token 通过 app.emit(events::AI_TOKEN, ...) 推送。
    pub async fn chat(
        &self,
        _app: AppHandle,
        messages: Vec<AiMessage>,
        context: AiContext,
    ) -> Result<AiReply, String> {
        let last = messages
            .iter()
            .rev()
            .find(|m| m.role == "user")
            .map(|m| m.content.clone())
            .unwrap_or_default();
        let scope = context
            .active_path
            .clone()
            .map(|p| format!("（当前目录：{p}）"))
            .unwrap_or_default();
        Ok(AiReply {
            text: format!(
                "[离线模式] 已收到：\"{last}\"{scope}\nWS-B 将在此实现真实 LLM 调用与工具编排。"
            ),
            tool_calls: Vec::new(),
            pending_confirm: None,
        })
    }
}

impl Default for AiService {
    fn default() -> Self {
        Self::new()
    }
}

/// 执行一次工具调用。TODO(WS-B): 实现 search_files / list_dir / fs 写操作分发。
pub fn execute_tool(
    _call: &AiToolCall,
    _approved: bool,
    _index: &IndexStore,
) -> Result<ToolOutcome, String> {
    Err("execute_tool 尚未实现（WS-B）".into())
}

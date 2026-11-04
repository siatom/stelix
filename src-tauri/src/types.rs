//! 前后端共享的数据契约。
//!
//! ⚠️ 本文件在 Phase 0 冻结，是并行开发的依据。
//! WS-A（索引/检索）与 WS-B（文件操作/AI）都依赖此处的类型定义，
//! 各 workstream **不得修改** 本文件的字段名与结构，只能新增（并同步通知）。

use serde::{Deserialize, Serialize};

/// 单个文件 / 目录条目
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    /// 稳定 id（路径哈希），供前端 key 与选中使用
    pub id: u64,
    pub name: String,
    /// 绝对路径
    pub path: String,
    pub is_dir: bool,
    /// 字节；目录为 0
    pub size: u64,
    /// Unix 毫秒时间戳
    pub modified: i64,
    /// 小写扩展名，无扩展名为 None
    pub ext: Option<String>,
    /// Windows 隐藏属性（FILE_ATTRIBUTE_HIDDEN）；非 Windows 恒 false
    pub hidden: bool,
    /// Windows 系统属性（FILE_ATTRIBUTE_SYSTEM）；非 Windows 恒 false
    pub system: bool,
}

/// 目录列表结果
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirListing {
    pub path: String,
    pub entries: Vec<FileEntry>,
}

/// 驱动器信息
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveInfo {
    pub name: String,
    pub mount: String,
    pub total: u64,
    pub free: u64,
}

/// 检索查询（由搜索框或 AI 工具调用构造）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchQuery {
    /// 文件名关键词（空串表示仅按条件过滤）
    #[serde(default)]
    pub text: String,
    /// 限定搜索根路径
    #[serde(default)]
    pub scope: Option<String>,
    /// Some(true) 只要目录，Some(false) 只要文件
    #[serde(default)]
    pub is_dir: Option<bool>,
    /// 扩展名白名单（小写，不含点）
    #[serde(default)]
    pub ext: Vec<String>,
    #[serde(default)]
    pub min_size: Option<u64>,
    #[serde(default)]
    pub max_size: Option<u64>,
    /// 仅保留该时间之后修改的条目（Unix 毫秒）
    #[serde(default)]
    pub modified_after: Option<i64>,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    500
}

/// 检索命中
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub entry: FileEntry,
    pub score: i32,
}

/// 索引状态
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexStatus {
    pub roots: Vec<String>,
    /// 已建立索引的条目总数
    pub total: usize,
    pub scanning: bool,
    /// 本次扫描已处理条目数
    pub scanned: usize,
    /// 当前是否由 Everything 提供全局搜索索引。
    #[serde(default)]
    pub using_everything: bool,
    /// 启动后正在等待 Everything IPC 就绪。
    #[serde(default)]
    pub waiting_for_everything: bool,
}

/// 索引进度事件负载
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexProgress {
    pub scanned: usize,
    pub total: usize,
    pub current: String,
    pub done: bool,
}

/// 文件操作类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FsOpKind {
    Copy,
    Move,
    Delete,
    Rename,
    Mkdir,
    /// 新建空文件
    CreateFile,
    /// 在原目录生成「xxx - 副本」副本
    Duplicate,
}

/// 一次文件操作请求
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsOp {
    pub op: FsOpKind,
    /// 源路径；CreateFile 时为空。Mkdir 时作为额外的目录目标（批量创建 / 批量提权重试）。
    #[serde(default)]
    pub sources: Vec<String>,
    /// 目标路径：Move/Copy 为目标目录，Rename 为新名（含路径），
    /// Mkdir 为新目录路径（批量创建时也可只填 sources），CreateFile 为新文件的完整路径
    #[serde(default)]
    pub target: Option<String>,
    /// Delete 时是否跳过回收站直接物理删除（永久删除，不可恢复）。默认 false。
    #[serde(default)]
    pub permanent: bool,
}

/// 文件操作结果
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsOpResult {
    pub ok: bool,
    #[serde(default)]
    pub errors: Vec<String>,
    /// 因「拒绝访问」（Windows `ERROR_ACCESS_DENIED` = `os error 5`）失败的路径。
    ///
    /// 前端据此发起 UAC 提权，并且**只重试这些路径**（已成功的项不重复执行）。
    /// Delete/Rename/Copy/Move 记的是失败的源路径；Mkdir/CreateFile 记的是 target。
    /// 其它错误（文件被占用 32 / 目录非空 145 / 权限不足 1314 等）**不**进入此列表。
    #[serde(default)]
    pub denied: Vec<String>,
    /// 已成功执行的条目对应的**逆向操作**（按执行顺序记录），供前端撤销。
    ///
    /// - Mkdir / CreateFile → 删除新建的路径
    /// - Rename → 改回原名
    /// - Copy / Move → 删除副本 / 移回原目录
    /// - Delete 无法逆向（普通删除可去回收站还原），不记录
    #[serde(default)]
    pub undo: Vec<FsOp>,
}

/// 回收站中的一个条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashEntry {
    /// 系统回收站内的 opaque id，用于还原 / 永久删除。
    pub id: String,
    pub name: String,
    /// 删除前的完整路径。
    pub original_path: String,
    /// 删除前所在目录。
    pub original_parent: String,
    /// 删除时间（Unix 毫秒）。
    pub deleted_at: i64,
    pub is_dir: bool,
    /// 文件字节数；目录为 0。
    pub size: u64,
    /// 目录的直接子项数量；文件为 0。
    pub item_count: u64,
}

/// 回收站批量操作结果（允许部分成功）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashOpResult {
    pub ok: usize,
    #[serde(default)]
    pub errors: Vec<String>,
}

/// AI 对话消息
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiMessage {
    /// "user" | "assistant" | "system"
    pub role: String,
    pub content: String,
}

/// 本地历史仅保存展示消息，不保存可恢复执行的文件操作。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiChatMessage {
    pub id: String,
    pub role: String,
    pub content: String,
    pub created_at: i64,
    pub status: String,
    pub model: String,
    pub kind: String,
    #[serde(default)]
    pub operation: Option<String>,
    /// 该轮回复的 token 用量（旧历史文件无此字段）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<AiUsage>,
    /// 该轮回复耗时（毫秒，由前端测量；旧历史文件无此字段）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiConversation {
    pub id: String,
    pub title: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub messages: Vec<AiChatMessage>,
}

/// AI 工具调用
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiToolCall {
    pub id: String,
    pub name: String,
    pub args: serde_json::Value,
}

/// AI 会话上下文（由前端根据当前活动窗格与选中项构造）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiContext {
    /// 当前活动窗格所在路径
    #[serde(default)]
    pub active_path: Option<String>,
    /// 当前选中项路径
    #[serde(default)]
    pub selected: Vec<String>,
}

/// AI 回复
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiReply {
    pub text: String,
    #[serde(default)]
    pub tool_calls: Vec<AiToolCall>,
    /// 需要用户二次确认的写操作（前端弹确认框，确认后调 ai_execute_tool）
    #[serde(default)]
    pub pending_confirm: Option<FsOp>,
    /// 用户中途停止了生成：`text` 为已收到的部分文本
    #[serde(default)]
    pub interrupted: bool,
    /// 本轮（含工具多轮）累计的 token 用量；模型未返回时为 None
    #[serde(default)]
    pub usage: Option<AiUsage>,
}

/// 一次回复的 token 用量。
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiUsage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
}

/// 工具执行结果
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "payload")]
pub enum ToolOutcome {
    /// 检索结果
    Search(Vec<SearchHit>),
    /// 目录列表
    Listing(DirListing),
    /// 文件操作结果
    Fs(FsOpResult),
    /// 纯文本
    Text(String),
}

/// AI 接入配置；Provider 与模型独立管理。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiConfig {
    #[serde(default)]
    pub providers: Vec<AiProvider>,
    #[serde(default)]
    pub active_provider_id: String,
    #[serde(default)]
    pub active_model_id: String,
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AiProtocol {
    #[serde(rename = "openai-chat")]
    OpenaiChat,
    #[serde(rename = "openai-responses")]
    OpenaiResponses,
    #[serde(rename = "anthropic")]
    Anthropic,
    #[serde(rename = "gemini")]
    Gemini,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiProvider {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub vendor: String,
    pub protocol: AiProtocol,
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub models: Vec<AiModel>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiModel {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub vendor: String,
}

impl AiConfig {
    pub fn active(&self) -> Result<(&AiProvider, &AiModel), String> {
        let provider = self
            .providers
            .iter()
            .find(|p| p.id == self.active_provider_id)
            .ok_or("请选择 AI Provider")?;
        let model = provider
            .models
            .iter()
            .find(|m| m.id == self.active_model_id)
            .ok_or("请选择 AI 模型")?;
        Ok((provider, model))
    }

    pub fn validate(&self) -> Result<(), String> {
        let mut ids = std::collections::HashSet::new();
        for provider in &self.providers {
            if provider.id.trim().is_empty() || !ids.insert(&provider.id) {
                return Err("Provider 标识不能为空或重复".into());
            }
            if provider.name.trim().is_empty() {
                return Err("Provider 名称不能为空".into());
            }
            validate_provider_url(&provider.base_url)?;
            let mut models = std::collections::HashSet::new();
            for model in &provider.models {
                if model.id.trim().is_empty() || !models.insert(&model.id) {
                    return Err("同一 Provider 的模型 ID 不能为空或重复".into());
                }
            }
        }
        if self.enabled || !self.active_provider_id.is_empty() || !self.active_model_id.is_empty() {
            self.active()?;
        }
        Ok(())
    }
}

/// 校验 Provider 的基础地址为不带凭据/查询/片段的 HTTP(S) 地址。
pub fn validate_provider_url(base_url: &str) -> Result<(), String> {
    let url = reqwest::Url::parse(base_url).map_err(|_| "API 地址无效")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("API 地址须为 HTTP/HTTPS 基础地址，不能包含凭据、查询参数或片段".into());
    }
    Ok(())
}

// ---------------------------------------------------------------- 定时任务
//
// 设计：**确定性规格**。AI 只在创建时把自然语言解析成下面的结构化规格，
// 运行时由后端按规格确定性执行（不调用 LLM），并可先 dry-run 预演。

/// 触发规则（简单预设）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSchedule {
    /// "interval"（每 N 分钟）| "daily"（每天 HH:MM）| "weekly"（每周 X HH:MM）
    /// | "monthly"（每月 X 日 HH:MM）
    #[serde(default)]
    pub kind: String,
    /// interval：间隔分钟数
    #[serde(default)]
    pub interval_minutes: Option<u64>,
    /// daily / weekly / monthly：小时（0-23）
    #[serde(default)]
    pub hour: Option<u8>,
    /// daily / weekly / monthly：分钟（0-59）
    #[serde(default)]
    pub minute: Option<u8>,
    /// weekly：0=周日 … 6=周六
    #[serde(default)]
    pub weekday: Option<u8>,
    /// monthly：1-28
    #[serde(default)]
    pub day_of_month: Option<u8>,
}

/// 文件筛选条件；全部为空表示「不限」。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskFilters {
    /// 扩展名白名单（小写、不含点）；空 = 不限
    #[serde(default)]
    pub extensions: Vec<String>,
    /// 文件名包含（不区分大小写）；空 = 不限
    #[serde(default)]
    pub name_contains: Option<String>,
    /// 仅处理修改时间早于 N 天的文件
    #[serde(default)]
    pub min_age_days: Option<u32>,
    /// 最小字节数
    #[serde(default)]
    pub min_size_bytes: Option<u64>,
    /// 最大字节数
    #[serde(default)]
    pub max_size_bytes: Option<u64>,
}

/// 任务动作。**只作用于文件，绝不删除目录**。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskAction {
    /// "delete" | "move"
    #[serde(default)]
    pub kind: String,
    /// 扫描的根目录绝对路径
    #[serde(default)]
    pub root: String,
    /// 是否递归子目录
    #[serde(default)]
    pub recursive: bool,
    /// move 的目标目录
    #[serde(default)]
    pub move_target: Option<String>,
    #[serde(default)]
    pub filters: TaskFilters,
}

/// 一条定时任务。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledTask {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub enabled: bool,
    pub schedule: TaskSchedule,
    pub action: TaskAction,
    /// Unix 毫秒
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub last_run: Option<i64>,
    /// 最近一次运行的中文结果摘要
    #[serde(default)]
    pub last_result: Option<String>,
    /// 下次触发时间（Unix 毫秒）
    #[serde(default)]
    pub next_run: Option<i64>,
    #[serde(default)]
    pub run_count: u64,
}

/// AI 规划出的任务草稿（无 id / 运行态字段，保存时由前端补全）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskDraft {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub schedule: TaskSchedule,
    #[serde(default)]
    pub action: TaskAction,
}

/// AI 任务规划回复。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskPlanReply {
    pub text: String,
    /// 解析成功时给出可填入表单的草稿
    #[serde(default)]
    pub draft: Option<TaskDraft>,
}

/// 预演（dry-run）结果。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskPreview {
    /// 命中的文件数
    pub count: usize,
    /// 命中文件总字节
    pub total_size: u64,
    /// 样例路径（最多 20 条）
    #[serde(default)]
    pub samples: Vec<String>,
    /// 命中数超过样例上限
    #[serde(default)]
    pub truncated: bool,
}

/// 意见反馈提交请求（前端表单 + 前端注入的版本 / commit）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackRequest {
    /// 反馈类型（Bug / 功能建议 / 其它）
    pub kind: String,
    pub title: String,
    pub description: String,
    /// 应用版本（前端 `__APP_VERSION__`）
    #[serde(default)]
    pub app_version: String,
    /// 构建 commit（前端 `__COMMIT_ID__`）
    #[serde(default)]
    pub commit: String,
}

/// 反馈提交结果（新建 issue 的编号与网页链接）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackCreated {
    pub number: String,
    pub url: String,
}

/// GitHub CLI 的可用性与登录状态，供反馈表单提示。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackAuth {
    /// 是否找到 GitHub CLI
    pub cli_found: bool,
    /// 是否已登录
    pub logged_in: bool,
    /// 登录用户名
    #[serde(default)]
    pub user: Option<String>,
    /// 面向用户的中文提示（含安装 / 登录指引）
    pub message: String,
}

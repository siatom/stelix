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
}

/// 一次文件操作请求
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsOp {
    pub op: FsOpKind,
    /// 源路径；Mkdir 时为空
    #[serde(default)]
    pub sources: Vec<String>,
    /// 目标路径：Move/Copy 为目标目录，Rename 为新名（含路径），Mkdir 为新目录路径
    #[serde(default)]
    pub target: Option<String>,
}

/// 文件操作结果
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsOpResult {
    pub ok: bool,
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

/// AI 配置（OpenAI 兼容）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiConfig {
    /// 例如 https://api.openai.com/v1
    #[serde(default = "default_base_url")]
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_model")]
    pub model: String,
    /// 关闭时走离线规则解析器
    #[serde(default)]
    pub enabled: bool,
}

fn default_base_url() -> String {
    "https://api.openai.com/v1".to_string()
}

fn default_model() -> String {
    "gpt-4o-mini".to_string()
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            base_url: default_base_url(),
            api_key: String::new(),
            model: default_model(),
            enabled: false,
        }
    }
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

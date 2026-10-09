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

//! Agent run 状态。
//!
//! 写操作需要用户确认，因此一轮对话不能像普通请求那样直接返回结束：
//! 后端保留模型消息栈、剩余轮次和待审批项，前端确认后从这里恢复继续执行。
//! run 只保存在内存中，并设置 TTL 与数量上限；进程重启后未完成的审批自动失效。

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use serde_json::Value;

use crate::file_locks;
use crate::types::{AiApproval, AiApprovalPreview, AiStep, AiToolCall, AiUsage, FsOp};

/// 单次对话最多调用工具/模型多少轮（含因审批挂起前后的轮次）。
pub const MAX_ROUNDS: usize = 6;
/// 挂起 run 的最长存活时间。
const RUN_TTL_MS: i64 = 60 * 60 * 1000;
/// 同时挂起的 run 上限，超出时淘汰最旧的。
const MAX_RUNS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunKind {
    /// 真实 LLM：审批后继续调用模型生成总结/下一步。
    Llm,
    /// 离线规则：审批后直接执行，不再调用模型。
    Offline,
}

/// 一轮挂起的对话。
pub struct AgentRun {
    pub id: String,
    pub kind: RunKind,
    pub request_id: String,
    /// 已经组装好的协议消息（含 assistant 的 tool_calls，等待补写 tool 结果）。
    pub api_messages: Vec<Value>,
    pub rounds_left: usize,
    pub last_text: String,
    pub usage: AiUsage,
    pub got_usage: bool,
    /// 本轮待审批的写操作。
    pub pending: Vec<AiApproval>,
    /// 本次 run 已执行的工具步骤。
    pub steps: Vec<AiStep>,
    /// 去重用的「工具名 + 参数」签名，避免模型反复发起同一个调用。
    pub seen_calls: HashSet<String>,
    pub created_at: i64,
}

impl AgentRun {
    pub fn new(id: String, kind: RunKind, request_id: String, api_messages: Vec<Value>) -> Self {
        Self {
            id,
            kind,
            request_id,
            api_messages,
            rounds_left: MAX_ROUNDS,
            last_text: String::new(),
            usage: AiUsage::default(),
            got_usage: false,
            pending: Vec::new(),
            steps: Vec::new(),
            seen_calls: HashSet::new(),
            created_at: now_ms(),
        }
    }

    /// 步骤列表只保留最近 20 条，避免历史文件无限膨胀。
    pub fn push_step(&mut self, step: AiStep) {
        self.steps.push(step);
        if self.steps.len() > 20 {
            let drop = self.steps.len() - 20;
            self.steps.drain(0..drop);
        }
    }
}

pub struct RunStore {
    runs: Mutex<HashMap<String, AgentRun>>,
}

impl RunStore {
    pub fn new() -> Self {
        Self {
            runs: Mutex::new(HashMap::new()),
        }
    }

    pub fn insert(&self, run: AgentRun) {
        let mut runs = self.runs.lock();
        expire_locked(&mut runs);
        if runs.len() >= MAX_RUNS {
            if let Some(oldest) = runs
                .iter()
                .min_by_key(|(_, value)| value.created_at)
                .map(|(id, _)| id.clone())
            {
                runs.remove(&oldest);
            }
        }
        runs.insert(run.id.clone(), run);
    }

    pub fn take(&self, id: &str) -> Option<AgentRun> {
        let mut runs = self.runs.lock();
        expire_locked(&mut runs);
        runs.remove(id)
    }

    pub fn remove(&self, id: &str) {
        self.runs.lock().remove(id);
    }
}

impl Default for RunStore {
    fn default() -> Self {
        Self::new()
    }
}

fn expire_locked(runs: &mut HashMap<String, AgentRun>) {
    let now = now_ms();
    runs.retain(|_, run| now - run.created_at < RUN_TTL_MS);
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

/// 构造一条待审批项，并顺带生成 dry-run 预览。
pub fn make_approval(call: &AiToolCall, op: FsOp, description: String) -> AiApproval {
    let preview = preview_op(&op);
    AiApproval {
        id: uuid::Uuid::new_v4().to_string(),
        call: call.clone(),
        op,
        description,
        preview,
    }
}

/// 只读检查操作影响面：不执行任何写操作。
pub fn preview_op(op: &FsOp) -> AiApprovalPreview {
    let mut preview = AiApprovalPreview::default();
    let sources = op.sources();
    preview.items = match op {
        FsOp::Mkdir { targets } => targets.len(),
        FsOp::CreateFile { .. } => 1,
        _ => sources.len(),
    };

    let mut warnings: Vec<String> = Vec::new();
    match op {
        FsOp::Move {
            sources, target, ..
        }
        | FsOp::Copy {
            sources, target, ..
        } => {
            if !target.is_empty() {
                let path = Path::new(target);
                if !path.exists() {
                    warnings.push(format!("目标目录不存在：{target}"));
                } else if !path.is_dir() {
                    warnings.push(format!("目标不是目录：{target}"));
                }
            }
            if let Some(source) = sources.first() {
                if drive_of(source).is_some() && drive_of(source) != drive_of(target) {
                    preview.cross_drive = true;
                    warnings.push("跨盘操作可能耗时较长".into());
                }
            }
        }
        FsOp::Rename { target, .. } => {
            preview.target_exists = Path::new(target).exists();
            if preview.target_exists {
                warnings.push(format!("目标已存在：{target}"));
            }
        }
        FsOp::Delete { permanent, .. } => {
            if *permanent {
                warnings.push("此为永久删除，无法从回收站恢复".into());
            } else {
                warnings.push("将移到系统回收站，可在回收站还原".into());
            }
        }
        FsOp::Mkdir { targets } => {
            preview.target_exists = targets.iter().any(|target| Path::new(target).exists());
            if preview.target_exists {
                warnings.push("部分目标目录已存在".into());
            }
        }
        FsOp::CreateFile { target } => {
            preview.target_exists = Path::new(target).exists();
            if preview.target_exists {
                warnings.push("目标文件已存在".into());
            }
        }
        FsOp::Duplicate { .. } => {}
    }

    // 占用检查最多取 20 项，避免一次删除几千个文件时卡住 UI。
    for path in sources.iter().take(20) {
        let locked = file_locks::locking_processes(path);
        if !locked.is_empty() {
            preview
                .locked
                .push(format!("{}（{}）", path, locked.join("、")));
        }
    }
    if !preview.locked.is_empty() {
        warnings.push(format!(
            "{} 个文件正被其他程序占用，执行可能失败",
            preview.locked.len()
        ));
    }

    preview.warnings = warnings;
    preview
}

fn drive_of(path: &str) -> Option<u8> {
    let bytes = path.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' {
        Some(bytes[0].to_ascii_uppercase())
    } else {
        None
    }
}

/// 按字符（而非字节）截断，避免切断 UTF-8。
pub fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_keeps_utf8_boundaries() {
        assert_eq!(truncate_chars("你好世界", 10), "你好世界");
        assert_eq!(truncate_chars("你好世界", 3), "你好…");
        assert_eq!(truncate_chars("", 2), "");
    }

    #[test]
    fn preview_reports_missing_target_and_permanent_delete() {
        let missing = preview_op(&FsOp::Move {
            sources: vec!["C:\\a.txt".into()],
            target: "Z:\\not-exist".into(),
            conflict: crate::types::ConflictPolicy::KeepBoth,
        });
        assert_eq!(missing.items, 1);
        assert!(missing
            .warnings
            .iter()
            .any(|w| w.contains("目标目录不存在")));

        let permanent = preview_op(&FsOp::Delete {
            sources: vec!["C:\\a.txt".into()],
            permanent: true,
        });
        assert!(permanent.warnings.iter().any(|w| w.contains("永久删除")));
    }

    #[test]
    fn run_store_evicts_and_takes_once() {
        let store = RunStore::new();
        let run = AgentRun::new("run-1".into(), RunKind::Offline, "req".into(), Vec::new());
        store.insert(run);
        assert!(store.take("run-1").is_some());
        assert!(store.take("run-1").is_none());
    }
}

//! 定时任务：确定性规格 + 后台调度。
//!
//! - 规格（[`ScheduledTask`]）由 AI 在创建时解析，或由用户手填；**运行时不再调用 LLM**。
//! - 执行器只作用于**文件**（绝不删除目录），并支持 dry-run 预演。
//! - 调度线程每 [`POLL_INTERVAL`] 检查一次到期任务。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{Datelike, Days, Local, LocalResult, TimeZone};
use parking_lot::RwLock;
use tauri::{AppHandle, Emitter};
use walkdir::WalkDir;

use crate::persistence;
use crate::types::{ScheduledTask, TaskAction, TaskFilters, TaskPreview, TaskSchedule};

/// 任务状态变化事件（前端据此刷新列表）。
pub const TASK_UPDATED_EVENT: &str = "task:updated";
/// 调度轮询间隔。
const POLL_INTERVAL: Duration = Duration::from_secs(20);
/// 预演最多返回的样例条数。
const PREVIEW_SAMPLE_LIMIT: usize = 20;
/// 间隔分钟数上界（一年），避免超大间隔导致下次触发时间溢出或实际上永不到达。
const MAX_INTERVAL_MINUTES: u64 = 525_600;

/// 当前 Unix 毫秒时间戳。
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 定时任务服务：持有任务列表与（可选的）AppHandle，供后台线程广播事件。
pub struct ScheduleService {
    tasks: RwLock<Vec<ScheduledTask>>,
    app: RwLock<Option<AppHandle>>,
}

impl ScheduleService {
    pub fn new() -> Self {
        let mut tasks = persistence::load_tasks();
        let now = now_ms();
        for task in &mut tasks {
            // 恢复旧版本在夏令时切换后丢失下次触发时间的任务。
            if task.enabled
                && task.next_run.is_none()
                && matches!(task.schedule.kind.as_str(), "daily" | "weekly" | "monthly")
                && validate(task).is_ok()
            {
                task.next_run = compute_next_run(&task.schedule, now);
            }
        }
        Self {
            tasks: RwLock::new(tasks),
            app: RwLock::new(None),
        }
    }

    pub fn list(&self) -> Vec<ScheduledTask> {
        self.tasks.read().clone()
    }

    /// 新增或更新（按 id 匹配），并重算下次触发时间。
    pub fn save(&self, mut task: ScheduledTask) -> Result<ScheduledTask, String> {
        validate(&task)?;
        let now = now_ms();
        if task.id.trim().is_empty() {
            task.id = format!("task-{now}");
        }
        if task.created_at <= 0 {
            task.created_at = now;
        }
        task.next_run = if task.enabled {
            compute_next_run(&task.schedule, now)
        } else {
            None
        };
        {
            let mut tasks = self.tasks.write();
            match tasks.iter_mut().find(|t| t.id == task.id) {
                Some(existing) => *existing = task.clone(),
                None => tasks.push(task.clone()),
            }
            tasks.sort_by_key(|t| t.created_at);
        }
        self.persist()?;
        Ok(task)
    }

    pub fn delete(&self, id: &str) -> Result<(), String> {
        self.tasks.write().retain(|t| t.id != id);
        self.persist()
    }

    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<(), String> {
        {
            let now = now_ms();
            let mut tasks = self.tasks.write();
            if let Some(task) = tasks.iter_mut().find(|t| t.id == id) {
                task.enabled = enabled;
                task.next_run = if enabled {
                    compute_next_run(&task.schedule, now)
                } else {
                    None
                };
            }
        }
        self.persist()
    }

    /// 立即运行一次。
    pub fn run_now(&self, id: &str) -> Result<String, String> {
        let task = self
            .tasks
            .read()
            .iter()
            .find(|t| t.id == id)
            .cloned()
            .ok_or_else(|| format!("任务不存在：{id}"))?;
        let summary = execute(&task);
        self.record_run(id, &summary);
        Ok(summary)
    }

    /// 预演：只统计命中的文件，不产生任何副作用。
    pub fn preview(&self, action: &TaskAction) -> TaskPreview {
        let matches = scan_matches(action);
        let total_size = matches.iter().map(|(_, size)| *size).sum();
        let truncated = matches.len() > PREVIEW_SAMPLE_LIMIT;
        let samples = matches
            .iter()
            .take(PREVIEW_SAMPLE_LIMIT)
            .map(|(path, _)| path.to_string_lossy().to_string())
            .collect();
        TaskPreview {
            count: matches.len(),
            total_size,
            samples,
            truncated,
        }
    }

    /// 记录一次运行结果、重算下次触发并持久化，随后广播事件。
    fn record_run(&self, id: &str, summary: &str) {
        let now = now_ms();
        {
            let mut tasks = self.tasks.write();
            if let Some(task) = tasks.iter_mut().find(|t| t.id == id) {
                task.last_run = Some(now);
                task.last_result = Some(summary.to_string());
                task.run_count += 1;
                task.next_run = if task.enabled {
                    compute_next_run(&task.schedule, now)
                } else {
                    None
                };
            }
        }
        let _ = self.persist();
        if let Some(app) = self.app.read().clone() {
            let _ = app.emit(TASK_UPDATED_EVENT, id.to_string());
        }
    }

    fn persist(&self) -> Result<(), String> {
        persistence::save_tasks(&self.tasks.read())
    }

    /// 启动后台调度线程（每 [`POLL_INTERVAL`] 检查一次到期任务）。
    pub fn start_loop(self: Arc<Self>, app: AppHandle) {
        *self.app.write() = Some(app);
        std::thread::spawn(move || loop {
            std::thread::sleep(POLL_INTERVAL);
            let now = now_ms();
            let due: Vec<ScheduledTask> = self
                .tasks
                .read()
                .iter()
                .filter(|t| t.enabled && t.next_run.is_some_and(|next| next <= now))
                .cloned()
                .collect();
            for task in due {
                let summary = execute(&task);
                self.record_run(&task.id, &summary);
            }
        });
    }
}

impl Default for ScheduleService {
    fn default() -> Self {
        Self::new()
    }
}

// ================================================================ 校验

/// 是否存在**真正起约束作用**的筛选条件。
///
/// 仅 `Some(0)` / `Some(u64::MAX)` / 空串等退化的「空约束」不算数，否则
/// 「至少一个筛选条件」这条防误删护栏会被绕过（例如 `minAgeDays: 0` 会匹配全部文件）。
fn has_any_filter(filters: &TaskFilters) -> bool {
    filters.extensions.iter().any(|e| !e.trim().is_empty())
        || filters
            .name_contains
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
        || filters.min_age_days.is_some_and(|days| days > 0)
        || filters.min_size_bytes.is_some_and(|bytes| bytes > 0)
        || filters.max_size_bytes.is_some_and(|bytes| bytes < u64::MAX)
}

fn validate(task: &ScheduledTask) -> Result<(), String> {
    if task.name.trim().is_empty() {
        return Err("任务名称不能为空".into());
    }
    if task.action.root.trim().is_empty() {
        return Err("目标目录不能为空".into());
    }
    match task.action.kind.as_str() {
        "delete" => {}
        "move" => {
            if task
                .action
                .move_target
                .as_deref()
                .is_none_or(|s| s.trim().is_empty())
            {
                return Err("「移动到」任务需要指定目标目录".into());
            }
        }
        other => return Err(format!("不支持的动作类型：{other}")),
    }
    // 安全护栏：无任何筛选条件时递归扫描可能匹配整棵目录树 → 拒绝，避免误删。
    if !has_any_filter(&task.action.filters) {
        return Err(
            "为避免误删，请至少指定一个筛选条件（扩展名 / 文件名包含 / 大小 / 修改时间）".into(),
        );
    }
    match task.schedule.kind.as_str() {
        "interval" => match task.schedule.interval_minutes {
            Some(minutes) if (1..=MAX_INTERVAL_MINUTES).contains(&minutes) => {}
            Some(_) => {
                return Err(format!("间隔分钟数需在 1-{MAX_INTERVAL_MINUTES} 之间"));
            }
            None => return Err("间隔分钟数必须大于 0".into()),
        },
        "daily" => validate_clock(&task.schedule)?,
        "weekly" => {
            validate_clock(&task.schedule)?;
            if task.schedule.weekday.is_some_and(|w| w > 6) {
                return Err("星期需在 0（周日）到 6（周六）之间".into());
            }
        }
        "monthly" => {
            validate_clock(&task.schedule)?;
            if task
                .schedule
                .day_of_month
                .is_some_and(|d| !(1..=28).contains(&d))
            {
                return Err("每月日期需在 1 到 28 之间".into());
            }
        }
        other => return Err(format!("不支持的触发类型：{other}")),
    }
    Ok(())
}

/// 校验小时 / 分钟取值范围，避免非法时间导致 `compute_next_run` 静默返回 `None`。
fn validate_clock(schedule: &TaskSchedule) -> Result<(), String> {
    if schedule.hour.is_some_and(|h| h > 23) {
        return Err("小时需在 0 到 23 之间".into());
    }
    if schedule.minute.is_some_and(|m| m > 59) {
        return Err("分钟需在 0 到 59 之间".into());
    }
    Ok(())
}

// ================================================================ 下次触发时间

fn compute_next_run(schedule: &TaskSchedule, from_ms: i64) -> Option<i64> {
    let from = Local.timestamp_millis_opt(from_ms).earliest()?;
    match schedule.kind.as_str() {
        "interval" => {
            let minutes = schedule.interval_minutes.unwrap_or(60).max(1);
            let delta = i64::try_from(minutes).ok()?.checked_mul(60_000)?;
            from_ms.checked_add(delta)
        }
        "daily" => next_occurrence(
            from,
            schedule.hour.unwrap_or(0),
            schedule.minute.unwrap_or(0),
            None,
            None,
        ),
        "weekly" => next_occurrence(
            from,
            schedule.hour.unwrap_or(0),
            schedule.minute.unwrap_or(0),
            schedule.weekday,
            None,
        ),
        "monthly" => next_occurrence(
            from,
            schedule.hour.unwrap_or(0),
            schedule.minute.unwrap_or(0),
            None,
            schedule.day_of_month,
        ),
        _ => None,
    }
}

/// 求 `from` 之后（严格大于）最近一次满足条件的本地时间点。
fn next_occurrence(
    from: chrono::DateTime<Local>,
    hour: u8,
    minute: u8,
    weekday: Option<u8>,
    day_of_month: Option<u8>,
) -> Option<i64> {
    let start_date = from.date_naive();
    for offset in 0..=366 {
        let date = start_date.checked_add_days(Days::new(offset))?;
        if let Some(target) = weekday {
            if date.weekday().num_days_from_sunday() as u8 != target {
                continue;
            }
        }
        if let Some(target) = day_of_month {
            if date.day() != target as u32 {
                continue;
            }
        }
        let naive = date.and_hms_opt(hour as u32, minute as u32, 0)?;
        let candidates = match Local.from_local_datetime(&naive) {
            LocalResult::None => continue,
            LocalResult::Single(time) => [Some(time), None],
            LocalResult::Ambiguous(first, second) => [Some(first), Some(second)],
        };
        // 往返转换排除切换边界处无效的偏移，再按绝对时间选第一次。
        let earliest = candidates
            .into_iter()
            .flatten()
            .map(|time| time.timestamp_millis())
            .filter(|&ms| {
                Local
                    .timestamp_millis_opt(ms)
                    .single()
                    .is_some_and(|time| time.naive_local() == naive)
            })
            .min();
        if let Some(ms) = earliest.filter(|&ms| ms > from.timestamp_millis()) {
            return Some(ms);
        }
    }
    None
}

// ================================================================ 执行

/// 执行一条任务，返回中文结果摘要。
fn execute(task: &ScheduledTask) -> String {
    match run_action(&task.action) {
        Ok((count, bytes, errors)) => {
            let verb = if task.action.kind == "move" {
                "移动"
            } else {
                "移到回收站"
            };
            let mut summary = format!("已{verb} {count} 个文件（{}）", human_size(bytes));
            if errors > 0 {
                summary.push_str(&format!("，{errors} 个失败"));
            }
            summary
        }
        Err(e) => format!("执行失败：{e}"),
    }
}

fn run_action(action: &TaskAction) -> Result<(usize, u64, usize), String> {
    if action.kind == "move"
        && action
            .move_target
            .as_deref()
            .is_none_or(|s| s.trim().is_empty())
    {
        return Err("未指定移动目标目录".into());
    }
    let matches = scan_matches(action);
    let mut ok = 0usize;
    let mut bytes = 0u64;
    let mut errors = 0usize;
    for (path, size) in &matches {
        let result: Result<(), String> = if action.kind == "move" {
            move_file(path, action.move_target.as_deref().unwrap_or(""))
                .map_err(|error| error.to_string())
        } else {
            // 与手动删除保持一致：默认移到系统回收站，避免定时任务静默永久删除。
            crate::fs_core::trash_path(path).map_err(|error| error.to_string())
        };
        match result {
            Ok(()) => {
                ok += 1;
                bytes += size;
            }
            Err(_) => errors += 1,
        }
    }
    Ok((ok, bytes, errors))
}

/// 扫描根目录，返回满足筛选条件的文件（路径, 字节数）。不产生副作用。
fn scan_matches(action: &TaskAction) -> Vec<(PathBuf, u64)> {
    let mut out = Vec::new();
    let root = Path::new(&action.root);
    if !root.is_dir() {
        return out;
    }
    let now = now_ms();
    let mut walker = WalkDir::new(root).min_depth(1).follow_links(false);
    if !action.recursive {
        walker = walker.max_depth(1);
    }
    for entry in walker.into_iter().filter_map(Result::ok) {
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        if matches_filters(entry.path(), &meta, &action.filters, now) {
            out.push((entry.path().to_path_buf(), meta.len()));
        }
    }
    out
}

fn matches_filters(path: &Path, meta: &std::fs::Metadata, filters: &TaskFilters, now: i64) -> bool {
    if !filters.extensions.is_empty() {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !filters
            .extensions
            .iter()
            .any(|want| want.trim_start_matches('.').eq_ignore_ascii_case(&ext))
        {
            return false;
        }
    }
    if let Some(needle) = filters
        .name_contains
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !name.to_lowercase().contains(&needle.to_lowercase()) {
            return false;
        }
    }
    if let Some(days) = filters.min_age_days {
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        if modified > now - days as i64 * 86_400_000 {
            return false;
        }
    }
    if let Some(min) = filters.min_size_bytes {
        if meta.len() < min {
            return false;
        }
    }
    if let Some(max) = filters.max_size_bytes {
        if meta.len() > max {
            return false;
        }
    }
    true
}

/// 移动文件到目标目录；同名冲突时追加序号，跨卷回退为「复制 + 删除」。
fn move_file(src: &Path, target_dir: &str) -> std::io::Result<()> {
    let dir = Path::new(target_dir);
    std::fs::create_dir_all(dir)?;
    let name = src
        .file_name()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "非法文件名"))?;
    let mut dest = dir.join(name);
    if dest.exists() {
        let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
        let ext = src.extension().and_then(|s| s.to_str());
        for i in 1..10_000 {
            let candidate = match ext {
                Some(e) => format!("{stem} ({i}).{e}"),
                None => format!("{stem} ({i})"),
            };
            let path = dir.join(candidate);
            if !path.exists() {
                dest = path;
                break;
            }
        }
    }
    std::fs::rename(src, &dest)
        .or_else(|_| std::fs::copy(src, &dest).and_then(|_| std::fs::remove_file(src)))
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Timelike;

    fn task_with(schedule: TaskSchedule, action: TaskAction) -> ScheduledTask {
        ScheduledTask {
            id: String::new(),
            name: "测试任务".into(),
            enabled: false,
            schedule,
            action,
            created_at: 0,
            last_run: None,
            last_result: None,
            next_run: None,
            run_count: 0,
        }
    }

    fn daily_schedule() -> TaskSchedule {
        TaskSchedule {
            kind: "daily".into(),
            hour: Some(1),
            minute: Some(0),
            ..Default::default()
        }
    }

    fn delete_action(filters: TaskFilters) -> TaskAction {
        TaskAction {
            kind: "delete".into(),
            root: "D:\\tmp".into(),
            recursive: true,
            move_target: None,
            filters,
        }
    }

    #[test]
    fn interval_adds_minutes() {
        let schedule = TaskSchedule {
            kind: "interval".into(),
            interval_minutes: Some(15),
            ..Default::default()
        };
        assert_eq!(
            compute_next_run(&schedule, 1_000_000).unwrap(),
            1_000_000 + 15 * 60_000
        );
    }

    #[test]
    fn daily_rolls_to_next_day_when_time_passed() {
        let from = Local.with_ymd_and_hms(2026, 1, 1, 10, 0, 0).unwrap();
        let schedule = TaskSchedule {
            kind: "daily".into(),
            hour: Some(9),
            minute: Some(0),
            ..Default::default()
        };
        let next = Local
            .timestamp_millis_opt(compute_next_run(&schedule, from.timestamp_millis()).unwrap())
            .earliest()
            .unwrap();
        assert_eq!(
            next.date_naive(),
            chrono::NaiveDate::from_ymd_opt(2026, 1, 2).unwrap()
        );
        assert_eq!(next.hour(), 9);
    }

    #[cfg(unix)]
    #[test]
    fn calendar_rules_across_daylight_saving_transitions() {
        use chrono::Utc;
        use std::process::Command;

        // 在子进程中设置时区，避免修改并行测试使用的进程环境。
        const CHILD: &str = "STELIX_TEST_DST_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let status = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "schedule::tests::calendar_rules_across_daylight_saving_transitions",
                ])
                .env(CHILD, "1")
                .env("TZ", "America/New_York")
                .status()
                .unwrap();
            assert!(status.success());
            return;
        }

        let utc = |month, day, hour, minute| {
            Utc.with_ymd_and_hms(2026, month, day, hour, minute, 0)
                .unwrap()
                .timestamp_millis()
        };
        let mut schedule = TaskSchedule {
            hour: Some(2),
            minute: Some(30),
            ..daily_schedule()
        };
        let before_gap = utc(3, 8, 5, 0);
        for (kind, weekday, day_of_month, expected) in [
            ("daily", None, None, utc(3, 9, 6, 30)),
            ("weekly", Some(0), None, utc(3, 15, 6, 30)),
            ("monthly", None, Some(8), utc(4, 8, 6, 30)),
        ] {
            schedule.kind = kind.into();
            schedule.weekday = weekday;
            schedule.day_of_month = day_of_month;
            assert_eq!(compute_next_run(&schedule, before_gap), Some(expected));
        }

        schedule.kind = "daily".into();
        schedule.minute = Some(0);
        assert_eq!(
            compute_next_run(&schedule, before_gap),
            Some(utc(3, 9, 6, 0))
        );

        schedule.hour = Some(3);
        schedule.minute = Some(30);
        // 前一天深夜加 24 小时会越过切换日的午夜。
        assert_eq!(
            compute_next_run(&schedule, utc(3, 8, 4, 45)),
            Some(utc(3, 8, 7, 30))
        );

        schedule.hour = Some(1);
        let first = utc(11, 1, 5, 30);
        assert_eq!(compute_next_run(&schedule, utc(11, 1, 5, 0)), Some(first));
        for from in [first, utc(11, 1, 6, 0)] {
            assert_eq!(compute_next_run(&schedule, from), Some(utc(11, 2, 6, 30)));
        }

        schedule.kind = "interval".into();
        schedule.interval_minutes = Some(60);
        assert_eq!(
            compute_next_run(&schedule, utc(3, 8, 6, 30)),
            Some(utc(3, 8, 7, 30))
        );
    }

    #[test]
    fn weekly_picks_target_weekday() {
        // 2026-01-01 为周四
        let from = Local.with_ymd_and_hms(2026, 1, 1, 10, 0, 0).unwrap();
        let schedule = TaskSchedule {
            kind: "weekly".into(),
            hour: Some(8),
            minute: Some(30),
            weekday: Some(1),
            ..Default::default()
        };
        let next = Local
            .timestamp_millis_opt(compute_next_run(&schedule, from.timestamp_millis()).unwrap())
            .earliest()
            .unwrap();
        assert_eq!(next.weekday().num_days_from_sunday(), 1);
        assert_eq!(next.hour(), 8);
    }

    #[test]
    fn validate_rejects_missing_filter() {
        let task = task_with(
            TaskSchedule {
                kind: "daily".into(),
                hour: Some(1),
                minute: Some(0),
                ..Default::default()
            },
            TaskAction {
                kind: "delete".into(),
                root: "D:\\tmp".into(),
                recursive: true,
                move_target: None,
                filters: TaskFilters::default(),
            },
        );
        assert!(validate(&task).is_err());
    }

    #[test]
    fn validate_accepts_extension_filter() {
        let task = task_with(
            TaskSchedule {
                kind: "daily".into(),
                hour: Some(1),
                minute: Some(0),
                ..Default::default()
            },
            TaskAction {
                kind: "delete".into(),
                root: "D:\\tmp".into(),
                recursive: true,
                move_target: None,
                filters: TaskFilters {
                    extensions: vec!["tmp".into()],
                    ..Default::default()
                },
            },
        );
        assert!(validate(&task).is_ok());
    }

    #[test]
    fn validate_rejects_degenerate_filters() {
        // 退化筛选（0 / u64::MAX / 空串）实际不约束任何文件，必须被拒绝，否则护栏形同虚设。
        let cases = [
            TaskFilters {
                min_age_days: Some(0),
                ..Default::default()
            },
            TaskFilters {
                min_size_bytes: Some(0),
                ..Default::default()
            },
            TaskFilters {
                max_size_bytes: Some(u64::MAX),
                ..Default::default()
            },
            TaskFilters {
                extensions: vec![String::new(), "  ".into()],
                ..Default::default()
            },
        ];
        for filters in cases {
            let task = task_with(daily_schedule(), delete_action(filters.clone()));
            assert!(validate(&task).is_err(), "退化筛选应被拒绝：{filters:?}");
        }
    }

    #[test]
    fn validate_accepts_effective_filters() {
        let cases = [
            TaskFilters {
                min_age_days: Some(1),
                ..Default::default()
            },
            TaskFilters {
                min_size_bytes: Some(1),
                ..Default::default()
            },
            TaskFilters {
                max_size_bytes: Some(u64::MAX - 1),
                ..Default::default()
            },
            TaskFilters {
                name_contains: Some("log".into()),
                ..Default::default()
            },
        ];
        for filters in cases {
            let task = task_with(daily_schedule(), delete_action(filters.clone()));
            assert!(validate(&task).is_ok(), "有效筛选应通过：{filters:?}");
        }
    }

    #[test]
    fn validate_rejects_out_of_range_schedule() {
        let action = delete_action(TaskFilters {
            extensions: vec!["tmp".into()],
            ..Default::default()
        });
        let cases = [
            TaskSchedule {
                kind: "interval".into(),
                interval_minutes: None,
                ..Default::default()
            },
            TaskSchedule {
                kind: "interval".into(),
                interval_minutes: Some(0),
                ..Default::default()
            },
            TaskSchedule {
                kind: "interval".into(),
                interval_minutes: Some(MAX_INTERVAL_MINUTES + 1),
                ..Default::default()
            },
            TaskSchedule {
                kind: "daily".into(),
                hour: Some(25),
                minute: Some(0),
                ..Default::default()
            },
            TaskSchedule {
                kind: "daily".into(),
                hour: Some(9),
                minute: Some(99),
                ..Default::default()
            },
            TaskSchedule {
                kind: "weekly".into(),
                weekday: Some(7),
                ..Default::default()
            },
            TaskSchedule {
                kind: "monthly".into(),
                day_of_month: Some(0),
                ..Default::default()
            },
            TaskSchedule {
                kind: "monthly".into(),
                day_of_month: Some(32),
                ..Default::default()
            },
        ];
        for schedule in cases {
            let task = task_with(schedule.clone(), action.clone());
            assert!(
                validate(&task).is_err(),
                "非法触发时间应被拒绝：{schedule:?}"
            );
        }
    }

    #[test]
    fn validate_accepts_boundary_schedule() {
        let action = delete_action(TaskFilters {
            extensions: vec!["tmp".into()],
            ..Default::default()
        });
        let cases = [
            TaskSchedule {
                kind: "interval".into(),
                interval_minutes: Some(MAX_INTERVAL_MINUTES),
                ..Default::default()
            },
            TaskSchedule {
                kind: "daily".into(),
                hour: Some(23),
                minute: Some(59),
                ..Default::default()
            },
            TaskSchedule {
                kind: "weekly".into(),
                hour: Some(0),
                minute: Some(0),
                weekday: Some(6),
                ..Default::default()
            },
            TaskSchedule {
                kind: "monthly".into(),
                day_of_month: Some(28),
                ..Default::default()
            },
        ];
        for schedule in cases {
            let task = task_with(schedule.clone(), action.clone());
            assert!(validate(&task).is_ok(), "边界值应通过：{schedule:?}");
        }
    }

    #[test]
    fn compute_next_run_interval_overflow_returns_none() {
        // u64 超出 i64 表示范围：必须返回 None，而不是 panic / 回绕。
        let too_large = TaskSchedule {
            kind: "interval".into(),
            interval_minutes: Some(u64::MAX),
            ..Default::default()
        };
        assert_eq!(compute_next_run(&too_large, 1_000_000), None);

        // 可转换为 i64 但乘法溢出：同样必须返回 None。
        let overflowing = TaskSchedule {
            kind: "interval".into(),
            interval_minutes: Some(1_000_000_000_000_000),
            ..Default::default()
        };
        assert_eq!(compute_next_run(&overflowing, 1_000_000), None);

        let near_max = TaskSchedule {
            kind: "interval".into(),
            interval_minutes: Some(MAX_INTERVAL_MINUTES),
            ..Default::default()
        };
        assert!(compute_next_run(&near_max, 1_000_000).is_some());
    }

    #[test]
    fn scan_matches_respects_extension_filter() {
        let dir = std::env::temp_dir().join(format!("stelix-task-test-{}", now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.tmp"), b"x").unwrap();
        std::fs::write(dir.join("b.txt"), b"y").unwrap();
        let action = TaskAction {
            kind: "delete".into(),
            root: dir.to_string_lossy().to_string(),
            recursive: false,
            move_target: None,
            filters: TaskFilters {
                extensions: vec!["tmp".into()],
                ..Default::default()
            },
        };
        let matches = scan_matches(&action);
        assert_eq!(matches.len(), 1);
        assert!(matches[0].0.ends_with("a.tmp"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn human_size_formats_units() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(2048), "2.0 KB");
    }
}

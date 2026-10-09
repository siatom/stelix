//! 查询解析与匹配 —— 由 **WS-A (feat/index-core)** 实现。
//!
//! 契约（Phase 0 冻结）：`parse_query` / `matches` / `score` 三个纯函数。
//!
//! 支持语法（大小写不敏感，空格分隔，多关键词取 AND）：
//! - `ext:pdf` / `ext:pdf,docx`          扩展名白名单（可逗号分隔、可带点）
//! - `size:>100mb` / `size:<1gb` / `size:100kb-10mb`
//!   比较符 `>`/`<`/`>=`/`<=`，区间用 `-`；单位支持 b/kb/mb/gb/tb
//! - `modified:today` / `modified:7d` / `modified:24h` / `modified:2024-01-01`
//! - `dir:` 仅目录；`file:` 仅文件
//! - 其余 token 拼成 `text`（文件名子串匹配）

use std::path::Path;

use chrono::{Duration, Local, NaiveDate};

use crate::types::{FileEntry, SearchQuery};

/// 与 types.rs 的 serde 默认值保持一致（`SearchQuery::default()` 的 limit 为 0）。
pub const DEFAULT_LIMIT: usize = 500;

/// 把用户输入（或 AI 生成的关键词）解析为结构化查询。
/// 支持形如 `ext:pdf size:>100mb modified:today 报告` 的语法。
pub fn parse_query(text: &str) -> SearchQuery {
    let mut q = SearchQuery {
        limit: DEFAULT_LIMIT,
        ..Default::default()
    };
    let mut terms: Vec<String> = Vec::new();

    for raw in text.split_whitespace() {
        let lower = raw.to_lowercase();

        if let Some(v) = lower.strip_prefix("ext:") {
            for part in v.split(',') {
                let p = part.trim().trim_start_matches('.').to_string();
                if !p.is_empty() {
                    q.ext.push(p);
                }
            }
            continue;
        }

        if let Some(v) = lower.strip_prefix("size:") {
            apply_size(&mut q, v);
            continue;
        }

        if let Some(v) = lower.strip_prefix("modified:") {
            apply_modified(&mut q, v);
            continue;
        }

        if lower == "dir:" {
            q.is_dir = Some(true);
            continue;
        }
        if lower == "file:" {
            q.is_dir = Some(false);
            continue;
        }

        // 其余作为普通关键词（保留原始大小写，便于展示）
        terms.push(raw.to_string());
    }

    q.text = terms.join(" ");
    q
}

/// 解析 `size:` 参数：`>100mb` / `<1gb` / `100kb-10mb` / `>=1mb` / `<=2mb`
fn apply_size(q: &mut SearchQuery, v: &str) {
    let v = v.trim();
    if v.is_empty() {
        return;
    }

    // 区间：100kb-10mb
    if let Some((lo, hi)) = v.split_once('-') {
        if let Some(lo) = parse_size(lo) {
            q.min_size = Some(lo);
        }
        if let Some(hi) = parse_size(hi) {
            q.max_size = Some(hi);
        }
        return;
    }

    if let Some(rest) = v.strip_prefix(">=") {
        if let Some(x) = parse_size(rest) {
            q.min_size = Some(x);
        }
    } else if let Some(rest) = v.strip_prefix("<=") {
        if let Some(x) = parse_size(rest) {
            q.max_size = Some(x);
        }
    } else if let Some(rest) = v.strip_prefix('>') {
        // 严格大于：min_size = x + 1（matches 用 >= 判定）
        if let Some(x) = parse_size(rest) {
            q.min_size = Some(x.saturating_add(1));
        }
    } else if let Some(rest) = v.strip_prefix('<') {
        // 严格小于：max_size = x - 1（matches 用 <= 判定）
        if let Some(x) = parse_size(rest) {
            q.max_size = Some(x.saturating_sub(1));
        }
    } else if let Some(x) = parse_size(v) {
        // 裸尺寸视为精确值
        q.min_size = Some(x);
        q.max_size = Some(x);
    }
}

/// 解析尺寸字面量（支持 b/kb/mb/gb/tb，允许小数如 `1.5gb`）
fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim().to_lowercase();
    if s.is_empty() {
        return None;
    }
    let digits: String = s
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    if digits.is_empty() {
        return None;
    }
    let num: f64 = digits.parse().ok()?;
    let unit = s[digits.len()..].trim();
    let mult = match unit {
        "" | "b" => 1f64,
        "kb" => 1024f64,
        "mb" => 1024f64 * 1024f64,
        "gb" => 1024f64 * 1024f64 * 1024f64,
        "tb" => 1024f64 * 1024f64 * 1024f64 * 1024f64,
        _ => return None,
    };
    Some((num * mult) as u64)
}

/// 解析 `modified:` 参数：`today` / `yesterday` / `7d` / `24h` / `30m` / `2024-01-01`
fn apply_modified(q: &mut SearchQuery, v: &str) {
    let v = v.trim();
    if v.is_empty() {
        return;
    }

    let now = Local::now();
    let after = if v == "today" {
        Some(local_midnight(now.date_naive()))
    } else if v == "yesterday" {
        Some(local_midnight((now - Duration::days(1)).date_naive()))
    } else if let Some(dur) = parse_duration(v) {
        Some((now - dur).timestamp_millis())
    } else if let Ok(d) = NaiveDate::parse_from_str(v, "%Y-%m-%d") {
        Some(local_midnight(d))
    } else {
        None
    };

    if let Some(ms) = after {
        q.modified_after = Some(ms);
    }
}

/// 解析相对时长：`7d` / `24h` / `30m` / `2w`
fn parse_duration(v: &str) -> Option<Duration> {
    let unit = v.chars().last()?;
    if !unit.is_ascii_alphabetic() {
        return None;
    }
    let num = &v[..v.len() - unit.len_utf8()];
    let n: i64 = num.parse().ok()?;
    match unit {
        'd' => Some(Duration::days(n)),
        'h' => Some(Duration::hours(n)),
        'm' => Some(Duration::minutes(n)),
        'w' => Some(Duration::weeks(n)),
        _ => None,
    }
}

/// 某本地日期 00:00 的 Unix 毫秒时间戳
fn local_midnight(d: NaiveDate) -> i64 {
    d.and_hms_opt(0, 0, 0)
        .and_then(|dt| dt.and_local_timezone(Local).earliest())
        .map(|dt| dt.timestamp_millis())
        .unwrap_or(0)
}

/// 条目是否满足查询条件（所有条件取 AND；text 内多关键词也取 AND）
pub fn matches(entry: &FileEntry, query: &SearchQuery) -> bool {
    // 路径范围
    if let Some(scope) = &query.scope {
        let scope = scope
            .trim_end_matches(['\\', '/'])
            .to_lowercase();
        if !scope.is_empty() && !entry.path.to_lowercase().starts_with(&scope) {
            return false;
        }
    }

    // 文件 / 目录
    if let Some(is_dir) = query.is_dir {
        if entry.is_dir != is_dir {
            return false;
        }
    }

    // 扩展名白名单（目录无扩展名，有 ext 过滤时自动排除）
    if !query.ext.is_empty() {
        match &entry.ext {
            Some(e) => {
                if !query.ext.iter().any(|x| x.eq_ignore_ascii_case(e)) {
                    return false;
                }
            }
            None => return false,
        }
    }

    // 尺寸
    if let Some(min) = query.min_size {
        if entry.size < min {
            return false;
        }
    }
    if let Some(max) = query.max_size {
        if entry.size > max {
            return false;
        }
    }

    // 修改时间
    if let Some(after) = query.modified_after {
        if entry.modified < after {
            return false;
        }
    }

    // 文件名关键词（多关键词 AND；空 text 不过滤）
    let text = query.text.trim();
    if !text.is_empty() {
        let name = entry.name.to_lowercase();
        for kw in text.split_whitespace() {
            if !name.contains(&kw.to_lowercase()) {
                return false;
            }
        }
    }

    true
}

/// 相关度打分，越大越靠前：
/// 完全相等 > 主干名相等 > 前缀 > 子串 > 仅条件命中
pub fn score(entry: &FileEntry, query: &SearchQuery) -> i32 {
    let text = query.text.trim();
    if text.is_empty() {
        return 0;
    }
    let name = entry.name.to_lowercase();
    let needle = text.to_lowercase();

    let stem_matches = Path::new(&name)
        .file_stem()
        .map(|s| s.to_string_lossy() == needle)
        .unwrap_or(false);

    if name == needle {
        1000
    } else if stem_matches {
        800
    } else if name.starts_with(&needle) {
        500
    } else if name.contains(&needle) {
        100
    } else {
        10
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, is_dir: bool, size: u64, ext: Option<&str>) -> FileEntry {
        FileEntry {
            id: 0,
            name: name.to_string(),
            path: format!("C:\\tmp\\{name}"),
            is_dir,
            size,
            modified: 0,
            ext: ext.map(|s| s.to_string()),
            hidden: false,
            system: false,
        }
    }

    #[test]
    fn parse_ext_multi() {
        let q = parse_query("ext:pdf,DOCX report");
        assert_eq!(q.ext, vec!["pdf", "docx"]);
        assert_eq!(q.text, "report");
        assert_eq!(q.limit, 500);
    }

    #[test]
    fn parse_ext_with_dot() {
        let q = parse_query("ext:.PNG");
        assert_eq!(q.ext, vec!["png"]);
    }

    #[test]
    fn parse_size_ops() {
        let q = parse_query("size:>100mb");
        assert_eq!(q.min_size, Some(100 * 1024 * 1024 + 1));

        let q = parse_query("size:<1gb");
        assert_eq!(q.max_size, Some(1024 * 1024 * 1024 - 1));

        let q = parse_query("size:100kb-10mb");
        assert_eq!(q.min_size, Some(100 * 1024));
        assert_eq!(q.max_size, Some(10 * 1024 * 1024));

        let q = parse_query("size:>=1mb");
        assert_eq!(q.min_size, Some(1024 * 1024));
    }

    #[test]
    fn parse_size_decimal() {
        assert_eq!(parse_size("1.5gb"), Some((1.5 * 1024.0 * 1024.0 * 1024.0) as u64));
        assert_eq!(parse_size("500b"), Some(500));
        assert_eq!(parse_size("nonsense"), None);
    }

    #[test]
    fn parse_modified_date() {
        let q = parse_query("modified:2024-01-01");
        assert!(q.modified_after.is_some());
    }

    #[test]
    fn parse_modified_relative() {
        let q = parse_query("modified:7d");
        assert!(q.modified_after.is_some());
        let q = parse_query("modified:today");
        assert!(q.modified_after.is_some());
    }

    #[test]
    fn parse_dir_file_flags() {
        assert_eq!(parse_query("dir:").is_dir, Some(true));
        assert_eq!(parse_query("file:").is_dir, Some(false));
    }

    #[test]
    fn matches_text_and_conditions() {
        let q = parse_query("ext:pdf report");
        assert!(matches(&entry("Report.pdf", false, 10, Some("pdf")), &q));
        assert!(!matches(&entry("Report.docx", false, 10, Some("docx")), &q));
        assert!(!matches(&entry("other.pdf", false, 10, Some("pdf")), &q));
    }

    #[test]
    fn matches_multi_keyword_and() {
        let q = parse_query("annual report");
        assert!(matches(&entry("Annual-Report.pdf", false, 1, Some("pdf")), &q));
        assert!(!matches(&entry("Annual.pdf", false, 1, Some("pdf")), &q));
    }

    #[test]
    fn matches_empty_text_not_filter_all() {
        let q = parse_query("ext:pdf");
        // 无 text 时不应过滤掉所有条目
        assert!(matches(&entry("anything.pdf", false, 1, Some("pdf")), &q));
    }

    #[test]
    fn matches_size_and_dir() {
        let q = parse_query("size:>1mb dir:");
        assert!(matches(&entry("folder", true, 0, None), &q) == false); // 目录 size=0，被 min 过滤
        let q2 = parse_query("dir:");
        assert!(matches(&entry("folder", true, 0, None), &q2));
        assert!(!matches(&entry("a.txt", false, 0, Some("txt")), &q2));
    }

    #[test]
    fn score_ordering() {
        let q = parse_query("report");
        let exact = score(&entry("report", false, 1, None), &q);
        let stem = score(&entry("report.pdf", false, 1, Some("pdf")), &q);
        let prefix = score(&entry("report-2024.pdf", false, 1, Some("pdf")), &q);
        let substr = score(&entry("my-report.pdf", false, 1, Some("pdf")), &q);
        assert!(exact > stem);
        assert!(stem > prefix);
        assert!(prefix > substr);
        assert!(substr > 0);
    }

    #[test]
    fn score_empty_text_zero() {
        let q = parse_query("ext:pdf");
        assert_eq!(score(&entry("a.pdf", false, 1, Some("pdf")), &q), 0);
    }
}

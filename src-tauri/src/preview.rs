//! 文件预览：按上限读取文本内容，并识别二进制 / 编码。
//!
//! 预览面板只负责「看一眼」，因此读取一律带大小上限：超出部分截断并返回
//! `truncated` 标记，避免超大文件把 IPC 与前端渲染拖死。二进制（图片 / PDF /
//! 可执行文件等）由前端直接走 asset 协议加载，这里只负责文本类内容。

use std::fs::File;
use std::io::{BufReader, Read};

use serde::Serialize;

/// 默认读取上限（字节）。1 MiB 足够展示几万行日志，也远小于会卡住渲染的量级。
pub const DEFAULT_MAX_BYTES: u64 = 1024 * 1024;
/// 前端可请求的上限；再大就按这个值截断，防止一次 IPC 传输几十兆文本。
const HARD_MAX_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextPreview {
    /// 解码后的文本内容；`binary` 为真时为空串。
    pub text: String,
    /// 文件比读取上限大，`text` 只是开头一部分。
    pub truncated: bool,
    /// 文件真实大小（字节）。
    pub size: u64,
    /// 内容看起来不是文本（含 NUL 字节或大量控制字符）。
    pub binary: bool,
    /// 采用的编码：`utf-8` / `utf-16le` / `utf-16be` / `gbk`。
    pub encoding: String,
}

/// 读取文本预览。
///
/// `max_bytes` 缺省为 [`DEFAULT_MAX_BYTES`]，会被夹在 4 KiB ~ 8 MiB 之间。
pub fn read_text_preview(path: &str, max_bytes: Option<u64>) -> Result<TextPreview, String> {
    let limit = max_bytes
        .unwrap_or(DEFAULT_MAX_BYTES)
        .clamp(4 * 1024, HARD_MAX_BYTES);

    // 先查元数据：Windows 上 `File::open` 对目录会直接报「拒绝访问」，先拦下来更易读。
    let meta = std::fs::metadata(path).map_err(|error| format!("无法读取文件信息：{error}"))?;
    if meta.is_dir() {
        return Err("这是一个文件夹".to_string());
    }
    let size = meta.len();

    let file = File::open(path).map_err(|error| format!("无法打开文件：{error}"))?;
    let mut buffer = Vec::with_capacity(limit.min(size) as usize + 4);
    BufReader::new(file)
        .take(limit)
        .read_to_end(&mut buffer)
        .map_err(|error| format!("读取文件失败：{error}"))?;
    let truncated = size > buffer.len() as u64;

    let (text, encoding) = decode(&buffer);
    let binary = looks_binary(&buffer, encoding);

    Ok(TextPreview {
        text: if binary { String::new() } else { text },
        truncated,
        size,
        binary,
        encoding: encoding.to_string(),
    })
}

/// 依据 BOM 与内容挑选编码。
fn decode(bytes: &[u8]) -> (String, &'static str) {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return (String::from_utf8_lossy(rest).into_owned(), "utf-8");
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return (decode_utf16(rest, true), "utf-16le");
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return (decode_utf16(rest, false), "utf-16be");
    }

    if let Ok(text) = std::str::from_utf8(bytes) {
        return (text.to_string(), "utf-8");
    }

    // 非法 UTF-8：中文 Windows 上多数是 GBK 文本，比较两种解码的替换字符数取更优者。
    let utf8_lossy = String::from_utf8_lossy(bytes);
    let (gbk_text, _, had_errors) = encoding_rs::GBK.decode(bytes);
    let utf8_bad = utf8_lossy.matches('\u{FFFD}').count();
    let gbk_bad = gbk_text.matches('\u{FFFD}').count();
    if !had_errors || gbk_bad < utf8_bad {
        (gbk_text.into_owned(), "gbk")
    } else {
        (utf8_lossy.into_owned(), "utf-8")
    }
}

/// UTF-16 解码（截断可能留下半个码元，用替换字符兜底）。
fn decode_utf16(bytes: &[u8], little_endian: bool) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| {
            if little_endian {
                u16::from_le_bytes([pair[0], pair[1]])
            } else {
                u16::from_be_bytes([pair[0], pair[1]])
            }
        })
        .collect();
    String::from_utf16_lossy(&units)
}

/// 二进制判定：含 NUL 字节，或可见控制字符占比过高。
fn looks_binary(bytes: &[u8], encoding: &str) -> bool {
    // UTF-16 文本天然带 NUL，不能按 NUL 判定。
    if encoding == "utf-8" && bytes.iter().take(8192).any(|byte| *byte == 0) {
        return true;
    }
    let sample = &bytes[..bytes.len().min(8192)];
    if sample.is_empty() {
        return false;
    }
    let control = sample
        .iter()
        .filter(|byte| {
            let b = **byte;
            b < 0x20 && b != b'\n' && b != b'\r' && b != b'\t' && b != 0x0C
        })
        .count();
    // GBK 双字节里第二字节可能落在 0x40-0x7E，不会触发控制字符统计。
    control * 100 / sample.len() > 10
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;

    fn temp_file(name: &str, bytes: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("stelix-preview-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("创建临时目录");
        let path = dir.join(name);
        let mut file = File::create(&path).expect("创建临时文件");
        file.write_all(bytes).expect("写入临时文件");
        path
    }

    #[test]
    fn reads_plain_utf8_text() {
        let path = temp_file("plain.txt", "第一行\nsecond line\n".as_bytes());
        let preview = read_text_preview(path.to_str().unwrap(), None).expect("读取预览");
        assert!(!preview.binary);
        assert!(!preview.truncated);
        assert_eq!(preview.encoding, "utf-8");
        assert!(preview.text.contains("第一行"));
    }

    #[test]
    fn truncates_large_text() {
        let content = "a".repeat(10_000);
        let path = temp_file("big.txt", content.as_bytes());
        let preview = read_text_preview(path.to_str().unwrap(), Some(4096)).expect("读取预览");
        assert!(preview.truncated);
        assert_eq!(preview.text.len(), 4096);
        assert_eq!(preview.size, 10_000);
    }

    #[test]
    fn detects_binary_content() {
        let path = temp_file("blob.bin", &[0x00, 0x01, 0x02, 0x00, 0x7F, 0x00]);
        let preview = read_text_preview(path.to_str().unwrap(), None).expect("读取预览");
        assert!(preview.binary);
        assert!(preview.text.is_empty());
    }

    #[test]
    fn decodes_utf8_bom() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice("带 BOM 的文本".as_bytes());
        let path = temp_file("bom.txt", &bytes);
        let preview = read_text_preview(path.to_str().unwrap(), None).expect("读取预览");
        assert_eq!(preview.encoding, "utf-8");
        assert_eq!(preview.text, "带 BOM 的文本");
    }

    #[test]
    fn decodes_utf16le() {
        let mut bytes = vec![0xFF, 0xFE];
        for unit in "宽字符".encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        let path = temp_file("utf16.txt", &bytes);
        let preview = read_text_preview(path.to_str().unwrap(), None).expect("读取预览");
        assert_eq!(preview.encoding, "utf-16le");
        assert_eq!(preview.text, "宽字符");
        assert!(!preview.binary);
    }

    #[test]
    fn decodes_gbk() {
        // 「中文」的 GBK 编码：D6 D0 CE C4
        let path = temp_file("gbk.txt", &[0xD6, 0xD0, 0xCE, 0xC4, 0x0A]);
        let preview = read_text_preview(path.to_str().unwrap(), None).expect("读取预览");
        assert_eq!(preview.encoding, "gbk");
        assert_eq!(preview.text.trim(), "中文");
    }

    #[test]
    fn rejects_directory() {
        let dir = std::env::temp_dir();
        let error = read_text_preview(dir.to_str().unwrap(), None).unwrap_err();
        assert!(error.contains("文件夹"));
    }
}

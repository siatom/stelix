//! 图标视图图片缩略图。
//!
//! 设计要点：
//! - 只处理本地常见位图格式（png / jpg / jpeg / webp / gif / bmp）。SVG、非图片
//!   文件、目录以及网络路径（UNC / 网络驱动器）一律返回 `None`，由前端继续显示
//!   系统类型图标。
//! - 先读文件头校验尺寸与像素总量，超过上限直接放弃：一张超大图不会因为解码占满
//!   内存，失败后前端仍能看到系统图标。
//! - 命中后按最长边缩放（`thumbnail` 快速采样）再编码为 PNG data URL；EXIF 方向
//!   会一并应用，手机照片不会横躺。
//! - 缓存 key 为「路径 + 文件大小 + 修改时间 + 目标边长」，文件改名或内容变化后
//!   自动失效；缓存同时限制条目数与总字节数，长时间浏览大量图片不会持续增长。

use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{BufReader, Cursor};
use std::path::Path;
use std::time::UNIX_EPOCH;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits};
use parking_lot::Mutex;

/// 允许生成缩略图的扩展名（与前端 `thumbnailCache.ts` 保持一致）。
const SUPPORTED_EXTS: &[&str] = &["png", "jpg", "jpeg", "webp", "gif", "bmp"];
/// 目标边长上下限：前端按 2x~3x 屏请求，夹紧后避免异常参数。
const MIN_EDGE: u32 = 32;
const MAX_EDGE: u32 = 512;
/// 源图尺寸上限：超过则不解码（宽/高与总像素任一超限都放弃）。
const MAX_SOURCE_EDGE: u32 = 12_000;
const MAX_SOURCE_PIXELS: u64 = 50_000_000;
/// 单张图解码可分配的字节上限。
const MAX_DECODE_BYTES: u64 = 256 * 1024 * 1024;
/// 缓存条目数与总字节上限。
const MAX_CACHE_ENTRIES: usize = 400;
const MAX_CACHE_BYTES: usize = 32 * 1024 * 1024;

/// 缩略图缓存（按路径 + 文件大小 + 修改时间 + 边长）。
#[derive(Default)]
pub struct ThumbnailCache {
    inner: Mutex<CacheInner>,
}

#[derive(Default)]
struct CacheInner {
    entries: HashMap<String, CacheEntry>,
    /// 插入顺序，用于淘汰最早的条目。
    order: VecDeque<String>,
    total_bytes: usize,
}

struct CacheEntry {
    url: String,
    bytes: usize,
}

impl ThumbnailCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// 取指定图片的缩略图（`data:image/png;base64,...`）；失败返回 `None`。
    pub fn get_or_load(&self, path: &str, edge: u32) -> Option<String> {
        let edge = edge.clamp(MIN_EDGE, MAX_EDGE);
        let meta = std::fs::metadata(path).ok()?;
        if !meta.is_file() || meta.len() == 0 {
            return None;
        }
        let modified = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|since| since.as_nanos())
            .unwrap_or(0);
        let key = cache_key(path, meta.len(), modified, edge);

        if let Some(hit) = self.get(&key) {
            return Some(hit);
        }
        if is_remote_path(path) {
            return None;
        }

        let url = render(path, edge)?;
        self.insert(key, url.clone());
        Some(url)
    }

    fn get(&self, key: &str) -> Option<String> {
        let mut inner = self.inner.lock();
        let url = inner.entries.get(key)?.url.clone();
        // 命中后移到队尾，保留最近使用过的条目。
        if let Some(index) = inner.order.iter().position(|item| item == key) {
            if let Some(item) = inner.order.remove(index) {
                inner.order.push_back(item);
            }
        }
        Some(url)
    }

    fn insert(&self, key: String, url: String) {
        let bytes = url.len();
        let mut inner = self.inner.lock();
        if let Some(old) = inner.entries.insert(key.clone(), CacheEntry { url, bytes }) {
            inner.total_bytes = inner.total_bytes.saturating_sub(old.bytes);
        }
        inner.order.push_back(key);
        inner.total_bytes += bytes;

        while inner.entries.len() > MAX_CACHE_ENTRIES || inner.total_bytes > MAX_CACHE_BYTES {
            let Some(oldest) = inner.order.pop_front() else {
                break;
            };
            if let Some(entry) = inner.entries.remove(&oldest) {
                inner.total_bytes = inner.total_bytes.saturating_sub(entry.bytes);
            }
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.inner.lock().entries.len()
    }
}

/// 缓存 key：文件名大小写不敏感，路径 + 大小 + 修改时间任一变化都会换 key。
fn cache_key(path: &str, len: u64, modified_nanos: u128, edge: u32) -> String {
    format!("{edge}\0{}\0{len}\0{modified_nanos}", path.to_lowercase())
}

/// 网络路径（SMB / WSL / 网络驱动器）默认不生成缩略图：
/// 远端读取的开销与延迟不可控，交给系统类型图标即可。
pub fn is_remote_path(path: &str) -> bool {
    // UNC：`\\server\share`，也覆盖 `\\wsl$` / `\\wsl.localhost`。
    if path.starts_with(r"\\") || path.starts_with("//") {
        return true;
    }

    #[cfg(windows)]
    {
        let bytes = path.as_bytes();
        if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
            const DRIVE_REMOTE: u32 = 4;
            let root = [bytes[0] as u16, b':' as u16, b'\\' as u16, 0];
            // SAFETY: 传入以 NUL 结尾的宽字符盘符根路径。
            if unsafe { windows_sys::Win32::Storage::FileSystem::GetDriveTypeW(root.as_ptr()) }
                == DRIVE_REMOTE
            {
                return true;
            }
        }
    }

    false
}

/// 小写扩展名；无扩展名返回空串。
fn extension_of(path: &str) -> String {
    Path::new(path)
        .extension()
        .map(|ext| ext.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

fn is_supported(path: &str) -> bool {
    SUPPORTED_EXTS.contains(&extension_of(path).as_str())
}

/// 解码 → 应用 EXIF 方向 → 缩放 → PNG data URL。
fn render(path: &str, edge: u32) -> Option<String> {
    if !is_supported(path) {
        return None;
    }

    let file = File::open(path).ok()?;
    let reader = ImageReader::new(BufReader::new(file))
        .with_guessed_format()
        .ok()?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SOURCE_EDGE);
    limits.max_image_height = Some(MAX_SOURCE_EDGE);
    limits.max_alloc = Some(MAX_DECODE_BYTES);

    let mut reader = reader;
    reader.limits(limits);
    let mut decoder = reader.into_decoder().ok()?;
    let (width, height) = decoder.dimensions();
    if width == 0 || height == 0 {
        return None;
    }
    // 解码器不一定都强制执行尺寸限制，这里再按像素总量兜底。
    if u64::from(width) * u64::from(height) > MAX_SOURCE_PIXELS {
        return None;
    }
    let orientation = decoder.orientation().ok()?;

    let mut image = DynamicImage::from_decoder(decoder).ok()?;
    image.apply_orientation(orientation);
    if image.width() == 0 || image.height() == 0 {
        return None;
    }

    let image = if image.width() > edge || image.height() > edge {
        image.thumbnail(edge, edge)
    } else {
        image
    };

    let mut out = Cursor::new(Vec::new());
    image.write_to(&mut out, ImageFormat::Png).ok()?;
    let encoded = BASE64.encode(out.into_inner());
    Some(format!("data:image/png;base64,{encoded}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("stelix-thumb-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("创建临时目录");
        dir
    }

    fn write_png(path: &Path, width: u32, height: u32) {
        let file = File::create(path).expect("创建 PNG");
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("写 PNG 头");
        let pixels = vec![0x80u8; (width * height * 4) as usize];
        writer.write_image_data(&pixels).expect("写 PNG 数据");
    }

    fn decode_data_url(url: &str) -> (u32, u32) {
        let encoded = url
            .strip_prefix("data:image/png;base64,")
            .expect("应是 PNG data URL");
        let bytes = BASE64.decode(encoded).expect("base64 应可解码");
        let mut reader = png::Decoder::new(Cursor::new(bytes))
            .read_info()
            .expect("PNG 应可读");
        let mut pixels = vec![0; reader.output_buffer_size()];
        let frame = reader.next_frame(&mut pixels).expect("PNG 应可解码");
        (frame.width, frame.height)
    }

    #[test]
    fn downscales_wide_image_keeping_aspect_ratio() {
        let dir = temp_dir("wide");
        let path = dir.join("wide.png");
        write_png(&path, 400, 100);

        let cache = ThumbnailCache::new();
        let url = cache
            .get_or_load(path.to_str().unwrap(), 64)
            .expect("应生成缩略图");
        assert_eq!(decode_data_url(&url), (64, 16));
    }

    #[test]
    fn does_not_upscale_small_image() {
        let dir = temp_dir("small");
        let path = dir.join("small.png");
        write_png(&path, 40, 20);

        let url = ThumbnailCache::new()
            .get_or_load(path.to_str().unwrap(), 128)
            .expect("应生成缩略图");
        assert_eq!(decode_data_url(&url), (40, 20));
    }

    #[test]
    fn caches_results_by_path_and_metadata() {
        let dir = temp_dir("cache");
        let path = dir.join("cached.png");
        write_png(&path, 60, 30);

        let cache = ThumbnailCache::new();
        let first = cache.get_or_load(path.to_str().unwrap(), 64).unwrap();
        let second = cache.get_or_load(path.to_str().unwrap(), 64).unwrap();
        assert_eq!(first, second);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn rejects_unsupported_and_missing_files() {
        let dir = temp_dir("reject");
        let text = dir.join("note.txt");
        let mut file = File::create(&text).unwrap();
        file.write_all(b"not an image").unwrap();

        let cache = ThumbnailCache::new();
        assert!(cache.get_or_load(text.to_str().unwrap(), 64).is_none());
        assert!(cache
            .get_or_load(dir.join("missing.png").to_str().unwrap(), 64)
            .is_none());
    }

    #[test]
    fn cache_key_changes_with_metadata() {
        let base = cache_key(r"C:\Pictures\a.png", 100, 1, 64);
        assert_eq!(base, cache_key(r"c:\pictures\A.PNG", 100, 1, 64));
        assert_ne!(base, cache_key(r"C:\Pictures\a.png", 101, 1, 64));
        assert_ne!(base, cache_key(r"C:\Pictures\a.png", 100, 2, 64));
        assert_ne!(base, cache_key(r"C:\Pictures\a.png", 100, 1, 128));
    }

    #[test]
    fn treats_unc_paths_as_remote() {
        assert!(is_remote_path(r"\\server\share\photo.png"));
        assert!(is_remote_path(r"\\wsl$\Ubuntu\home\me\photo.png"));
        assert!(is_remote_path(r"\\wsl.localhost\Ubuntu\photo.png"));
        assert!(!is_remote_path(r"C:\Pictures\photo.png"));
    }
}

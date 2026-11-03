//! Windows 系统图标：按「图标种类」提取 Shell 图标并缓存为 base64 PNG data URL。
//!
//! 设计要点（性能）：
//! - **不按文件缓存，按图标种类缓存**。绝大多数文件（.txt/.pdf/…）只按扩展名
//!   取系统关联图标，优先查询真实路径；不可用时按扩展名回退。
//! - 自带图标资源的文件（exe/lnk/ico/…）按每个文件缓存内嵌图标。
//! - 缓存 key 规则与前端 `src/components/icons/fileIconCache.ts` 保持一致：
//!   - 目录 → `dir`
//!   - 普通文件 → `e:<ext>`
//!   - 特殊文件 → `p:<小写路径>`
//!
//! 非 Windows 平台一律返回 `None`，前端回退到内置矢量图标。

use std::path::Path;

use dashmap::DashMap;

/// 自带图标资源、需要按「每个文件」单独提取的扩展名（与前端保持一致）。
const PER_FILE_EXTS: &[&str] = &[
    "exe", "lnk", "ico", "msi", "scr", "cpl", "url", "cur", "ani", "appx",
];

/// 图标缓存。缓存 `None` 表示提取失败，避免对同一 key 反复做慢查询。
#[derive(Default)]
pub struct IconCache {
    map: DashMap<String, Option<String>>,
}

impl IconCache {
    pub fn new() -> Self {
        Self {
            map: DashMap::new(),
        }
    }

    /// 取指定路径的图标（`data:image/png;base64,...`）；失败返回 `None`。
    pub fn get_or_load(&self, path: &str, is_dir: bool, use_path_icon: bool) -> Option<String> {
        let key = cache_key(path, is_dir, use_path_icon);
        if let Some(hit) = self.map.get(&key) {
            return hit.clone();
        }
        let value = extract(path, is_dir, use_path_icon);
        self.map.insert(key, value.clone());
        value
    }
}

/// 计算缓存 key（与前端 `fileIconCache.ts` 的 `iconKey` 同规则）。
/// 磁盘根目录单独缓存，避免与无扩展名文件共用图标。
pub fn cache_key(path: &str, is_dir: bool, use_path_icon: bool) -> String {
    if use_path_icon {
        return format!("system:{}", path.to_lowercase());
    }
    if !is_dir && is_drive_root(path) {
        return format!("drive:{}", path.to_lowercase());
    }
    if is_dir {
        return "dir".to_string();
    }
    let ext = extension_of(path);
    if PER_FILE_EXTS.contains(&ext.as_str()) {
        format!("p:{}", path.to_lowercase())
    } else {
        format!("e:{ext}")
    }
}

fn is_drive_root(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() == 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
}

/// 小写扩展名（不含点）；无扩展名返回空串。
///
/// 与 Windows Shell 的关联规则保持一致：以 `.` 开头的文件名（`.env`、`.gitignore` 等）
/// 同样取最后一个点之后的部分作为扩展名，因此 `.env` 与 `foo.env` 共用图标，
/// 而 `.env` / `.gitignore` 等不同 dotfile 不会再共用同一个缓存 key。
///
/// 注意：这里不能直接用 `Path::extension()`——它把 `.env` 视为「无扩展名」，
/// 会让所有 dotfile 落入同一个 `e:` 缓存，先加载的那个文件的图标会覆盖全部。
fn extension_of(path: &str) -> String {
    let name = Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    match name.rfind('.') {
        Some(index) if index + 1 < name.len() => name[index + 1..].to_lowercase(),
        _ => String::new(),
    }
}

// ---------------------------------------------------------------- 提取

fn extract(path: &str, is_dir: bool, use_path_icon: bool) -> Option<String> {
    #[cfg(windows)]
    {
        extract_windows(path, is_dir, use_path_icon)
    }
    #[cfg(not(windows))]
    {
        let _ = (path, is_dir, use_path_icon);
        None
    }
}

#[cfg(windows)]
fn extract_windows(path: &str, is_dir: bool, use_path_icon: bool) -> Option<String> {
    // Shell 首次初始化并发查询可能返回空图标；串行提取也避免争用系统图标缓存。
    static SHELL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _shell = SHELL_LOCK.lock().unwrap_or_else(|error| error.into_inner());

    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL,
    };
    use windows_sys::Win32::System::Com::{
        CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED,
    };
    use windows_sys::Win32::UI::Shell::{SHGFI_ICON, SHGFI_LARGEICON, SHGFI_USEFILEATTRIBUTES};
    use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyIcon, HICON};

    // 每次成功初始化（包括 S_FALSE）都必须配对释放。
    struct ComGuard(bool);
    impl Drop for ComGuard {
        fn drop(&mut self) {
            if self.0 {
                unsafe { CoUninitialize() };
            }
        }
    }
    let _com =
        ComGuard(unsafe { CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) >= 0 });

    let ext = extension_of(path);
    let base = SHGFI_ICON | SHGFI_LARGEICON;

    // 用假文件名按扩展名取「类型图标」，不读盘。
    let fake_name = if ext.is_empty() {
        "file".to_string()
    } else {
        format!("x.{ext}")
    };

    // Shell 命名空间对象（如 `::{645FF040-...}` 回收站）必须先把字符串解析成
    // PIDL，再以 `SHGFI_PIDL` 取图标；直接按普通路径查询只会得到通用文件图标。
    let hicon: Option<HICON> = if path.starts_with("::") {
        query_shell_icon(path, base)
    } else if is_dir {
        if use_path_icon {
            query_icon(path, FILE_ATTRIBUTE_DIRECTORY, base).or_else(|| {
                query_icon(
                    "folder",
                    FILE_ATTRIBUTE_DIRECTORY,
                    base | SHGFI_USEFILEATTRIBUTES,
                )
            })
        } else {
            query_icon(
                "folder",
                FILE_ATTRIBUTE_DIRECTORY,
                base | SHGFI_USEFILEATTRIBUTES,
            )
        }
    } else {
        // 每种扩展名只加载一次：真实路径可取得 Shell 扩展提供的关联图标。
        // 文件不存在（例如远程条目）时，仍可按扩展名取系统类型图标。
        query_icon(path, FILE_ATTRIBUTE_NORMAL, base).or_else(|| {
            query_icon(
                &fake_name,
                FILE_ATTRIBUTE_NORMAL,
                base | SHGFI_USEFILEATTRIBUTES,
            )
        })
    };

    let hicon = hicon?;
    let png = hicon_to_data_url(hicon);
    // SHGetFileInfoW 返回的 HICON 需要调用方释放。
    unsafe { DestroyIcon(hicon) };
    png
}

/// 调用 `SHGetFileInfoW` 取 HICON。
#[cfg(windows)]
fn query_icon(
    name: &str,
    attributes: u32,
    flags: u32,
) -> Option<windows_sys::Win32::UI::WindowsAndMessaging::HICON> {
    use windows_sys::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW};

    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let mut info: SHFILEINFOW = unsafe { std::mem::zeroed() };
    let ret = unsafe {
        SHGetFileInfoW(
            wide.as_ptr(),
            attributes,
            &mut info,
            std::mem::size_of::<SHFILEINFOW>() as u32,
            flags,
        )
    };
    if ret == 0 || info.hIcon.is_null() {
        None
    } else {
        Some(info.hIcon)
    }
}

/// 取 Shell 命名空间对象（`::{GUID}`）的图标：先解析为 PIDL，再用 `SHGFI_PIDL` 查询。
#[cfg(windows)]
fn query_shell_icon(
    name: &str,
    flags: u32,
) -> Option<windows_sys::Win32::UI::WindowsAndMessaging::HICON> {
    use std::ffi::c_void;
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{
        SHGetFileInfoW, SHParseDisplayName, SHFILEINFOW, SHGFI_PIDL,
    };

    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let mut pidl = std::ptr::null_mut();
    let hr = unsafe {
        SHParseDisplayName(
            wide.as_ptr(),
            std::ptr::null_mut(),
            &mut pidl,
            0,
            std::ptr::null_mut(),
        )
    };
    if hr < 0 || pidl.is_null() {
        return None;
    }

    let mut info: SHFILEINFOW = unsafe { std::mem::zeroed() };
    let ret = unsafe {
        SHGetFileInfoW(
            pidl as *const u16,
            0,
            &mut info,
            std::mem::size_of::<SHFILEINFOW>() as u32,
            flags | SHGFI_PIDL,
        )
    };
    // SHParseDisplayName 返回的 PIDL 由调用方用 CoTaskMemFree 释放。
    unsafe { CoTaskMemFree(pidl as *const c_void) };

    if ret == 0 || info.hIcon.is_null() {
        None
    } else {
        Some(info.hIcon)
    }
}

/// HICON → RGBA → PNG → base64 data URL。
#[cfg(windows)]
fn hicon_to_data_url(hicon: windows_sys::Win32::UI::WindowsAndMessaging::HICON) -> Option<String> {
    use windows_sys::Win32::Graphics::Gdi::{DeleteObject, HBITMAP, HGDIOBJ};
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetIconInfo, ICONINFO};

    let mut info: ICONINFO = unsafe { std::mem::zeroed() };
    if unsafe { GetIconInfo(hicon, &mut info) } == 0 {
        return None;
    }
    let color: HBITMAP = info.hbmColor;
    let mask: HBITMAP = info.hbmMask;

    let rgba = if color.is_null() {
        None
    } else {
        bitmap_to_rgba(color, mask)
    };

    // GetIconInfo 会新建两个位图，需自行释放。
    unsafe {
        if !color.is_null() {
            DeleteObject(color as HGDIOBJ);
        }
        if !mask.is_null() {
            DeleteObject(mask as HGDIOBJ);
        }
    }

    let (width, height, pixels) = rgba?;
    encode_png_data_url(width, height, &pixels)
}

/// 32bpp 颜色位图（+ 可选 1bpp 掩码）→ 直通 alpha 的 RGBA 像素。
#[cfg(windows)]
fn bitmap_to_rgba(
    color: windows_sys::Win32::Graphics::Gdi::HBITMAP,
    mask: windows_sys::Win32::Graphics::Gdi::HBITMAP,
) -> Option<(u32, u32, Vec<u8>)> {
    use windows_sys::Win32::Graphics::Gdi::{
        CreateCompatibleDC, DeleteDC, GetDC, GetDIBits, GetObjectW, ReleaseDC, BITMAP, BITMAPINFO,
        BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HGDIOBJ, RGBQUAD,
    };

    let mut bm: BITMAP = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        GetObjectW(
            color as HGDIOBJ,
            std::mem::size_of::<BITMAP>() as i32,
            &mut bm as *mut _ as *mut std::ffi::c_void,
        )
    };
    if ok == 0 || bm.bmWidth <= 0 || bm.bmHeight <= 0 {
        return None;
    }
    let width = bm.bmWidth as u32;
    let height = bm.bmHeight as u32;

    let screen_dc = unsafe { GetDC(std::ptr::null_mut()) };
    let mem_dc = unsafe { CreateCompatibleDC(screen_dc) };

    // ---- 颜色位图（32bpp，自顶向下） ----
    let mut bi: BITMAPINFO = unsafe { std::mem::zeroed() };
    bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bi.bmiHeader.biWidth = bm.bmWidth;
    bi.bmiHeader.biHeight = -bm.bmHeight;
    bi.bmiHeader.biPlanes = 1;
    bi.bmiHeader.biBitCount = 32;
    bi.bmiHeader.biCompression = BI_RGB;

    let mut color_buf = vec![0u8; (width * height * 4) as usize];
    // GetDIBits 要求位图未被选入 DC。
    let lines = unsafe {
        GetDIBits(
            mem_dc,
            color,
            0,
            height,
            color_buf.as_mut_ptr() as *mut std::ffi::c_void,
            &mut bi,
            DIB_RGB_COLORS,
        )
    };
    if lines == 0 {
        unsafe {
            DeleteDC(mem_dc);
            ReleaseDC(std::ptr::null_mut(), screen_dc);
        }
        return None;
    }

    // ---- 掩码位图（1bpp，自顶向下） ----
    let mask_stride = (width.div_ceil(32)) * 4;
    let mut mask_buf = vec![0u8; (mask_stride * height) as usize];
    let mut has_mask = false;
    if !mask.is_null() {
        let mut bm_mask: BITMAP = unsafe { std::mem::zeroed() };
        let ok_mask = unsafe {
            GetObjectW(
                mask as HGDIOBJ,
                std::mem::size_of::<BITMAP>() as i32,
                &mut bm_mask as *mut _ as *mut std::ffi::c_void,
            )
        };
        if ok_mask != 0 && bm_mask.bmHeight >= bm.bmHeight {
            // 1bpp 需要两个调色板项；BITMAPINFO 本身只预留了一个。
            #[repr(C)]
            struct MaskBitmapInfo {
                header: BITMAPINFOHEADER,
                colors: [RGBQUAD; 2],
            }
            let mut bi_mask: MaskBitmapInfo = unsafe { std::mem::zeroed() };
            bi_mask.header.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            bi_mask.header.biWidth = bm.bmWidth;
            bi_mask.header.biHeight = -bm.bmHeight;
            bi_mask.header.biPlanes = 1;
            bi_mask.header.biBitCount = 1;
            bi_mask.header.biCompression = BI_RGB;

            let lines_mask = unsafe {
                GetDIBits(
                    mem_dc,
                    mask,
                    0,
                    height,
                    mask_buf.as_mut_ptr() as *mut std::ffi::c_void,
                    &mut bi_mask as *mut MaskBitmapInfo as *mut BITMAPINFO,
                    DIB_RGB_COLORS,
                )
            };
            has_mask = lines_mask != 0;
        }
    }

    unsafe {
        DeleteDC(mem_dc);
        ReleaseDC(std::ptr::null_mut(), screen_dc);
    }

    // 32bpp 图标通常自带 alpha；若整张图 alpha 全为 0，则改用掩码推导。
    let has_alpha = color_buf.chunks_exact(4).any(|px| px[3] != 0);

    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let i = ((y * width + x) * 4) as usize;
            let b = color_buf[i];
            let g = color_buf[i + 1];
            let r = color_buf[i + 2];
            let a = if has_alpha {
                color_buf[i + 3]
            } else if has_mask {
                let byte = mask_buf[((y * mask_stride) + x / 8) as usize];
                if (byte >> (7 - (x % 8))) & 1 == 1 {
                    0
                } else {
                    255
                }
            } else {
                255
            };
            rgba.extend_from_slice(&[r, g, b, a]);
        }
    }

    Some((width, height, rgba))
}

/// RGBA 像素 → PNG → base64 data URL。
#[cfg(windows)]
fn encode_png_data_url(width: u32, height: u32, rgba: &[u8]) -> Option<String> {
    use base64::Engine as _;

    let mut out: Vec<u8> = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().ok()?;
        writer.write_image_data(rgba).ok()?;
    }
    let encoded = base64::engine::general_purpose::STANDARD.encode(&out);
    Some(format!("data:image/png;base64,{encoded}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_matches_expected_rules() {
        assert_eq!(cache_key(r"C:\Windows", true, false), "dir");
        assert_eq!(cache_key(r"C:\a\b.txt", false, false), "e:txt");
        assert_eq!(cache_key(r"C:\a\b.PDF", false, false), "e:pdf");
        assert_eq!(cache_key(r"C:\a\b", false, false), "e:");
        assert_eq!(
            cache_key(r"C:\Program Files\App.EXE", false, false),
            r"p:c:\program files\app.exe"
        );
        // dotfile 按 Windows Shell 规则去掉前导点取扩展名；
        // 不同 dotfile 必须分开缓存，否则会互相串用图标。
        assert_eq!(cache_key(r"C:\a\.env", false, false), "e:env");
        assert_eq!(cache_key(r"C:\a\.gitignore", false, false), "e:gitignore");
        assert_ne!(
            cache_key(r"C:\a\.env", false, false),
            cache_key(r"C:\a\.gitignore", false, false)
        );
        assert_eq!(cache_key(r"C:\a\README", false, false), "e:");
        assert_eq!(cache_key(r"C:\a\.env.local", false, false), "e:local");
    }

    #[cfg(windows)]
    #[test]
    fn extracts_folder_icon_as_png_data_url() {
        use base64::Engine as _;

        let url = extract(r"C:\Windows", true, false).expect("应能提取文件夹图标");
        let encoded = url
            .strip_prefix("data:image/png;base64,")
            .expect("应返回 base64 PNG data URL");
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .expect("base64 应可解码");
        assert_eq!(
            &bytes[..8],
            &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a],
            "应是合法 PNG 签名"
        );
    }

    #[cfg(windows)]
    #[test]
    fn extracts_recycle_bin_shell_icon() {
        use base64::Engine as _;
        // 回收站是 Shell 命名空间对象，用它的 GUID 路径取系统图标。
        let url = extract("::{645FF040-5081-101B-9F08-00AA002F954E}", false, true)
            .expect("应能提取回收站系统图标");
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(url.strip_prefix("data:image/png;base64,").unwrap())
            .unwrap();
        let mut reader = png::Decoder::new(std::io::Cursor::new(bytes))
            .read_info()
            .unwrap();
        let mut pixels = vec![0; reader.output_buffer_size()];
        let frame = reader.next_frame(&mut pixels).unwrap();
        let visible = pixels[..frame.buffer_size()]
            .chunks_exact(4)
            .filter(|p| p[3] > 0)
            .count();
        assert!(visible > 0, "回收站图标不应是全透明图");
    }

    #[cfg(windows)]
    #[test]
    fn extracts_document_icons_with_visible_pixels() {
        use base64::Engine as _;

        let cache = IconCache::new();
        for ext in ["docx", "xlsx", "pdf", "txt"] {
            let path = format!(r"C:\stelix-icon-test-missing.{ext}");
            let url = cache
                .get_or_load(&path, false, false)
                .unwrap_or_else(|| panic!("应能提取 {ext} 系统类型图标"));
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(url.strip_prefix("data:image/png;base64,").unwrap())
                .unwrap();
            let mut reader = png::Decoder::new(std::io::Cursor::new(bytes))
                .read_info()
                .unwrap();
            let mut pixels = vec![0; reader.output_buffer_size()];
            let frame = reader.next_frame(&mut pixels).unwrap();
            assert_eq!(frame.color_type, png::ColorType::Rgba);
            assert!(pixels[..frame.buffer_size()]
                .chunks_exact(4)
                .any(|p| p[3] > 0));
            assert_eq!(cache.get_or_load(&path, false, false), Some(url));
        }
    }
}

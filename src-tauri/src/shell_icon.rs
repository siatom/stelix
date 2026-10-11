//! Windows 系统图标：按「图标种类」提取 Shell 图标并缓存为 base64 PNG data URL。
//!
//! 设计要点（性能）：
//! - **不按文件缓存，按图标种类缓存**。绝大多数文件（.txt/.pdf/…）只按扩展名
//!   取「类型图标」，用 `SHGFI_USEFILEATTRIBUTES` 传入假文件名，**不读盘**。
//! - 只有自带图标资源的文件（exe/lnk/ico/…）才用真实路径读内嵌图标。
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
    pub fn get_or_load(&self, path: &str, is_dir: bool) -> Option<String> {
        let key = cache_key(path, is_dir);
        if let Some(hit) = self.map.get(&key) {
            return hit.clone();
        }
        let value = extract(path, is_dir);
        self.map.insert(key, value.clone());
        value
    }
}

/// 计算缓存 key（与前端 `fileIconCache.ts` 的 `iconKey` 同规则）。
pub fn cache_key(path: &str, is_dir: bool) -> String {
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

/// 小写扩展名（不含点）；无扩展名返回空串。
fn extension_of(path: &str) -> String {
    Path::new(path)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

// ---------------------------------------------------------------- 提取

fn extract(path: &str, is_dir: bool) -> Option<String> {
    #[cfg(windows)]
    {
        extract_windows(path, is_dir)
    }
    #[cfg(not(windows))]
    {
        let _ = (path, is_dir);
        None
    }
}

#[cfg(windows)]
fn extract_windows(path: &str, is_dir: bool) -> Option<String> {
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL,
    };
    use windows_sys::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
    use windows_sys::Win32::UI::Shell::{SHGFI_ICON, SHGFI_LARGEICON, SHGFI_USEFILEATTRIBUTES};
    use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyIcon, HICON};

    // Shell API 内部依赖 COM；当前线程若未初始化则补上，已初始化/换过 apartment 都忽略。
    unsafe {
        CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
    }

    let ext = extension_of(path);
    let per_file = !is_dir && PER_FILE_EXTS.contains(&ext.as_str());
    let base = SHGFI_ICON | SHGFI_LARGEICON;

    // 用假文件名按扩展名取「类型图标」，不读盘。
    let fake_name = if ext.is_empty() {
        "file".to_string()
    } else {
        format!("x.{ext}")
    };

    let hicon: Option<HICON> = if is_dir {
        query_icon(
            "folder",
            FILE_ATTRIBUTE_DIRECTORY,
            base | SHGFI_USEFILEATTRIBUTES,
        )
    } else if per_file {
        // 先用真实路径读内嵌图标；文件不存在/读取失败时退回类型图标。
        query_icon(path, FILE_ATTRIBUTE_NORMAL, base).or_else(|| {
            query_icon(
                &fake_name,
                FILE_ATTRIBUTE_NORMAL,
                base | SHGFI_USEFILEATTRIBUTES,
            )
        })
    } else {
        query_icon(
            &fake_name,
            FILE_ATTRIBUTE_NORMAL,
            base | SHGFI_USEFILEATTRIBUTES,
        )
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
        CreateCompatibleDC, DeleteDC, GetDC, GetDIBits, GetObjectW, ReleaseDC, SelectObject,
        BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HGDIOBJ,
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
    let old = unsafe { SelectObject(mem_dc, color as HGDIOBJ) };
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
    unsafe { SelectObject(mem_dc, old) };
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
            let mut bi_mask: BITMAPINFO = unsafe { std::mem::zeroed() };
            bi_mask.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            bi_mask.bmiHeader.biWidth = bm.bmWidth;
            bi_mask.bmiHeader.biHeight = -bm.bmHeight;
            bi_mask.bmiHeader.biPlanes = 1;
            bi_mask.bmiHeader.biBitCount = 1;
            bi_mask.bmiHeader.biCompression = BI_RGB;

            let old_mask = unsafe { SelectObject(mem_dc, mask as HGDIOBJ) };
            let lines_mask = unsafe {
                GetDIBits(
                    mem_dc,
                    mask,
                    0,
                    height,
                    mask_buf.as_mut_ptr() as *mut std::ffi::c_void,
                    &mut bi_mask,
                    DIB_RGB_COLORS,
                )
            };
            unsafe { SelectObject(mem_dc, old_mask) };
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
        assert_eq!(cache_key(r"C:\Windows", true), "dir");
        assert_eq!(cache_key(r"C:\a\b.txt", false), "e:txt");
        assert_eq!(cache_key(r"C:\a\b.PDF", false), "e:pdf");
        assert_eq!(cache_key(r"C:\a\b", false), "e:");
        assert_eq!(
            cache_key(r"C:\Program Files\App.EXE", false),
            r"p:c:\program files\app.exe"
        );
    }

    #[cfg(windows)]
    #[test]
    fn extracts_folder_icon_as_png_data_url() {
        use base64::Engine as _;

        let url = extract(r"C:\Windows", true).expect("应能提取文件夹图标");
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
}

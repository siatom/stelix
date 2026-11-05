//! Windows 文件剪贴板（CF_HDROP），用于与 Explorer 等程序互通。

#[cfg(windows)]
mod platform {
    use std::{mem::size_of, os::windows::ffi::OsStrExt, ptr};
    use windows_sys::Win32::{
        Foundation::{GlobalFree, HANDLE},
        System::{
            DataExchange::{
                CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable,
                OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
            },
            Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE},
            Ole::CF_HDROP,
        },
        UI::{Shell::DragQueryFileW, WindowsAndMessaging::GetForegroundWindow},
    };

    const DROPFILES_SIZE: usize = 20;
    const DROP_EFFECT_COPY: u32 = 1;
    const DROP_EFFECT_MOVE: u32 = 2;

    fn wide(value: &str) -> Vec<u16> {
        std::ffi::OsStr::new(value)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    fn preferred_effect_format() -> u32 {
        let name = wide("Preferred DropEffect");
        unsafe { RegisterClipboardFormatW(name.as_ptr()) }
    }

    pub fn set_files(paths: &[String], move_files: bool) -> Result<(), String> {
        if paths.is_empty() {
            return Err("剪贴板没有文件路径".into());
        }

        let mut list = Vec::<u16>::new();
        for path in paths {
            list.extend(std::ffi::OsStr::new(path).encode_wide());
            list.push(0);
        }
        list.push(0);

        let data_size = DROPFILES_SIZE + list.len() * size_of::<u16>();
        let drop_handle = unsafe { GlobalAlloc(GMEM_MOVEABLE, data_size) };
        if drop_handle.is_null() {
            return Err("无法分配文件剪贴板内存".into());
        }
        let drop_ptr = unsafe { GlobalLock(drop_handle) } as *mut u8;
        if drop_ptr.is_null() {
            unsafe { GlobalFree(drop_handle); }
            return Err("无法锁定文件剪贴板内存".into());
        }
        unsafe {
            ptr::write_bytes(drop_ptr, 0, DROPFILES_SIZE);
            ptr::write_unaligned(drop_ptr as *mut u32, DROPFILES_SIZE as u32);
            ptr::write_unaligned(drop_ptr.add(16) as *mut i32, 1); // fWide
            ptr::copy_nonoverlapping(
                list.as_ptr() as *const u8,
                drop_ptr.add(DROPFILES_SIZE),
                list.len() * 2,
            );
            GlobalUnlock(drop_handle);
        }

        let effect_format = preferred_effect_format();
        if effect_format == 0 {
            unsafe { GlobalFree(drop_handle); }
            return Err("无法注册文件剪贴板格式".into());
        }
        let effect_handle = unsafe { GlobalAlloc(GMEM_MOVEABLE, size_of::<u32>()) };
        if effect_handle.is_null() {
            unsafe { GlobalFree(drop_handle); }
            return Err("无法分配剪贴板操作标记".into());
        }
        let effect_ptr = unsafe { GlobalLock(effect_handle) } as *mut u32;
        if effect_ptr.is_null() {
            unsafe { GlobalFree(drop_handle); GlobalFree(effect_handle); }
            return Err("无法锁定剪贴板操作标记".into());
        }
        unsafe {
            *effect_ptr = if move_files {
                DROP_EFFECT_MOVE
            } else {
                DROP_EFFECT_COPY
            };
            GlobalUnlock(effect_handle);
        }

        let owner = unsafe { GetForegroundWindow() };
        if owner.is_null() || unsafe { OpenClipboard(owner) } == 0 {
            unsafe {
                GlobalFree(drop_handle);
                GlobalFree(effect_handle);
            }
            return Err("无法打开系统剪贴板".into());
        }
        let mut result = Ok(());
        unsafe {
            if EmptyClipboard() == 0
                || SetClipboardData(CF_HDROP as u32, drop_handle as HANDLE).is_null()
            {
                result = Err("无法写入文件剪贴板".into());
            } else {
                if SetClipboardData(effect_format, effect_handle as HANDLE).is_null() {
                    result = Err("无法写入剪切/复制标记".into());
                    GlobalFree(effect_handle);
                }
            }
            if result.is_err() {
                // SetClipboardData 成功时系统接管对应句柄；失败时仍由本进程释放。
                if GetClipboardData(CF_HDROP as u32) != drop_handle as HANDLE {
                    GlobalFree(drop_handle);
                }
            }
            CloseClipboard();
        }
        result
    }

    pub fn get_files() -> Result<Option<(Vec<String>, bool)>, String> {
        if unsafe { IsClipboardFormatAvailable(CF_HDROP as u32) } == 0 {
            return Ok(None);
        }
        if unsafe { OpenClipboard(ptr::null_mut()) } == 0 {
            return Err("无法读取系统剪贴板".into());
        }
        let result = unsafe {
            let handle = GetClipboardData(CF_HDROP as u32);
            if handle.is_null() {
                Err("无法读取文件剪贴板".into())
            } else {
                let count = DragQueryFileW(handle, u32::MAX, ptr::null_mut(), 0);
                let mut paths = Vec::with_capacity(count as usize);
                for index in 0..count {
                    let len = DragQueryFileW(handle, index, ptr::null_mut(), 0);
                    let mut buffer = vec![0u16; len as usize + 1];
                    let copied =
                        DragQueryFileW(handle, index, buffer.as_mut_ptr(), buffer.len() as u32);
                    buffer.truncate(copied as usize);
                    paths.push(String::from_utf16_lossy(&buffer));
                }
                let effect_format = preferred_effect_format();
                let is_move = if effect_format != 0 {
                    let effect = GetClipboardData(effect_format);
                    if effect.is_null() {
                        false
                    } else {
                        let value = GlobalLock(effect) as *const u32;
                        if value.is_null() {
                            false
                        } else {
                            let is_move = *value & DROP_EFFECT_MOVE != 0;
                            GlobalUnlock(effect);
                            is_move
                        }
                    }
                } else {
                    false
                };
                Ok(Some((paths, is_move)))
            }
        };
        unsafe {
            CloseClipboard();
        }
        result
    }
}

#[cfg(windows)]
pub use platform::{get_files, set_files};

/// macOS 文件剪贴板：通过 AppleScript 写入 / 读取 Finder 可识别的文件别名。
///
/// AppleScript 不区分「复制 / 剪切」标记，`move_files` 仅由应用内部剪切逻辑使用，
/// 跨应用粘贴总按复制处理。
#[cfg(target_os = "macos")]
mod apple {
    use std::process::Command;

    const SET_SCRIPT: &str = r#"
on run argv
  set fileList to {}
  repeat with p in argv
    set end of fileList to (POSIX file (contents of p))
  end repeat
  set the clipboard to fileList
end run
"#;

    const GET_SCRIPT: &str = r#"
on run
  set out to {}
  try
    set itemsList to the clipboard as list
  on error
    try
      set itemsList to {the clipboard as alias}
    on error
      return ""
    end try
  end try
  repeat with itemRef in itemsList
    try
      set end of out to POSIX path of (itemRef as alias)
    end try
  end repeat
  set AppleScript's text item delimiters to linefeed
  return out as text
end run
"#;

    fn run(script: &str, args: &[String]) -> Result<String, String> {
        let output = Command::new("osascript")
            .arg("-e")
            .arg(script)
            .args(args)
            .output()
            .map_err(|error| format!("无法启动 osascript：{error}"))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
            Err(if message.is_empty() {
                "AppleScript 执行失败".to_string()
            } else {
                message
            })
        }
    }

    pub fn set_files(paths: &[String], _move_files: bool) -> Result<(), String> {
        if paths.is_empty() {
            return Err("剪贴板没有文件路径".into());
        }
        run(SET_SCRIPT, paths).map(|_| ())
    }

    pub fn get_files() -> Result<Option<(Vec<String>, bool)>, String> {
        let text = run(GET_SCRIPT, &[])?;
        let paths: Vec<String> = text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect();
        Ok(if paths.is_empty() {
            None
        } else {
            Some((paths, false))
        })
    }
}

#[cfg(target_os = "macos")]
pub use apple::{get_files, set_files};

#[cfg(not(any(windows, target_os = "macos")))]
pub fn set_files(_: &[String], _: bool) -> Result<(), String> {
    Err("文件剪贴板目前仅支持 Windows / macOS".into())
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn get_files() -> Result<Option<(Vec<String>, bool)>, String> {
    Ok(None)
}

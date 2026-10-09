// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    #[cfg(windows)]
    if !acquire_single_instance() {
        return;
    }

    file_povit_lib::run()
}

#[cfg(windows)]
fn acquire_single_instance() -> bool {
    use windows_sys::Win32::{
        Foundation::{GetLastError, ERROR_ALREADY_EXISTS},
        System::Threading::CreateMutexW,
    };

    let build_kind = if cfg!(debug_assertions) {
        "dev"
    } else {
        "release"
    };
    let mutex_name: Vec<u16> = format!("Local\\com.filepovit.desktop.{build_kind}")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    // 保留句柄直至进程退出，让 Windows 自动释放互斥体。
    let mutex = unsafe { CreateMutexW(std::ptr::null(), 1, mutex_name.as_ptr()) };
    if mutex.is_null() {
        return false;
    }

    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe { windows_sys::Win32::Foundation::CloseHandle(mutex) };
        focus_existing_instance();
        return false;
    }

    // 原始 Win32 句柄不会随变量离开作用域自动关闭；进程退出时由系统释放。
    true
}

#[cfg(windows)]
fn focus_existing_instance() {
    use std::{thread, time::Duration};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        FindWindowW, SetForegroundWindow, ShowWindow, SW_RESTORE,
    };

    let title: Vec<u16> = "File Povit".encode_utf16().chain(std::iter::once(0)).collect();

    // 首实例可能刚拿到互斥体、窗口尚未创建，短暂等待它就绪。
    for _ in 0..30 {
        let window = unsafe { FindWindowW(std::ptr::null(), title.as_ptr()) };
        if !window.is_null() {
            unsafe {
                ShowWindow(window, SW_RESTORE);
                SetForegroundWindow(window);
            }
            return;
        }
        thread::sleep(Duration::from_millis(100));
    }
}

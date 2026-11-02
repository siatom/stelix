//! Windows volume arrival and removal notifications.

use std::sync::OnceLock;

use tauri::{AppHandle, Emitter};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    System::LibraryLoader::GetModuleHandleW,
    UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, RegisterClassW,
        RegisterDeviceNotificationW, TranslateMessage, DBT_DEVICEARRIVAL, DBT_DEVICEREMOVECOMPLETE,
        DBT_DEVTYP_VOLUME, DEVICE_NOTIFY_WINDOW_HANDLE, DEV_BROADCAST_HDR, DEV_BROADCAST_VOLUME,
        MSG, WM_DEVICECHANGE, WNDCLASSW, WS_POPUP,
    },
};

const EVENT_NAME: &str = "computer:drives-changed";
static APP: OnceLock<AppHandle> = OnceLock::new();

pub fn start(app: AppHandle) {
    let _ = APP.set(app);
    std::thread::Builder::new()
        .name("drive-change-monitor".into())
        .spawn(run)
        .expect("failed to start drive change monitor");
}

fn run() {
    let class_name: Vec<u16> = "StelixDriveChangeMonitor\0".encode_utf16().collect();
    let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
    let class = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(window_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: instance,
        hIcon: std::ptr::null_mut(),
        hCursor: std::ptr::null_mut(),
        hbrBackground: std::ptr::null_mut(),
        lpszMenuName: std::ptr::null(),
        lpszClassName: class_name.as_ptr(),
    };
    if unsafe { RegisterClassW(&class) } == 0 {
        return;
    }

    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            class_name.as_ptr(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        )
    };
    if hwnd.is_null() {
        return;
    }

    let filter = DEV_BROADCAST_VOLUME {
        dbcv_size: std::mem::size_of::<DEV_BROADCAST_VOLUME>() as u32,
        dbcv_devicetype: DBT_DEVTYP_VOLUME,
        dbcv_reserved: 0,
        dbcv_unitmask: 0,
        dbcv_flags: 0,
    };
    let notification = unsafe {
        RegisterDeviceNotificationW(
            hwnd,
            (&filter as *const DEV_BROADCAST_VOLUME).cast(),
            DEVICE_NOTIFY_WINDOW_HANDLE,
        )
    };
    if notification.is_null() {
        return;
    }

    let mut message: MSG = unsafe { std::mem::zeroed() };
    while unsafe { GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) } > 0 {
        unsafe {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_DEVICECHANGE
        && (wparam as u32 == DBT_DEVICEARRIVAL || wparam as u32 == DBT_DEVICEREMOVECOMPLETE)
        && lparam != 0
    {
        let header = &*(lparam as *const DEV_BROADCAST_HDR);
        if header.dbch_devicetype == DBT_DEVTYP_VOLUME {
            if let Some(app) = APP.get() {
                let _ = app.emit(EVENT_NAME, ());
            }
        }
    }
    DefWindowProcW(hwnd, message, wparam, lparam)
}

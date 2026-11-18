//! Windows 原生文件拖拽（自研实现，不依赖第三方拖拽插件）。
//!
//! 拖拽必须走 OLE `DoDragDrop`，外部应用（资源管理器 / 微信等）才能拿到真实
//! 文件路径。但 `DoDragDrop` 是阻塞式的：在主线程执行会让 Tauri 的事件循环
//! 停摆，拖拽期间前端收不到任何位置事件，也就无法实时高亮目标文件夹。
//!
//! 这里把拖拽放到独立的 STA 线程：Windows 继续接管鼠标，主线程保持空闲。
//!
//! 代价有两个，都在本模块里补掉：
//! 1. 后台线程拿不到真实的鼠标捕获，`QueryContinueDrag` 读到的按键状态会一直
//!    停在「左键按下」，`DoDragDrop` 永远不结束（松手也不落下）。→ 拖拽线程
//!    自带隐藏窗口接管鼠标捕获，并额外监听全局左键状态兜底。
//! 2. 后台线程发起的拖拽，WebView2 的 drop 目标并不总会收到 `Drop`，前端拿不到
//!    `tauri://drag-drop`。→ 拖拽过程中由本模块轮询光标位置，通过
//!    `stelix:drag-position` / `stelix:drag-release` / `stelix:drag-end` 三个事件
//!    把「悬停位置」和「是否落下」直接告诉前端，应用内移动/复制由前端据此完成。

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::Duration;

use tauri::{AppHandle, Emitter};
use windows::core::{implement, BOOL, HRESULT, PCWSTR};
use windows::Win32::Foundation::{
    DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, S_OK,
};
use windows::Win32::System::Com::{CoTaskMemFree, IDataObject};
use windows::Win32::System::Ole::{
    DoDragDrop, IDropSource, IDropSource_Impl, OleInitialize, DROPEFFECT, DROPEFFECT_COPY,
};
use windows::Win32::System::SystemServices::{MODIFIERKEYS_FLAGS, MK_LBUTTON};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{ILCreateFromPathW, SHCreateShellItemArrayFromIDLists, BHID_DataObject};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    mouse_event, GetAsyncKeyState, ReleaseCapture, SetActiveWindow, SetCapture, MOUSEEVENTF_MOVE,
    VK_LBUTTON,
};
use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GetCursorPos, PostThreadMessageW, RegisterClassW,
    SetForegroundWindow, WNDCLASSW, WM_LBUTTONUP, WS_POPUP,
};

/// 拖拽过程中光标所在位置（相对窗口客户区，物理像素）。
#[derive(Clone, serde::Serialize)]
struct DragPoint {
    x: i32,
    y: i32,
}

/// 拖拽结束（`dropped` = 正常落下，`cancelled` = 用户取消）。
#[derive(Clone, serde::Serialize)]
struct DragEnd {
    outcome: String,
}

#[derive(Clone, Copy)]
enum DragOutcome {
    Dropped,
    Cancelled,
}

/// 拖拽来源：Esc 取消，左键松开视为落下，其余继续拖拽。
#[implement(IDropSource)]
struct FileDropSource;

impl IDropSource_Impl for FileDropSource_Impl {
    fn QueryContinueDrag(&self, escape_pressed: BOOL, key_state: MODIFIERKEYS_FLAGS) -> HRESULT {
        if escape_pressed.as_bool() {
            DRAGDROP_S_CANCEL
        } else if (key_state & MK_LBUTTON) == MODIFIERKEYS_FLAGS(0) {
            DRAGDROP_S_DROP
        } else {
            S_OK
        }
    }

    fn GiveFeedback(&self, _effect: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

enum Request {
    Start {
        app: AppHandle,
        /// 主窗口句柄（裸值）：用来把屏幕坐标换算成客户区坐标。
        window: isize,
        paths: Vec<PathBuf>,
        reply: Sender<Result<DragOutcome, String>>,
    },
}

static DRAG_TX: OnceLock<Sender<Request>> = OnceLock::new();

/// 专用拖拽线程：首次调用时创建，之后复用。
///
/// `DoDragDrop` 要求调用线程是 STA 并自行泵消息；独立线程满足这一点，同时
/// 让主线程（Tauri / WebView2）保持空闲，拖拽期间的位置事件才能实时送到前端。
fn drag_sender() -> &'static Sender<Request> {
    DRAG_TX.get_or_init(|| {
        let (tx, rx) = channel::<Request>();
        thread::Builder::new()
            .name("stelix-file-drag".into())
            .spawn(move || {
                unsafe {
                    let _ = OleInitialize(None);
                }
                let hwnd = create_drag_window();
                while let Ok(Request::Start { app, window, paths, reply }) = rx.recv() {
                    let _ = reply.send(run_drag(hwnd, window as HWND, app, &paths));
                }
            })
            .expect("无法创建文件拖拽线程");
        tx
    })
}

unsafe extern "system" fn drag_window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    DefWindowProcW(hwnd, message, wparam, lparam)
}

/// 拖拽线程自己的隐藏窗口：`SetCapture` 只对调用线程拥有的窗口有效，有了它
/// 拖拽线程才能真的收到鼠标消息、让 `QueryContinueDrag` 看到左键松开。
fn create_drag_window() -> HWND {
    let class_name: Vec<u16> = "StelixFileDragSource\0".encode_utf16().collect();
    let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
    let class = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(drag_window_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: instance,
        hIcon: std::ptr::null_mut(),
        hCursor: std::ptr::null_mut(),
        hbrBackground: std::ptr::null_mut(),
        lpszMenuName: std::ptr::null(),
        lpszClassName: class_name.as_ptr(),
    };
    unsafe {
        RegisterClassW(&class);
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
    }
}

/// 拖拽跟踪器：轮询光标位置并广播给前端，同时兜底结束 OLE 拖拽循环。
///
/// 即使鼠标捕获没落到拖拽线程（比如被别的窗口抢走），松开时投递的
/// `WM_LBUTTONUP` 也能让 OLE 的拖拽循环重新判断按键状态并真正结束拖拽。
struct DragTracker {
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl DragTracker {
    fn start(app: AppHandle, window: isize) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let thread_id = unsafe { GetCurrentThreadId() };
        let handle = thread::spawn(move || {
            let window = window as HWND;
            let mut was_down = true;
            let mut last: Option<(i32, i32)> = None;
            while !flag.load(Ordering::SeqCst) {
                let mut point = POINT { x: 0, y: 0 };
                let has_point = unsafe { GetCursorPos(&mut point) } != 0;
                if has_point {
                    let mut local = POINT { x: point.x, y: point.y };
                    if unsafe { ScreenToClient(window, &mut local) } != 0 {
                        if last != Some((local.x, local.y)) {
                            last = Some((local.x, local.y));
                            let _ = app.emit(
                                "stelix:drag-position",
                                DragPoint { x: local.x, y: local.y },
                            );
                        }
                    }
                }
                let down = unsafe { GetAsyncKeyState(VK_LBUTTON as i32) } < 0;
                if was_down && !down {
                    unsafe {
                        mouse_event(MOUSEEVENTF_MOVE, 1, 0, 0, 0);
                        PostThreadMessageW(thread_id, WM_LBUTTONUP, 0, 0);
                    }
                    if let Some((x, y)) = last {
                        let _ = app.emit("stelix:drag-release", DragPoint { x, y });
                    }
                    break;
                } else if down {
                    was_down = true;
                }
                thread::sleep(Duration::from_millis(30));
            }
        });
        Self {
            stop,
            handle: Some(handle),
        }
    }

    fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// 用 Shell 的标准数据对象承载文件列表（等价于资源管理器拖拽的数据）。
fn file_data_object(paths: &[PathBuf]) -> windows::core::Result<IDataObject> {
    unsafe {
        let pidls: Vec<*const ITEMIDLIST> =
            paths.iter().map(|path| file_pidl(path) as *const ITEMIDLIST).collect();
        let result = SHCreateShellItemArrayFromIDLists(&pidls)
            .and_then(|array| array.BindToHandler(None, &BHID_DataObject));
        for pidl in pidls {
            CoTaskMemFree(Some(pidl as *const c_void));
        }
        result
    }
}

fn file_pidl(path: &Path) -> *mut ITEMIDLIST {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    unsafe { ILCreateFromPathW(PCWSTR::from_raw(wide.as_ptr())) }
}

fn run_drag(
    drag_hwnd: HWND,
    window: HWND,
    app: AppHandle,
    paths: &[PathBuf],
) -> Result<DragOutcome, String> {
    // 左键已经松开时才进来（比如用户只是点了一下），DoDragDrop 会一直等输入，
    // 直接按取消处理。
    let button = unsafe { GetAsyncKeyState(VK_LBUTTON as i32) };
    if button >= 0 {
        let _ = app.emit("stelix:drag-end", DragEnd { outcome: "cancelled".into() });
        return Ok(DragOutcome::Cancelled);
    }
    let data_object =
        file_data_object(paths).map_err(|error| format!("无法创建拖拽数据对象：{error}"))?;
    let drop_source: IDropSource = FileDropSource.into();
    let mut effect = DROPEFFECT::default();
    unsafe {
        SetActiveWindow(drag_hwnd);
        SetCapture(drag_hwnd);
    }
    let tracker = DragTracker::start(app.clone(), window as isize);
    // 只声明 COPY：拖到外部应用时不会因为对方的默认动作把源文件移走；
    // 应用内是移动还是复制由前端根据 Ctrl / Shift 自行决定。
    let result = unsafe { DoDragDrop(&data_object, &drop_source, DROPEFFECT_COPY, &mut effect) };
    tracker.stop();
    unsafe {
        ReleaseCapture();
        // `SetActiveWindow` 会把拖拽线程的隐藏窗口顶成前台窗口，拖完必须还给主窗口，
        // 否则应用会失去焦点。
        SetForegroundWindow(window);
    }
    let outcome = if result == DRAGDROP_S_DROP {
        DragOutcome::Dropped
    } else if result == DRAGDROP_S_CANCEL {
        DragOutcome::Cancelled
    } else {
        let _ = app.emit("stelix:drag-end", DragEnd { outcome: "cancelled".into() });
        return Err(format!("拖拽失败：{result:?}"));
    };
    let _ = app.emit(
        "stelix:drag-end",
        DragEnd {
            outcome: match outcome {
                DragOutcome::Dropped => "dropped".into(),
                DragOutcome::Cancelled => "cancelled".into(),
            },
        },
    );
    Ok(outcome)
}

/// 开始一次 Windows 原生文件拖拽，返回 `dropped` 或 `cancelled`。
#[tauri::command]
pub async fn start_file_drag(
    app: AppHandle,
    window: tauri::Window,
    paths: Vec<String>,
) -> Result<String, String> {
    if paths.is_empty() {
        return Err("没有可拖拽的文件".to_string());
    }
    let window_handle = window
        .hwnd()
        .map_err(|error| format!("无法获取窗口句柄：{error}"))?;
    let (reply, reply_rx) = channel();
    drag_sender()
        .send(Request::Start {
            app,
            window: window_handle.0 as isize,
            paths: paths.into_iter().map(PathBuf::from).collect(),
            reply,
        })
        .map_err(|_| "文件拖拽线程不可用".to_string())?;
    let received = tauri::async_runtime::spawn_blocking(move || reply_rx.recv())
        .await
        .map_err(|error| format!("文件拖拽任务失败：{error}"))?;
    let outcome = received.map_err(|_| "文件拖拽线程已退出".to_string())??;
    Ok(match outcome {
        DragOutcome::Dropped => "dropped".to_string(),
        DragOutcome::Cancelled => "cancelled".to_string(),
    })
}

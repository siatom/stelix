//! Windows Shell 原生右键菜单的筛选与弹出。
//!
//! “更多”菜单不自行维护应用列表：由 Shell 的 `IContextMenu` 构建菜单，过滤掉
//! 应用里已有的常用项后，用原生 `TrackPopupMenuEx` 弹出。子菜单、图标、第三方
//! 扩展项（7-Zip / Git / 云盘等）全部由系统渲染。
//!
//! ⚠ 必须在拥有窗口的 UI 线程调用。`show` 会阻塞到菜单关闭，这是原生菜单的
//! 正常行为；调用方（Tauri 同步 command）运行在主线程。

use std::cell::RefCell;
use std::mem::size_of;

use windows::core::{Interface, PCSTR, PCWSTR, PSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::Com::{
    CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    DefSubclassProc, IContextMenu, IContextMenu2, IContextMenu3, IShellFolder,
    RemoveWindowSubclass, SHBindToParent, SHParseDisplayName, SetWindowSubclass, CMF_EXPLORE,
    CMF_NORMAL, CMIC_MASK_PTINVOKE, CMINVOKECOMMANDINFO, CMINVOKECOMMANDINFOEX, GCS_VERBA,
    GCS_VERBW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreatePopupMenu, DeleteMenu, DestroyMenu, GetCursorPos, GetMenuItemCount, GetMenuItemID,
    GetMenuState, GetMenuStringW, PostMessageW, SetForegroundWindow, TrackPopupMenuEx, HMENU,
    MF_BYPOSITION, MF_SEPARATOR, SW_SHOWNORMAL, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_DRAWITEM,
    WM_INITMENUPOPUP, WM_MEASUREITEM, WM_MENUCHAR, WM_NULL,
};

/// Shell 为 `QueryContextMenu` 分配的命令 ID 范围。
const ID_CMD_FIRST: u32 = 1;
const ID_CMD_LAST: u32 = 0x7FFF;

/// 用于 `SetWindowSubclass` 的稳定子类 ID。
const SUBCLASS_ID: usize = 0x5354_4D31;

/// `CMIC_MASK_UNICODE` 在 windows crate 中未导出。
const CMIC_MASK_UNICODE: u32 = 0x0000_4000;

/// 应用菜单已经提供、无需在“更多”里重复的规范动词。
const HANDLED_VERBS: &[&str] = &[
    "open",
    "openas",
    "openwith",
    "opennew",
    "copy",
    "cut",
    "paste",
    "delete",
    "rename",
    "properties",
    "copyaspath",
    "runas",
    // 文件夹背景菜单里与应用工具条重复的项。
    "new",
    "view",
    "sort",
    "sortby",
    "refresh",
    "groupby",
    "arrange",
    "openinterminal",
    "openterminal",
];

/// 拿不到规范动词时，按本地化标题兜底去重（中英文常见值）。
const HANDLED_LABELS: &[&str] = &[
    "打开",
    "打开方式",
    "复制",
    "剪切",
    "粘贴",
    "删除",
    "重命名",
    "属性",
    "复制路径",
    "以管理员身份运行",
    "新建",
    "查看",
    "排序方式",
    "刷新",
    "在终端中打开",
    "在终端打开",
    "open",
    "open with",
    "copy",
    "cut",
    "paste",
    "delete",
    "rename",
    "properties",
    "copy as path",
    "run as administrator",
    "new",
    "view",
    "sort",
    "sort by",
    "refresh",
    "open in terminal",
    "open in windows terminal",
];

/// “更多”按钮在 WebView 客户端坐标系中的锚点（CSS 像素 + DPI 缩放）。
#[derive(Debug, Clone, Copy)]
pub struct Anchor {
    pub x: f64,
    pub y: f64,
    pub scale: f64,
}

/// 在 `hwnd` 的原生菜单中弹出经筛选的系统菜单。
///
/// `anchor` 存在时把菜单锚到该客户端坐标右侧（供“更多”做二级菜单）；
/// 缺失或坐标无效时回退到当前光标位置。
///
/// 返回 `false` 表示过滤后没有任何可显示项（不算错误）；用户点空白处取消
/// 也视为成功。真正的 Shell 调用错误以 `Err` 返回。
pub fn show(hwnd: HWND, path: &str, anchor: Option<Anchor>) -> Result<bool, String> {
    if hwnd.is_invalid() {
        return Err("窗口句柄无效".to_string());
    }

    let _com = ComInit::new()?;

    let menu = unsafe { CreatePopupMenu() }.map_err(|error| format!("创建菜单失败: {error}"))?;
    let menu = MenuGuard(menu);

    let context_menu = unsafe { create_context_menu(hwnd, path)? };
    let hr = unsafe {
        context_menu.QueryContextMenu(
            menu.0,
            0,
            ID_CMD_FIRST,
            ID_CMD_LAST,
            CMF_NORMAL | CMF_EXPLORE,
        )
    };
    if hr.is_err() {
        return Err(format!("构建系统菜单失败（HRESULT 0x{:08X}）", hr.0 as u32));
    }

    unsafe {
        filter_menu(&context_menu, menu.0);
        cleanup_separators(menu.0);
    }

    if unsafe { GetMenuItemCount(Some(menu.0)) } <= 0 {
        return Ok(false);
    }

    // 把动态子菜单 / owner-draw 消息转交给 Shell，否则“发送到”等子菜单可能为空。
    ACTIVE_MENU.with(|slot| {
        *slot.borrow_mut() = Some(ActiveMenu {
            cm2: context_menu.cast::<IContextMenu2>().ok(),
            cm3: context_menu.cast::<IContextMenu3>().ok(),
        });
    });
    let _subclass = SubclassGuard::install(hwnd);

    let _ = unsafe { SetForegroundWindow(hwnd) };
    let mut cursor = POINT::default();
    let _ = unsafe { GetCursorPos(&mut cursor) };
    let position = menu_position(hwnd, cursor, anchor);

    let command = unsafe {
        TrackPopupMenuEx(
            menu.0,
            TPM_RETURNCMD.0 | TPM_RIGHTBUTTON.0,
            position.x,
            position.y,
            hwnd,
            None,
        )
    };
    let _ = unsafe { PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0)) };

    drop(_subclass);
    ACTIVE_MENU.with(|slot| *slot.borrow_mut() = None);

    let command_id = command.0 as u32;
    if command_id == 0 {
        return Ok(true);
    }
    if command_id < ID_CMD_FIRST {
        return Err(format!("系统菜单返回了无效命令 ID: {command_id}"));
    }

    let offset = command_id - ID_CMD_FIRST;
    let mut info = CMINVOKECOMMANDINFOEX::default();
    info.cbSize = size_of::<CMINVOKECOMMANDINFOEX>() as u32;
    info.fMask = CMIC_MASK_UNICODE | CMIC_MASK_PTINVOKE;
    info.hwnd = hwnd;
    info.lpVerb = PCSTR(offset as usize as *const u8);
    info.lpVerbW = PCWSTR(offset as usize as *const u16);
    info.nShow = SW_SHOWNORMAL.0;
    info.ptInvoke = position;

    unsafe {
        if let Err(error) =
            context_menu.InvokeCommand(&info as *const _ as *const CMINVOKECOMMANDINFO)
        {
            // 用户取消（例如“发送到”子菜单里按 Esc）不是错误。
            const E_ABORT: i32 = 0x8000_4004u32 as i32;
            const CANCELLED: i32 = 0x8007_04C7u32 as i32;
            let code = error.code().0;
            if code == E_ABORT || code == CANCELLED {
                return Ok(true);
            }
            return Err(format!("执行系统菜单命令失败: {error}"));
        }
    }

    Ok(true)
}

/// 把 WebView 的 CSS 坐标换算成屏幕物理坐标；失败时回退到光标位置。
fn menu_position(hwnd: HWND, fallback: POINT, anchor: Option<Anchor>) -> POINT {
    let Some(anchor) = anchor else {
        return fallback;
    };
    if !anchor.x.is_finite()
        || !anchor.y.is_finite()
        || !anchor.scale.is_finite()
        || anchor.scale <= 0.0
    {
        return fallback;
    }

    let mut point = POINT {
        x: (anchor.x * anchor.scale).round() as i32,
        y: (anchor.y * anchor.scale).round() as i32,
    };
    if unsafe { ClientToScreen(hwnd, &mut point) }.as_bool() {
        point
    } else {
        fallback
    }
}

/// 通过 `SHParseDisplayName` + `IShellFolder::GetUIObjectOf` 取文件的 `IContextMenu`。
unsafe fn create_context_menu(hwnd: HWND, path: &str) -> Result<IContextMenu, String> {
    let path_wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
    SHParseDisplayName(PCWSTR(path_wide.as_ptr()), None, &mut pidl, 0, None)
        .map_err(|error| format!("无法解析文件路径: {error}"))?;
    if pidl.is_null() {
        return Err("无法解析文件路径：Shell 返回了空 PIDL".to_string());
    }

    let mut child: *mut ITEMIDLIST = std::ptr::null_mut();
    let folder = match SHBindToParent::<IShellFolder>(pidl, Some(&mut child)) {
        Ok(folder) => folder,
        Err(error) => {
            CoTaskMemFree(Some(pidl as *const _));
            return Err(format!("无法绑定文件所在文件夹: {error}"));
        }
    };

    let child: *const ITEMIDLIST = child;
    let context_menu = match folder.GetUIObjectOf::<IContextMenu>(hwnd, &[child], None) {
        Ok(menu) => menu,
        Err(error) => {
            CoTaskMemFree(Some(pidl as *const _));
            return Err(format!("无法读取系统右键菜单: {error}"));
        }
    };

    CoTaskMemFree(Some(pidl as *const _));
    Ok(context_menu)
}

/// 从后往前删除应用已提供或明确不需要的系统项。
unsafe fn filter_menu(context_menu: &IContextMenu, menu: HMENU) {
    let count = GetMenuItemCount(Some(menu));
    for index in (0..count).rev() {
        let index = index as u32;
        let state = GetMenuState(menu, index, MF_BYPOSITION);
        if state & MF_SEPARATOR.0 != 0 {
            continue;
        }

        let label = menu_label(menu, index);
        let id = GetMenuItemID(menu, index as i32);
        let verb = if (ID_CMD_FIRST..=ID_CMD_LAST).contains(&id) {
            command_verb(context_menu, id - ID_CMD_FIRST).unwrap_or_default()
        } else {
            String::new()
        };

        if !should_keep(&verb, &label) {
            let _ = DeleteMenu(menu, index, MF_BYPOSITION);
        }
    }
}

/// 读取菜单项标题（Shell 已完成本地化）。
unsafe fn menu_label(menu: HMENU, index: u32) -> String {
    let mut buffer = [0u16; 512];
    let length = GetMenuStringW(menu, index, Some(&mut buffer), MF_BYPOSITION);
    if length <= 0 {
        return String::new();
    }
    let length = (length as usize).min(buffer.len());
    let end = buffer[..length]
        .iter()
        .position(|&value| value == 0)
        .unwrap_or(length);
    String::from_utf16_lossy(&buffer[..end])
}

/// 读取菜单项的规范动词，优先 Unicode，失败后回退 ANSI。
unsafe fn command_verb(context_menu: &IContextMenu, offset: u32) -> Option<String> {
    if let Some(verb) = command_verb_w(context_menu, offset) {
        return Some(verb);
    }

    let mut buffer = [0u8; 256];
    context_menu
        .GetCommandString(
            offset as usize,
            GCS_VERBA,
            None,
            PSTR(buffer.as_mut_ptr()),
            buffer.len() as u32,
        )
        .ok()?;
    let end = buffer
        .iter()
        .position(|&value| value == 0)
        .unwrap_or(buffer.len());
    if end == 0 {
        None
    } else {
        Some(String::from_utf8_lossy(&buffer[..end]).into_owned())
    }
}

unsafe fn command_verb_w(context_menu: &IContextMenu, offset: u32) -> Option<String> {
    let mut buffer = [0u16; 256];
    context_menu
        .GetCommandString(
            offset as usize,
            GCS_VERBW,
            None,
            PSTR(buffer.as_mut_ptr() as *mut u8),
            buffer.len() as u32,
        )
        .ok()?;
    let end = buffer
        .iter()
        .position(|&value| value == 0)
        .unwrap_or(buffer.len());
    if end == 0 {
        None
    } else {
        Some(String::from_utf16_lossy(&buffer[..end]))
    }
}

fn should_keep(verb: &str, label: &str) -> bool {
    let label = normalize_label(label);
    if label.is_empty() {
        return false;
    }

    let verb = verb.trim().to_ascii_lowercase();
    if HANDLED_VERBS.contains(&verb.as_str()) {
        return false;
    }

    !HANDLED_LABELS.contains(&label.as_str())
}

/// 去掉 Windows 快捷键标记（`&`）和末尾的 `(H)` / `(&H)` 加速键说明，
/// 让中英文菜单项都能和去重表稳定匹配。
fn normalize_label(label: &str) -> String {
    let label = label.replace('&', "");
    let label = label.trim();

    if let Some(open) = label.rfind('(') {
        if label.ends_with(')') {
            let inner = &label[open + 1..label.len() - 1];
            if !inner.is_empty()
                && inner.chars().count() <= 2
                && inner.chars().all(|value| value.is_alphanumeric())
            {
                return label[..open].trim().to_lowercase();
            }
        }
    }

    label.to_lowercase()
}

/// 删除首尾以及连续的分隔线。
unsafe fn cleanup_separators(menu: HMENU) {
    loop {
        let count = GetMenuItemCount(Some(menu));
        if count <= 0 {
            break;
        }

        let mut removed = false;
        for index in 0..count {
            let state = GetMenuState(menu, index as u32, MF_BYPOSITION);
            if state & MF_SEPARATOR.0 == 0 {
                continue;
            }

            let is_first = index == 0;
            let is_last = index == count - 1;
            let previous_is_separator = index > 0
                && GetMenuState(menu, (index - 1) as u32, MF_BYPOSITION) & MF_SEPARATOR.0 != 0;
            if is_first || is_last || previous_is_separator {
                if DeleteMenu(menu, index as u32, MF_BYPOSITION).is_ok() {
                    removed = true;
                    break;
                }
            }
        }

        if !removed {
            break;
        }
    }
}

// ---------------------------------------------------------------- 消息转发

struct ActiveMenu {
    cm2: Option<IContextMenu2>,
    cm3: Option<IContextMenu3>,
}

thread_local! {
    static ACTIVE_MENU: RefCell<Option<ActiveMenu>> = const { RefCell::new(None) };
}

fn active_menu2() -> Option<IContextMenu2> {
    ACTIVE_MENU.with(|slot| slot.borrow().as_ref().and_then(|menu| menu.cm2.clone()))
}

fn active_menu3() -> Option<IContextMenu3> {
    ACTIVE_MENU.with(|slot| slot.borrow().as_ref().and_then(|menu| menu.cm3.clone()))
}

unsafe extern "system" fn shell_subclass_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    _data: usize,
) -> LRESULT {
    match message {
        WM_INITMENUPOPUP | WM_DRAWITEM | WM_MEASUREITEM => {
            if let Some(menu) = active_menu3() {
                let mut result = LRESULT(0);
                if menu
                    .HandleMenuMsg2(message, wparam, lparam, Some(&mut result))
                    .is_ok()
                {
                    return result;
                }
            }
            if let Some(menu) = active_menu2() {
                if menu.HandleMenuMsg(message, wparam, lparam).is_ok() {
                    return LRESULT(0);
                }
            }
        }
        WM_MENUCHAR => {
            if let Some(menu) = active_menu3() {
                let mut result = LRESULT(0);
                if menu
                    .HandleMenuMsg2(message, wparam, lparam, Some(&mut result))
                    .is_ok()
                {
                    return result;
                }
            }
        }
        _ => {}
    }

    DefSubclassProc(hwnd, message, wparam, lparam)
}

struct SubclassGuard {
    hwnd: HWND,
    installed: bool,
}

impl SubclassGuard {
    fn install(hwnd: HWND) -> Self {
        let installed =
            unsafe { SetWindowSubclass(hwnd, Some(shell_subclass_proc), SUBCLASS_ID, 0).as_bool() };
        Self { hwnd, installed }
    }
}

impl Drop for SubclassGuard {
    fn drop(&mut self) {
        if self.installed {
            unsafe {
                let _ = RemoveWindowSubclass(self.hwnd, Some(shell_subclass_proc), SUBCLASS_ID);
            }
        }
    }
}

// ---------------------------------------------------------------- 资源守卫

struct MenuGuard(HMENU);

impl Drop for MenuGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyMenu(self.0);
        }
    }
}

struct ComInit(bool);

impl ComInit {
    fn new() -> Result<Self, String> {
        const RPC_E_CHANGED_MODE: i32 = 0x8001_0106u32 as i32;

        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        if hr.is_err() && hr.0 != RPC_E_CHANGED_MODE {
            return Err(format!("初始化 COM 失败（HRESULT 0x{:08X}）", hr.0 as u32));
        }
        Ok(Self(hr.is_ok()))
    }
}

impl Drop for ComInit {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_duplicated_and_dangerous_verbs() {
        assert!(!should_keep("open", "打开"));
        assert!(!should_keep("openas", "打开方式"));
        assert!(!should_keep("", "打开方式(&H)"));
        assert!(!should_keep("copyaspath", "复制路径"));
        assert!(!should_keep("delete", "删除"));
        assert!(!should_keep("runas", "以管理员身份运行"));
    }

    #[test]
    fn keeps_useful_shell_items() {
        assert!(should_keep("sendto", "发送到"));
        assert!(should_keep("print", "打印"));
        assert!(should_keep("share", "共享"));
        assert!(should_keep("", "7-Zip"));
    }

    #[test]
    fn normalizes_accelerator_labels() {
        assert_eq!(normalize_label("发送到(&N)"), "发送到");
        assert_eq!(normalize_label("Open with (&H)"), "open with");
        assert_eq!(normalize_label("7-Zip (x64)"), "7-zip (x64)");
    }

    /// 依赖本机 Shell 注册表（第三方扩展不同），只用于手动排查：
    /// `cargo test --lib shell_menu::tests::dumps_filtered_menu -- --ignored --nocapture`
    #[test]
    #[ignore = "依赖本机 Shell 菜单注册，手动运行"]
    fn dumps_filtered_menu() {
        let path = std::env::temp_dir().join("stelix-shell-menu-test.txt");
        std::fs::write(&path, b"x").expect("write temp file");

        let items = debug_items(path.to_str().expect("temp path is utf-8")).expect("build menu");
        println!("{items:#?}");
        assert!(!items.is_empty());

        let _ = std::fs::remove_file(path);
    }

    fn debug_items(path: &str) -> Result<Vec<(String, String)>, String> {
        let _com = ComInit::new()?;
        let menu =
            unsafe { CreatePopupMenu() }.map_err(|error| format!("创建菜单失败: {error}"))?;
        let menu = MenuGuard(menu);
        let context_menu = unsafe { create_context_menu(HWND::default(), path)? };

        let hr = unsafe {
            context_menu.QueryContextMenu(
                menu.0,
                0,
                ID_CMD_FIRST,
                ID_CMD_LAST,
                CMF_NORMAL | CMF_EXPLORE,
            )
        };
        if hr.is_err() {
            return Err(format!("构建系统菜单失败（HRESULT 0x{:08X}）", hr.0 as u32));
        }

        unsafe {
            filter_menu(&context_menu, menu.0);
            cleanup_separators(menu.0);
        }

        let count = unsafe { GetMenuItemCount(Some(menu.0)) };
        let mut items = Vec::new();
        for index in 0..count {
            let label = unsafe { menu_label(menu.0, index as u32) };
            let id = unsafe { GetMenuItemID(menu.0, index as i32) };
            let verb = if (ID_CMD_FIRST..=ID_CMD_LAST).contains(&id) {
                unsafe { command_verb(&context_menu, id - ID_CMD_FIRST) }.unwrap_or_default()
            } else {
                String::new()
            };
            items.push((verb, label));
        }
        Ok(items)
    }
}

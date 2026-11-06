//! 通过 Windows Shell 的 `IContextMenu` 显示完整原生右键菜单。
//!
//! ⚠ 必须在拥有窗口的 UI 线程调用。`show_native` 会阻塞到菜单关闭，这是原生菜单的
//! 正常行为；调用方（Tauri 同步 command）运行在主线程。

use std::cell::RefCell;
use std::mem::size_of;

use windows::core::{Interface, PCSTR, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::Com::{
    CoInitializeEx, CoTaskMemFree, CoUninitialize, IBindCtx, COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    BHID_SFUIObject, DefSubclassProc, IContextMenu, IContextMenu2, IContextMenu3, IShellFolder,
    IShellItemArray, RemoveWindowSubclass, SHBindToParent, SHCreateShellItemArrayFromIDLists,
    SHParseDisplayName, SetWindowSubclass, CMF_EXPLORE, CMF_NORMAL, CMIC_MASK_PTINVOKE,
    CMINVOKECOMMANDINFO, CMINVOKECOMMANDINFOEX,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, GetMenuItemCount, PostMessageW,
    SetForegroundWindow, TrackPopupMenuEx, HMENU, MF_SEPARATOR, MF_STRING, SW_SHOWNORMAL,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_DRAWITEM, WM_INITMENUPOPUP, WM_MEASUREITEM, WM_MENUCHAR,
    WM_NULL,
};

/// Shell 为 `QueryContextMenu` 分配的命令 ID 范围。
const ID_CMD_FIRST: u32 = 1;
const ID_CMD_LAST: u32 = 0x7FFF;

/// 用于 `SetWindowSubclass` 的稳定子类 ID。
const SUBCLASS_ID: usize = 0x5354_4D31;

/// `CMIC_MASK_UNICODE` 在 windows crate 中未导出。
const CMIC_MASK_UNICODE: u32 = 0x0000_4000;

/// “更多”按钮在 WebView 客户端坐标系中的锚点（CSS 像素 + DPI 缩放）。
#[derive(Debug, Clone, Copy)]
pub struct Anchor {
    pub x: f64,
    pub y: f64,
    pub scale: f64,
}

/// 应用自定义菜单项（追加在系统菜单末尾的分隔线之后）。
#[derive(Debug, Clone)]
pub struct MenuItem {
    /// 返回给前端的标识（如 `duplicate` / `copy-to`）。
    pub id: String,
    /// 菜单显示文字。
    pub label: String,
}

/// 应用自定义命令 ID 的起始值：与 Shell 的 [ID_CMD_FIRST, ID_CMD_LAST] 不重叠。
const APP_CMD_FIRST: u32 = 0x8000;

/// 在 `hwnd` 中显示 Windows Shell 为**整个选中集**提供的完整原生菜单。
///
/// `paths` 是选中集（至少一项，被右键项排在最前）：Shell 扩展和系统项看到的是整份选中集，
/// 所以 7-Zip 的「添加到压缩包」会把所有条目打进同一个包。
///
/// `items` 会作为「应用扩展段」追加在系统菜单末尾；用户选中其中一项时返回它的 id，
/// 选中系统菜单项或取消时返回 `None`（系统项由 Shell 自己执行）。
pub fn show_native(
    hwnd: HWND,
    paths: &[String],
    anchor: Option<Anchor>,
    items: &[MenuItem],
) -> Result<Option<String>, String> {
    if hwnd.is_invalid() {
        return Err("窗口句柄无效".to_string());
    }
    if paths.is_empty() {
        return Err("没有可操作的路径".to_string());
    }
    let _com = ComInit::new()?;
    let menu = unsafe { CreatePopupMenu() }.map_err(|error| format!("创建菜单失败: {error}"))?;
    let menu = MenuGuard(menu);
    let context_menu = unsafe { create_context_menu(hwnd, paths)? };
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
    if unsafe { GetMenuItemCount(Some(menu.0)) } <= 0 && items.is_empty() {
        return Ok(None);
    }
    // 追加应用扩展段：系统菜单 + 一条分隔线 + 应用自己的操作。
    if !items.is_empty() {
        unsafe {
            let _ = AppendMenuW(menu.0, MF_SEPARATOR, 0, PCWSTR::null());
        }
        for (index, item) in items.iter().enumerate() {
            let label: Vec<u16> = item
                .label
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            unsafe {
                AppendMenuW(
                    menu.0,
                    MF_STRING,
                    (APP_CMD_FIRST + index as u32) as usize,
                    PCWSTR(label.as_ptr()),
                )
                .map_err(|error| format!("追加应用菜单项失败: {error}"))?;
            }
        }
    }
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
        return Ok(None);
    }
    // 应用扩展段：不交给 Shell，直接把 id 返回前端执行。
    if command_id >= APP_CMD_FIRST {
        let index = (command_id - APP_CMD_FIRST) as usize;
        return Ok(items.get(index).map(|item| item.id.clone()));
    }
    if !(ID_CMD_FIRST..=ID_CMD_LAST).contains(&command_id) {
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
            const E_ABORT: i32 = 0x8000_4004u32 as i32;
            const CANCELLED: i32 = 0x8007_04C7u32 as i32;
            let code = error.code().0;
            if code == E_ABORT || code == CANCELLED {
                return Ok(None);
            }
            return Err(format!("执行系统菜单命令失败: {error}"));
        }
    }
    Ok(None)
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

/// SHParseDisplayName 只接受反斜杠分隔符：把应用内部的正斜杠路径统一成原生形式。
fn to_native_path(path: &str) -> String {
    path.replace('/', "\\")
}

/// 为整个选中集取 `IContextMenu`。
///
/// 多选走 `IShellItemArray`：选中项可以散落在不同目录（搜索标签页），而单选实现里
/// 「父 `IShellFolder` + 子 PIDL」的表达方式只能覆盖同一目录下的兄弟项。
unsafe fn create_context_menu(hwnd: HWND, paths: &[String]) -> Result<IContextMenu, String> {
    if paths.len() > 1 {
        // 非常规选中项（回收站条目、已消失的路径）建不出多项菜单时，退回只处理
        // 被右键的那一项：行为不劣于修复前，也不会让整个菜单打不开。
        if let Ok(menu) = create_multi_context_menu(paths) {
            return Ok(menu);
        }
    }
    create_single_context_menu(hwnd, &paths[0])
}

/// 多选：`SHParseDisplayName` → `IShellItemArray` → `BindToHandler(BHID_SFUIObject)`。
unsafe fn create_multi_context_menu(paths: &[String]) -> Result<IContextMenu, String> {
    let pidls = parse_pidls(paths)?;
    let array = shell_item_array(&pidls)?;
    array
        .BindToHandler(None::<&IBindCtx>, &BHID_SFUIObject)
        .map_err(|error| format!("无法读取系统右键菜单: {error}"))
}

/// 单选：`SHParseDisplayName` + `IShellFolder::GetUIObjectOf`。
unsafe fn create_single_context_menu(hwnd: HWND, path: &str) -> Result<IContextMenu, String> {
    let pidl = OwnedPidl::parse(path)?;
    let mut child: *mut ITEMIDLIST = std::ptr::null_mut();
    let folder = SHBindToParent::<IShellFolder>(pidl.as_ptr(), Some(&mut child))
        .map_err(|error| format!("无法绑定文件所在文件夹: {error}"))?;

    let child: *const ITEMIDLIST = child;
    folder
        .GetUIObjectOf::<IContextMenu>(hwnd, &[child], None)
        .map_err(|error| format!("无法读取系统右键菜单: {error}"))
}

/// 多个路径 → 绝对 PIDL 列表，保持传入顺序。
unsafe fn parse_pidls(paths: &[String]) -> Result<Vec<OwnedPidl>, String> {
    paths.iter().map(|path| OwnedPidl::parse(path)).collect()
}

/// 选中集的 `IShellItemArray`（`SHCreateShellItemArrayFromIDLists` 的封装）。
unsafe fn shell_item_array(pidls: &[OwnedPidl]) -> Result<IShellItemArray, String> {
    let absolute: Vec<*const ITEMIDLIST> = pidls.iter().map(OwnedPidl::as_ptr).collect();
    SHCreateShellItemArrayFromIDLists(&absolute).map_err(|error| format!("无法构造选中集: {error}"))
}

/// `SHParseDisplayName` 分配的绝对 PIDL，离开作用域时用 `CoTaskMemFree` 释放。
struct OwnedPidl(*const ITEMIDLIST);

impl OwnedPidl {
    unsafe fn parse(path: &str) -> Result<Self, String> {
        let native = to_native_path(path);
        let path_wide: Vec<u16> = native.encode_utf16().chain(std::iter::once(0)).collect();
        let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
        SHParseDisplayName(PCWSTR(path_wide.as_ptr()), None, &mut pidl, 0, None)
            .map_err(|error| format!("无法解析文件路径: {error}"))?;
        if pidl.is_null() {
            return Err("无法解析文件路径：Shell 返回了空 PIDL".to_string());
        }
        Ok(Self(pidl))
    }

    fn as_ptr(&self) -> *const ITEMIDLIST {
        self.0
    }
}

impl Drop for OwnedPidl {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CoTaskMemFree(Some(self.0 as *const _)) };
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

    /// 临时目录：`Drop` 时递归删除。
    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("stelix-shell-menu-{tag}-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&path).expect("创建临时目录");
            Self(path)
        }

        /// 写入一个文件（相对路径可带子目录），返回它的完整路径。
        fn write(&self, relative: &str) -> String {
            let path = self.0.join(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("创建子目录");
            }
            std::fs::write(&path, b"x").expect("写入临时文件");
            path.to_string_lossy().into_owned()
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn normalizes_forward_slashes_for_shell_parsing() {
        assert_eq!(to_native_path("D:/a/b.txt"), "D:\\a\\b.txt");
        assert_eq!(to_native_path("D:\\a\\b.txt"), "D:\\a\\b.txt");
    }

    /// 多选必须把**全部**选中项放进 `IShellItemArray`：7-Zip 这类 Shell 扩展正是从这个
    /// 选中集里取文件列表，少一项就会少打包一个文件（本缺陷的回归测试）。
    #[test]
    fn multi_selection_builds_item_array_for_all_paths() {
        let dir = TempDir::new("array");
        let sibling = dir.write("sibling.txt");
        let nested = dir.write("nested/deep.txt");
        let _com = ComInit::new().expect("初始化 COM");

        // 同目录兄弟项与跨目录选中项走同一条路径，都必须得到完整的 2 项。
        let selections = [
            vec![sibling.clone(), dir.write("second.txt")],
            vec![sibling, nested],
        ];
        for paths in selections {
            let pidls = unsafe { parse_pidls(&paths) }.expect("解析 PIDL");
            let array = unsafe { shell_item_array(&pidls) }.expect("构造选中集");
            let count = unsafe { array.GetCount() }.expect("读取选中集条目数");
            assert_eq!(count, paths.len() as u32);
        }
    }

    #[test]
    fn missing_path_is_rejected() {
        let dir = TempDir::new("missing");
        let absent = dir.0.join("absent.txt").to_string_lossy().into_owned();
        let _com = ComInit::new().expect("初始化 COM");
        assert!(unsafe { parse_pidls(&[absent]) }.is_err());
    }

    /// 真实创建多项 `IContextMenu` 并 `QueryContextMenu`。需要带桌面的 Windows 会话，
    /// 不进 CI（无桌面 runner 上会 flaky）：
    /// `cargo test --manifest-path src-tauri/Cargo.toml -- --ignored`。
    #[test]
    #[ignore = "需要带桌面的 Windows 会话"]
    fn multi_selection_context_menu_has_commands() {
        let dir = TempDir::new("menu");
        let paths = vec![dir.write("a.txt"), dir.write("b.txt")];
        let _com = ComInit::new().expect("初始化 COM");
        let hwnd = HWND(std::ptr::null_mut());
        let context_menu = unsafe { create_context_menu(hwnd, &paths) }.expect("创建系统菜单");

        let popup = MenuGuard(unsafe { CreatePopupMenu() }.expect("创建弹出菜单"));
        let hr = unsafe {
            context_menu.QueryContextMenu(
                popup.0,
                0,
                ID_CMD_FIRST,
                ID_CMD_LAST,
                CMF_NORMAL | CMF_EXPLORE,
            )
        };
        assert!(hr.is_ok(), "QueryContextMenu 失败：{hr:?}");
        assert!(
            unsafe { GetMenuItemCount(Some(popup.0)) } > 0,
            "多项菜单没有任何条目"
        );
    }
}

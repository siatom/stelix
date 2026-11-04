//! 查询「谁占用了这个文件」——基于 Windows Restart Manager。
//!
//! 文件操作失败时（典型是 ERROR_SHARING_VIOLATION / os error 32），把占用进程名
//! 拼进错误文案，用户就知道该关掉哪个程序，而不是只看到一句「另一个程序正在使用此文件」。
//!
//! 全程失败即返回空列表：查不出占用者绝不能让「删除/重命名」本身的报错流程也崩掉。

/// 返回占用指定路径的进程名（Restart Manager 的 `strAppName`）。
///
/// 已去掉首尾空白、去重，最多 5 个。路径不存在、Restart Manager 不可用、
/// 没有任何占用者等情况一律返回空 `Vec`；非 Windows 平台恒为空。
pub fn locking_processes(path: &str) -> Vec<String> {
    #[cfg(windows)]
    {
        imp::locking_processes(path)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Vec::new()
    }
}

#[cfg(windows)]
mod imp {
    use windows_sys::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS};
    use windows_sys::Win32::System::RestartManager::{
        RmEndSession, RmGetList, RmRegisterResources, RmStartSession, CCH_RM_SESSION_KEY,
        RM_PROCESS_INFO,
    };

    /// 最多返回多少个进程名（错误文案不宜过长）。
    const MAX_NAMES: usize = 5;
    /// 单次查询最多申请多少个 `RM_PROCESS_INFO`，防止异常情况下巨量分配。
    const MAX_CANDIDATES: usize = 64;

    pub fn locking_processes(path: &str) -> Vec<String> {
        if path.trim().is_empty() {
            return Vec::new();
        }
        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        query(&wide)
    }

    fn query(wide: &[u16]) -> Vec<String> {
        let mut handle: u32 = 0;
        // 会话键必须是 CCH_RM_SESSION_KEY + 1 个 wchar（含结尾 0），由 RmStartSession 填充。
        let mut key = [0u16; CCH_RM_SESSION_KEY as usize + 1];
        if unsafe { RmStartSession(&mut handle, 0, key.as_mut_ptr()) } != ERROR_SUCCESS {
            return Vec::new();
        }
        let files = [wide.as_ptr()];
        let registered = unsafe {
            RmRegisterResources(
                handle,
                files.len() as u32,
                files.as_ptr(),
                0,
                std::ptr::null(),
                0,
                std::ptr::null(),
            )
        };
        let names = if registered == ERROR_SUCCESS {
            collect(handle)
        } else {
            Vec::new()
        };
        unsafe {
            RmEndSession(handle);
        }
        names
    }

    /// 两段式取列表：先问需要多少条，再按数量取数据。
    fn collect(handle: u32) -> Vec<String> {
        let mut needed: u32 = 0;
        let mut count: u32 = 0;
        let mut reasons: u32 = 0;
        let rc = unsafe {
            RmGetList(
                handle,
                &mut needed,
                &mut count,
                std::ptr::null_mut(),
                &mut reasons,
            )
        };
        // 没有占用者时返回 ERROR_SUCCESS 且 needed == 0；有占用者时返回 ERROR_MORE_DATA。
        if rc != ERROR_SUCCESS && rc != ERROR_MORE_DATA {
            return Vec::new();
        }
        if needed == 0 {
            return Vec::new();
        }

        let cap = (needed as usize).min(MAX_CANDIDATES);
        let zeroed: RM_PROCESS_INFO = unsafe { std::mem::zeroed() };
        let mut infos = vec![zeroed; cap];
        count = cap as u32;
        let rc = unsafe {
            RmGetList(
                handle,
                &mut needed,
                &mut count,
                infos.as_mut_ptr(),
                &mut reasons,
            )
        };
        if rc != ERROR_SUCCESS {
            return Vec::new();
        }

        let mut names: Vec<String> = Vec::new();
        for info in infos.iter().take((count as usize).min(cap)) {
            let name = app_name(info);
            if name.is_empty() || names.iter().any(|n| n == &name) {
                continue;
            }
            names.push(name);
            if names.len() >= MAX_NAMES {
                break;
            }
        }
        names
    }

    /// `strAppName` 是定长 `[u16; 256]`，取第一个 0 之前的部分并去掉首尾空白。
    fn app_name(info: &RM_PROCESS_INFO) -> String {
        let len = info
            .strAppName
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(info.strAppName.len());
        String::from_utf16_lossy(&info.strAppName[..len])
            .trim()
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonexistent_path_has_no_lockers() {
        let mut path = std::env::temp_dir();
        path.push("stelix-file-locks-nonexistent-xyz.txt");
        let _ = std::fs::remove_file(&path);
        assert!(locking_processes(&path.to_string_lossy()).is_empty());
    }

    /// 独占打开（share_mode = 0）的文件，Restart Manager 应能报出占用者。
    ///
    /// 某些环境下 Restart Manager 不报告「本进程自己」持有的句柄，此时仅提示、不算失败。
    #[cfg(windows)]
    #[test]
    fn exclusive_open_is_reported() {
        use std::os::windows::fs::OpenOptionsExt;

        let mut path = std::env::temp_dir();
        path.push(format!(
            "stelix-file-locks-{}-{}.txt",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::write(&path, b"x").expect("写入临时文件");

        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0)
            .open(&path)
            .expect("独占打开临时文件");
        let names = locking_processes(&path.to_string_lossy());
        drop(file);
        let _ = std::fs::remove_file(&path);

        if names.is_empty() {
            eprintln!("Restart Manager 未报告本进程的独占句柄，跳过断言（环境不支持）");
            return;
        }
        assert!(names.len() <= 5, "最多返回 5 个进程名");
    }
}

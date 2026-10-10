//! 通过 Windows 网络资源枚举获取“网上邻居”可见的 SMB 设备与共享。
use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SmbNeighbor {
    pub server: String,
    pub shares: Vec<String>,
}

#[cfg(windows)]
fn shell_neighbors() -> Vec<String> {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    // 资源管理器“网络”包含 WSD/SSDP 发现的设备；只接受其中带 UNC 路径的电脑。
    const SCRIPT: &str = r#"[Console]::OutputEncoding=[Text.Encoding]::UTF8
$shell=New-Object -ComObject Shell.Application
$folder=$shell.Namespace('shell:NetworkPlacesFolder')
if($null -ne $folder){ foreach($item in $folder.Items()){ if($item.Path -match '^\\\\[^\\]+$'){ [Console]::WriteLine($item.Path) } } }"#;
    let Ok(mut child) = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return Vec::new();
    };
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            _ => {
                let _ = child.kill();
                break;
            }
        }
    }
    let Ok(output) = child.wait_with_output() else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let server = line.trim().strip_prefix("\\\\")?;
            if server.is_empty() || server.contains(['\\', '/']) {
                None
            } else {
                Some(server.to_owned())
            }
        })
        .collect()
}

#[cfg(windows)]
pub fn list_shares(server: &str) -> Result<Vec<String>, String> {
    use std::ptr;
    use windows_sys::Win32::Foundation::ERROR_MORE_DATA;
    use windows_sys::Win32::NetworkManagement::NetManagement::NetApiBufferFree;
    use windows_sys::Win32::Storage::FileSystem::{
        NetShareEnum, SHARE_INFO_1, STYPE_DISKTREE, STYPE_SPECIAL,
    };

    if server.is_empty() || server.contains(['\\', '/', ':']) {
        return Err("设备名称无效".into());
    }
    let wide: Vec<u16> = format!("\\\\{server}")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut shares = Vec::new();
    let mut resume = 0;
    loop {
        let mut buffer: *mut u8 = ptr::null_mut();
        let mut read = 0;
        let mut total = 0;
        let status = unsafe {
            NetShareEnum(
                wide.as_ptr(),
                1,
                &mut buffer,
                u32::MAX,
                &mut read,
                &mut total,
                &mut resume,
            )
        };
        if !buffer.is_null() {
            let rows =
                unsafe { std::slice::from_raw_parts(buffer.cast::<SHARE_INFO_1>(), read as usize) };
            for row in rows {
                if row.shi1_type & 0xffff == STYPE_DISKTREE && row.shi1_type & STYPE_SPECIAL == 0 {
                    let mut length = 0;
                    unsafe {
                        while *row.shi1_netname.add(length) != 0 {
                            length += 1
                        }
                    }
                    let name = unsafe {
                        String::from_utf16_lossy(std::slice::from_raw_parts(
                            row.shi1_netname,
                            length,
                        ))
                    };
                    if !name.is_empty() && !name.contains(['\\', '/']) {
                        shares.push(format!("\\\\{server}\\{name}"));
                    }
                }
            }
            unsafe { NetApiBufferFree(buffer.cast()) };
        }
        if status != ERROR_MORE_DATA {
            if status != 0 {
                return Err(format!(
                    "无法列出 {server} 的共享（Windows 错误码 {status}）"
                ));
            }
            break;
        }
    }
    shares.sort_by_key(|share| share.to_lowercase());
    Ok(shares)
}

#[cfg(not(windows))]
pub fn list_shares(_server: &str) -> Result<Vec<String>, String> {
    Err("局域网 SMB 共享仅支持 Windows".into())
}

#[cfg(windows)]
pub fn discover() -> Result<Vec<SmbNeighbor>, String> {
    use std::collections::BTreeMap;
    use std::ffi::c_void;
    use std::ptr;
    use windows_sys::Win32::Foundation::{ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, HANDLE};
    use windows_sys::Win32::NetworkManagement::WNet::{
        WNetCloseEnum, WNetEnumResourceW, WNetOpenEnumW, NETRESOURCEW, RESOURCETYPE_ANY,
        RESOURCEUSAGE_CONTAINER, RESOURCE_CONNECTED, RESOURCE_CONTEXT, RESOURCE_GLOBALNET,
    };

    struct EnumHandle(HANDLE);
    impl Drop for EnumHandle {
        fn drop(&mut self) {
            unsafe { WNetCloseEnum(self.0) };
        }
    }

    unsafe fn wide_string(ptr: *const u16) -> String {
        if ptr.is_null() {
            return String::new();
        }
        let mut len = 0;
        while *ptr.add(len) != 0 {
            len += 1
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len))
    }

    fn collect(
        scope: u32,
        parent: Option<&NETRESOURCEW>,
        depth: usize,
        remaining: &mut usize,
        found: &mut BTreeMap<String, (String, Vec<String>)>,
    ) -> Result<(), u32> {
        if depth > 5 || *remaining == 0 {
            return Ok(());
        }
        let mut raw = ptr::null_mut();
        let status = unsafe {
            WNetOpenEnumW(
                scope,
                RESOURCETYPE_ANY,
                0,
                parent.map_or(ptr::null(), |item| item),
                &mut raw,
            )
        };
        if status != 0 {
            return Err(status);
        }
        let handle = EnumHandle(raw);
        let mut buffer = vec![0u64; 2048]; // 16 KiB；NETRESOURCEW 需要指针对齐。
        loop {
            let mut count = u32::MAX;
            let mut size = (buffer.len() * std::mem::size_of::<u64>()) as u32;
            let status = unsafe {
                WNetEnumResourceW(
                    handle.0,
                    &mut count,
                    buffer.as_mut_ptr().cast::<c_void>(),
                    &mut size,
                )
            };
            if status == ERROR_NO_MORE_ITEMS {
                break;
            }
            if status == ERROR_MORE_DATA {
                if size > 256 * 1024 {
                    break;
                }
                buffer.resize((size as usize).div_ceil(8).max(buffer.len() * 2), 0);
                continue;
            }
            if status != 0 {
                return Err(status);
            }
            let resources = unsafe {
                std::slice::from_raw_parts(buffer.as_ptr().cast::<NETRESOURCEW>(), count as usize)
            };
            for resource in resources {
                if *remaining == 0 {
                    return Ok(());
                }
                *remaining -= 1;
                let path = unsafe { wide_string(resource.lpRemoteName) };
                let parts: Vec<_> = path.split('\\').filter(|part| !part.is_empty()).collect();
                if path.starts_with("\\\\") && !parts.is_empty() {
                    let server = parts[0].to_string();
                    let entry = found
                        .entry(server.to_lowercase())
                        .or_insert_with(|| (server, Vec::new()));
                    if parts.len() >= 2 && resource.dwType == 1 {
                        let share = format!("\\\\{}\\{}", parts[0], parts[1]);
                        if !entry
                            .1
                            .iter()
                            .any(|existing| existing.eq_ignore_ascii_case(&share))
                        {
                            entry.1.push(share);
                        }
                    }
                }
                if resource.dwUsage & RESOURCEUSAGE_CONTAINER != 0 {
                    let _ = collect(
                        RESOURCE_GLOBALNET,
                        Some(resource),
                        depth + 1,
                        remaining,
                        found,
                    );
                }
            }
        }
        Ok(())
    }

    let mut found = BTreeMap::new();
    for server in shell_neighbors() {
        found
            .entry(server.to_lowercase())
            .or_insert((server, Vec::new()));
    }
    let mut remaining = 512;
    let _ = collect(RESOURCE_CONNECTED, None, 0, &mut remaining, &mut found);
    let context_result = if found.is_empty() {
        collect(RESOURCE_CONTEXT, None, 0, &mut remaining, &mut found)
    } else {
        Ok(())
    };
    if found.is_empty() {
        let _ = collect(RESOURCE_GLOBALNET, None, 0, &mut remaining, &mut found);
    }
    if found.is_empty() {
        if let Err(status) = context_result {
            return Err(format!("无法扫描 Windows 网络资源（错误码 {status}）"));
        }
    }
    let mut neighbors: Vec<_> = found
        .into_values()
        .map(|(server, mut shares)| {
            shares.sort_by_key(|share| share.to_lowercase());
            SmbNeighbor { server, shares }
        })
        .collect();
    neighbors.sort_by_key(|item| item.server.to_lowercase());
    Ok(neighbors)
}

#[cfg(not(windows))]
pub fn discover() -> Result<Vec<SmbNeighbor>, String> {
    Err("局域网 SMB 自动发现仅支持 Windows".into())
}

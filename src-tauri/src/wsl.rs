//! 列出已安装的 WSL 发行版，供“此电脑 / Linux”目录树使用。

#[cfg(windows)]
pub fn list_distros() -> Result<Vec<String>, String> {
    use std::os::windows::process::CommandExt;
    // CREATE_NO_WINDOW：GUI 进程启动控制台程序时若不指定，会弹出短暂的控制台窗口。
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let output = match std::process::Command::new("wsl.exe")
        .args(["--list", "--quiet"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("无法启动 WSL：{error}")),
    };
    if !output.status.success() {
        // 未安装发行版时 wsl.exe 以非零状态退出，“Linux”入口无需显示。
        return Ok(Vec::new());
    }
    let bytes = &output.stdout;
    let utf16 = bytes.starts_with(&[0xff, 0xfe])
        || bytes
            .chunks_exact(2)
            .take(128)
            .filter(|pair| pair[1] == 0)
            .count()
            > 4;
    let decoded = if utf16 {
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    };
    let mut names = Vec::new();
    for line in decoded.lines() {
        let name = line.trim_matches(['\u{feff}', '\0', '\r', ' ', '\t']);
        if name.is_empty() || name.contains(['\\', '/']) || name == "." || name == ".." {
            continue;
        }
        if !names
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(name))
        {
            names.push(name.to_owned());
        }
    }
    Ok(names)
}

#[cfg(not(windows))]
pub fn list_distros() -> Result<Vec<String>, String> {
    Ok(Vec::new())
}

// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // 提权子进程：只执行一次文件操作并把结果写成 JSON，绝不初始化 Tauri
    // （不建单实例互斥体、不建窗口 / WebView2、不注册托盘），避免与主实例冲突。
    if let Some(code) = stelix_file_manager_lib::try_run_elevated_helper() {
        std::process::exit(code);
    }
    stelix_file_manager_lib::run()
}

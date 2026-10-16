fn main() {
    // 图标目录变更时重新编译资源，否则 exe 内嵌图标不会更新
    println!("cargo:rerun-if-changed=icons");
    tauri_build::build()
}

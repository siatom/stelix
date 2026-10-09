# File Povit

![File Povit 标志](public/app-icon.svg)

基于 Tauri 2、Vue 3 和 TypeScript 构建的桌面文件管理器。

## 开发环境

- Node.js
- pnpm
- Rust 工具链（运行桌面端时需要）

## 开始开发

```bash
pnpm install
pnpm tauri dev
```

仅启动前端页面：

```bash
pnpm dev
```

## 构建

```bash
pnpm build
pnpm tauri build
```

## 技术栈

- Vue 3、TypeScript、Vite
- Tauri 2、Rust
- Pinia

## 文件索引

Windows 首次启动时会建立 NTFS MFT 索引，并将索引缓存保存在本机应用数据目录。后续启动会载入缓存，并通过 NTFS USN 变更日志补齐期间的文件变化；如果缓存不可用或变更日志已不再包含所需记录，应用会重新扫描对应卷。非 NTFS 卷仍使用目录遍历更新索引。

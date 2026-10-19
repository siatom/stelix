# Stelix File Manager 星索

![Stelix File Manager 星索标志](public/app-icon.svg)

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

## 远程文件传输

在左侧「网络 → FTP/SFTP」中添加主机并连接后，可在文件窗格中浏览远端目录，并通过以下方式与本地互传：

- 把远程文件 / 文件夹拖到本地窗格，或复制后粘贴到本地目录：下载（目录递归）。
- 把本地文件 / 文件夹拖到远程窗格，或复制后粘贴到远程目录：上传（目录递归，空目录也会创建）。
- 远程窗格右键「上传文件… / 上传文件夹… / 下载到…」可直接选择本机文件或下载位置。
- 远程与本地之间拖拽默认按复制处理；使用剪切（Ctrl+X）粘贴时，源文件会在传输成功后删除。

SFTP 依赖带 libssh2 的 `curl.exe`（Windows 自带的 `System32\curl.exe` 不含 SFTP）。应用会自动从 PATH、Git for Windows 安装目录与常见 curl 安装位置中查找；也可以通过环境变量 `STELIX_CURL` 指定 curl 的完整路径。首次连接新主机会沿用 curl 的 SSH 主机密钥校验（`~/.ssh/known_hosts`）。

## AI 大模型接入

在“设置 → AI 大模型”中添加 Provider，填写基础地址、接口协议和 API Key，再添加服务商提供的模型 ID。每个 Provider 可以配置多个模型，中转服务的模型可以来自不同厂商。在“当前使用”中选择 Provider 和模型，启用 AI 后保存；AI 助手和定时任务规划使用同一配置。

支持 OpenAI Chat Completions、OpenAI Responses、Anthropic Messages 和 Gemini GenerateContent。厂商选项用于预填接入信息，实际兼容性由所选协议和服务商支持决定；其他服务可以通过自定义地址接入。基础地址不要包含最终请求路径，系统会按协议追加路径。本地服务无需认证时可以留空密钥，文件工具调用需要模型支持 function calling。

“测试连接”只发送一条简短请求，不发送文件内容，也不保存配置；测试第三方服务可能产生少量接口费用。关闭 AI 或请求失败时使用离线规则解析。旧的单 Provider 配置会自动读取为新结构，保存后使用新的配置格式。

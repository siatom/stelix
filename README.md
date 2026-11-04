<p align="center">
  <img src="src-tauri/icons/icon.png" width="96" height="96" alt="Stelix" />
</p>


# Stelix File Manager 星索

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

## 持续集成与发布

GitHub Actions 工作流位于 `.github/workflows/`：

- `ci.yml`：推送到 `main`、提交 PR 或手动触发时运行。`frontend` 任务在 Ubuntu 上执行 `pnpm build`（`vue-tsc` 类型检查 + Vite 构建）；`rust` 任务在 Windows 上执行 `cargo clippy --all-targets` 和 `cargo test`，并缓存前端与 Cargo 依赖（clippy 暂不阻断：现有代码仍有少量 warning，清理完再加 `-- -D warnings`）。
- `release.yml`：推送 `v*` 形式的 tag 时触发，在 Windows 上构建 NSIS 安装包并创建 GitHub Release 草稿。

发布新版本：

```bash
git tag v0.2.0
git push origin v0.2.0   # origin 同时推送到 GitHub 与 gitee
```

版本号以 tag 为准：工作流用 `scripts/sync-version.mjs` 把 `v0.2.0` 写入 `package.json`、`src-tauri/tauri.conf.json`、`Cargo.toml` 和 `Cargo.lock` 后再打包，所以安装包文件名与 exe 元数据跟 tag 一致。本地需要同步时执行 `node scripts/sync-version.mjs v0.2.0`。

发布默认创建草稿，确认产物无误后再在 GitHub 上发布。目前只构建 Windows x64 安装包，且安装包未做代码签名，首次运行可能触发 SmartScreen 提示。

## 技术栈

- Vue 3、TypeScript、Vite
- Tauri 2、Rust
- Pinia

## 文件索引

索引由目录遍历建立（walkdir + 并行读取元数据），只索引设置中配置的根目录（默认为用户主目录），并自动跳过 AppData、node_modules、target 等缓存与构建目录。索引只保存在内存、不落盘，每次启动在后台重建；运行期新增或删除的文件需要等下次启动重扫才会进入索引。

## 远程连接与文件传输

在左侧「网络 → 远程」中添加主机即可连接远程服务器，支持密码与 SSH 私钥两种认证方式（私钥密码短语可选，凭据保存在系统凭据库）。保存前可点「测试连接」验证地址与凭据（成功时显示耗时与根目录条目数）。连接主机时会自动切换为「左本地 / 右远程」双窗格；不同主机会在右窗格以独立标签并存，可同时连接多台服务器。

远端目录浏览与互传：

- 把远程文件 / 文件夹拖到本地窗格，或复制后粘贴到本地目录：下载（目录递归）。
- 把本地文件 / 文件夹拖到远程窗格，或复制后粘贴到远程目录：上传（目录递归，空目录也会创建）。
- 远程窗格右键「上传文件… / 上传文件夹… / 下载到…」可直接选择本机文件或下载位置。
- 远程与本地之间拖拽默认按复制处理；使用剪切（Ctrl+X）粘贴时，源文件会在传输成功后删除。

SFTP 主机行右侧的终端按钮会在文件窗格中新增一个 SSH 终端标签（与目录标签、多主机标签并存，支持窗口尺寸同步与 Ctrl+Shift+C / Ctrl+Shift+V 复制粘贴）。SFTP 文件传输与 SSH 终端共用内置 SSH 实现（russh），不依赖 curl，支持 `ssh-ed25519` / ECDSA / RSA 主机密钥；主机密钥按 accept-new 校验：首次连接自动记录到 `~/.ssh/known_hosts`，之后密钥变化会拒绝连接。

FTP 仍使用系统 `curl.exe`，可通过环境变量 `STELIX_CURL` 指定 curl 的完整路径。

## 云存储

左侧「网络 → 云存储」可添加百度网盘与迅雷云盘账号，浏览方式与远程主机一致：点击账号进入「左本地 / 右远程」双窗格，支持目录展开、下载、拖拽与复制粘贴传输；下载目录会自动递归。

- **百度网盘**：在百度网盘开放平台创建应用，回调地址填写 `http://127.0.0.1:17890/oauth/callback`，然后把 AppKey 与 SecretKey 填入 Stelix。登录时会在系统浏览器完成百度账号授权。支持浏览、下载与上传（按 4MB 分片上传）。
- **迅雷云盘**：使用迅雷账号密码登录；若触发人机验证，可先打开验证链接完成验证，再改用刷新令牌登录。当前支持浏览与下载，暂不支持在 Stelix 内上传。

云存储令牌保存在系统凭据库，应用内只保存账号标识与显示名。

## AI 大模型接入

在“设置 → AI 大模型”中添加 Provider，填写基础地址、接口协议和 API Key，再添加服务商提供的模型 ID。每个 Provider 可以配置多个模型，中转服务的模型可以来自不同厂商。在“当前使用”中选择 Provider 和模型，启用 AI 后保存；AI 助手和定时任务规划使用同一配置。

支持 OpenAI Chat Completions、OpenAI Responses、Anthropic Messages 和 Gemini GenerateContent。厂商选项用于预填接入信息，实际兼容性由所选协议和服务商支持决定；其他服务可以通过自定义地址接入。基础地址不要包含最终请求路径，系统会按协议追加路径。本地服务无需认证时可以留空密钥，文件工具调用需要模型支持 function calling。

“测试连接”只发送一条简短请求，不发送文件内容，也不保存配置；测试第三方服务可能产生少量接口费用。关闭 AI 或请求失败时使用离线规则解析。旧的单 Provider 配置会自动读取为新结构，保存后使用新的配置格式。

AI 助手会把同一轮的写操作集中成一批，逐项展示影响范围、冲突/跨盘/文件占用等只读预检结果，由用户勾选后执行；执行完成会继续交给模型总结，模型仍可发起下一批操作。每次写操作都会追加到 `{配置目录}/ai-audit.jsonl` 审计日志。等待审批的任务只保存在内存中，切换会话、新建对话或应用重启后会失效，不会在后台自动执行。

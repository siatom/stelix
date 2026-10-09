---
name: cdp-verify
description: >-
  在 Windows 上通过 WebView2 的 CDP 检查 File Povit 的运行界面、读取 DOM 状态并模拟点击、输入和拖拽。
  适用于 Vue 界面改动或确认 Tauri 开发版是否正常运行。项目没有自带 CDP 自动化脚本；使用前需有可用的
  cdp-check.cjs（例如由调用环境提供）。文件选择器等原生窗口不属于 WebView，不能用 CDP 操作。
---

# File Povit 界面验证

File Povit 是 Vue 3 + Vite + Tauri 2 桌面文件管理器。Windows 开发版使用 WebView2，可在启动时启用 CDP，
再对页面做截图、DOM 求值和输入操作。CDP 验证前端 WebView 行为；文件系统操作仍需在安全的临时目录中实际验证。

## 1. 启动开发版并开放 CDP

项目使用 pnpm，Vite 开发服务器固定在 `http://localhost:1420`。在 PowerShell 的项目根目录执行：

```powershell
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9222'
pnpm tauri dev
```

环境变量只对该终端及其启动的子进程生效。启动前设置后，新建的 WebView2 才会开放调试端口；已运行的安装版或旧开发进程不会因此获得 CDP。验证完关闭本次开发进程后，可清除变量：

```powershell
Remove-Item Env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS -ErrorAction SilentlyContinue
```

项目没有 `scripts/dev-local.sh`、`/dev-local` 工作流或 `scripts/cdp-check.cjs`。启动使用上面的项目命令；如需使用下方简化命令，调用环境必须另外提供 `cdp-check.cjs`，不要假设该脚本存在于仓库。

确认调试端口及页面：

```powershell
Get-NetTCPConnection -LocalPort 9222 -State Listen -ErrorAction SilentlyContinue
Invoke-RestMethod http://127.0.0.1:9222/json
```

页面目标应是 File Povit 的 WebView，而非 `about:blank`。窗口标题默认为 `File Povit`；本项目当前只有一个 Tauri 窗口，通常无需指定 target。

> `src-tauri/tauri.conf.json` 的应用标识是 `com.filepovit.desktop`，开发版未配置单实例插件或独立标识。避免同时启动多个开发版；若安装版也使用相同标识，先确认它不会与开发版争用 WebView2 用户数据目录。不要结束或关闭用户正在使用的安装版来腾出端口。

## 2. CDP 检查脚本

项目当前不包含 CDP 客户端脚本。若运行环境提供了 `cdp-check.cjs`，在仓库根目录按其参数调用，例如：

```powershell
node <脚本路径> '{"shot":"file-povit.png"}'
node <脚本路径> '{"target":"File Povit","eval":"document.title"}'
```

常用参数（取决于所用脚本的实现）：

| 参数 | 用途 |
|---|---|
| `port` | CDP 端口，默认 `9222` |
| `target` | 按页面 URL 或标题匹配目标窗口；本项目标题为 `File Povit` |
| `wait` | 操作前等待毫秒数 |
| `click` | 页面坐标点击 `[x,y]` |
| `drag` | 页面坐标拖拽 `[x1,y1,x2,y2]` |
| `keys` / `insertText` | 输入按键或文本；具体支持范围由脚本决定 |
| `eval` | 在页面中求值 DOM 或 JavaScript 表达式 |
| `shot` | 保存页面截图 |

不要直接复制其他项目特有的终端、工作区、Monaco 或窗口布局选择器。File Povit 的主要界面是侧边栏、左右双窗格、路径栏、文件列表、标签栏、状态栏和可选 AI 面板；先通过截图/DOM 检查当前页面，再按改动选择对应区域。

双窗格分隔条由 `mousedown` / `mousemove` / `mouseup` 调整，验证拖动时使用 CDP 的真实鼠标输入。若 CDP 脚本支持 `drag`，可按截图测量页面坐标后使用；不要用 `eval` 合成 `PointerEvent` 代替真实输入。

## 3. 验证边界与注意事项

- WebView 页面中的 HTML 控件和 Vue 交互可通过 CDP 检查；Windows 原生文件选择器、系统托盘和原生窗口不能通过页面 CDP 操作。
- 文件列表、复制/移动/删除等功能依赖 Tauri Rust 命令。用专门的临时目录验证，不要对真实用户文件执行删除或覆盖操作。
- `localStorage`、Pinia 持久化数据和窗口状态可能是用户数据。若验证修改了持久化设置，结束前恢复原值；优先使用一次性测试状态。
- 窗口标题和路由以运行中的页面为准；当前配置标题是 `File Povit`，启动 URL 是 `http://localhost:1420`。
- 代码改动可能触发 Vite HMR。涉及状态或交互的验证要留意页面重载，并在重载后重新检查结果。

## 4. 排障

| 现象 | 检查方式 |
|---|---|
| 9222 未监听 | 确认启动前设置了 `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`，并确认运行的是新启动的 Windows 开发版 |
| `/json` 没有页面目标 | 确认 `pnpm tauri dev` 已成功打开窗口，且端口未被其他进程占用；查看 `Get-NetTCPConnection -LocalPort 9222` |
| 找到的是错误页面 | 用 `Invoke-RestMethod http://127.0.0.1:9222/json` 查看各 target 的 URL 和标题，选择 File Povit 页面 |
| 页面能检查但原生对话框不可见 | 这是 CDP 的边界；手动操作原生对话框，并在安全临时目录验证文件操作 |
| 重启开发版后端口仍不可用 | 确认旧 WebView2 进程已随开发版退出；不要用结束进程的方式关闭用户安装版 |

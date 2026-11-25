<p align="center">
  <img src="src-tauri/icons/icon.png" width="96" height="96" alt="Stelix app icon" />
</p>

<h1 align="center">Stelix File Manager</h1>

<p align="center">
  A cross-platform desktop file manager built with Tauri 2, Vue 3, TypeScript, and Rust.
</p>

<p align="center"><a href="README.zh-CN.md">简体中文</a></p>

Stelix supports Windows and macOS. Both platforms share the main file-management workflows. Features tied to Windows, such as Everything search, UAC elevation, Windows Shell icons, and WSL integration, are disabled or replaced with native macOS behavior where appropriate.

## Features

- **Local file management:** Browse configured locations, use a dual-pane workspace, and work with files and folders through the native filesystem integration.
- **Background file index:** Builds an in-memory index for configured roots (the home directory by default). AppData, `node_modules`, `target`, and other configured cache or build folders are skipped. The index is rebuilt at startup and is not persisted to disk; changes made while the app is running appear after the next scan.
- **Remote connections:** Browse SFTP and FTP servers alongside local files. SFTP supports password or private-key authentication, recursive upload and download, and an SSH terminal tab. SFTP credentials are stored in the operating system credential store.
- **Cloud storage:** Browse Baidu Netdisk and Xunlei Cloud Drive accounts, with directory browsing and downloads. Baidu Netdisk also supports uploads; Xunlei uploads are not currently available.
- **AI providers:** Connect compatible OpenAI Chat Completions, OpenAI Responses, Anthropic Messages, or Gemini GenerateContent endpoints. Configure multiple providers and models, including services that expose compatible APIs.
- **Reviewed AI file operations:** Proposed write operations are grouped for review. Stelix shows preflight details such as affected paths, conflicts, cross-drive operations, and file-lock conditions, then executes only the operations you approve. Actions are recorded in an append-only `ai-audit.jsonl` file in the app configuration directory.

## Platform support

| Capability | Windows | macOS |
| --- | --- | --- |
| Local file browsing and operations | Yes | Yes |
| SFTP, FTP, and cloud connections | Yes | Yes |
| SSH terminal | Yes | Yes |
| Everything search | Windows only | Not available |
| UAC elevation | Windows only | Not available |
| Windows Shell icons and WSL integration | Windows only | Not available |
| Native trash and terminal integration | Windows native behavior | macOS native behavior |

## Download

Visit the [GitHub Releases](https://github.com/siatom/stelix/releases) page for published builds. Release builds are currently unsigned: Windows may show a SmartScreen warning, and macOS may require approval under **Privacy & Security** before the app can be opened.

## Requirements

- Node.js 24 or a compatible version supported by the project toolchain
- pnpm (the repository pins its package manager version in `package.json`)
- Rust stable toolchain
- Platform build tools required by Tauri

On macOS, install Xcode Command Line Tools to provide Clang and the macOS SDK:

```bash
xcode-select --install
```

## Development

Install JavaScript dependencies and start the desktop app:

```bash
pnpm install
pnpm tauri dev
```

To run only the Vite frontend in a browser:

```bash
pnpm dev
```

The frontend uses Vue 3, TypeScript, Vite, and Pinia. Native filesystem and operating-system operations are implemented in Rust and exposed through the existing Tauri bridge.

## Build and checks

Build the frontend and package the desktop app on the current platform:

```bash
pnpm build
pnpm tauri build
```

Run the Rust checks used by the repository's CI workflow:

```bash
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets
cargo test --manifest-path src-tauri/Cargo.toml
```

The frontend build runs `vue-tsc --noEmit` before Vite, so it also checks Vue and TypeScript types. There is currently no separate frontend test or lint script.

## Remote connections and file transfer

Add servers under **Network → Remote**. Stelix supports password and SSH private-key authentication; a key passphrase is optional. Use **Test Connection** to check the credentials and display the response time and number of entries in the remote root. Opening a server switches the workspace to a local/remote dual-pane layout. Each host has its own tab, so multiple hosts can remain open.

Transfer files between panes by dragging or copying and pasting:

- Remote to local copies files or folders to the selected local directory. Folder downloads are recursive.
- Local to remote uploads files or folders recursively, including empty folders.
- The remote pane's context menu also offers **Upload File…**, **Upload Folder…**, and **Download to…**.
- Cross-pane drag-and-drop copies by default. When using cut and paste (`Ctrl+X`), the source is removed only after the transfer succeeds.

The terminal button on an SFTP host opens an SSH terminal tab in the file workspace. It can coexist with directory and host tabs, follows window-size changes, and supports `Ctrl+Shift+C` / `Ctrl+Shift+V` for copy and paste. SFTP file transfer and the terminal use the built-in `russh` implementation and do not depend on `curl`.

SSH host keys use an accept-new policy: the first connection records the key in `~/.ssh/known_hosts`; a changed key is rejected. FTP uses the system `curl.exe`. Set `STELIX_CURL` to the full path to a different curl executable if needed.

## Cloud storage

Open **Network → Cloud Storage** to add Baidu Netdisk or Xunlei Cloud Drive accounts. Selecting an account opens the same local/remote dual-pane workflow, with directory browsing, drag-and-drop, copy and paste, and recursive directory downloads.

- **Baidu Netdisk:** Create an application on the Baidu Netdisk Open Platform. Set its callback URL to `http://127.0.0.1:17890/oauth/callback`, then enter the App Key and Secret Key in Stelix. Sign in through the system browser. Browsing, downloads, and uploads are supported; uploads use 4 MB parts.
- **Xunlei Cloud Drive:** Sign in with a Xunlei account and password. If verification is required, open the verification link first, then sign in with a refresh token. Browsing and downloads are supported; in-app uploads are not currently supported.

Cloud-storage tokens are kept in the operating system credential store. Stelix stores only the account identifier and display name in its app data.

## AI provider setup and behavior

Open **Settings → AI Models** to add a provider. Enter its base URL, API protocol, and API key, then add the model ID supplied by that service. A provider can contain multiple models. Choose a provider and model under **Current Model**, enable AI, and save. The AI assistant and scheduled-task planner share this configuration.

The provider presets help fill in connection details; actual compatibility depends on the selected protocol and the service. For custom services, enter a custom base URL. Do not include the final request path in the base URL because Stelix appends the path required by the selected protocol. A local service that does not require authentication can use an empty API key. File-tool calls require a model that supports function calling.

**Test Connection** sends one short request; it does not send file contents or save the configuration. Testing a third-party service may incur a small API charge. When AI is disabled or a request fails, Stelix falls back to offline rule-based parsing. Older single-provider settings are read into the new structure and converted when saved.

For AI-assisted file changes, the assistant gathers proposed writes into batches and presents their preflight results for approval. Pending approval exists only in memory and expires when you switch sessions, start a new conversation, or restart the app. Nothing runs in the background while awaiting approval. Completed writes are appended to the audit log in the app configuration directory.

## CI and releases

GitHub Actions workflows are in `.github/workflows/`:

- **CI** runs on pushes to `main`, pull requests, and manual dispatch. The frontend job installs dependencies and runs `pnpm build`. Rust jobs run Clippy and `cargo test` on Windows and macOS Apple Silicon.
- **Release** runs when a `v*` version tag is pushed. It builds a Windows x64 NSIS installer, a macOS Apple Silicon DMG, and a macOS Intel DMG, then creates or updates a draft GitHub Release.

The release workflow checks that the tag matches the version in `package.json`. It then synchronizes the version from the tag into the Tauri and Cargo configuration before packaging. To prepare a release, update the project version files and lockfile, commit the release changes, and push a matching tag, for example:

```bash
git tag v0.4.0
git push origin v0.4.0
```

The workflow creates a draft. Review all platform assets before publishing it on GitHub. macOS builds are not signed or notarized; users may need to approve the app in **Privacy & Security** on first launch.

## Repository layout

- `src/` — Vue and TypeScript frontend (`components/`, `api/`, `shell/`, `stores/`, `styles/`, and `assets/`)
- `src-tauri/` — Tauri configuration, Rust application code, native integrations, and capabilities
- `public/` — static web assets
- `.github/workflows/` — CI and release workflows
- `scripts/` — version synchronization and packaging helpers

`dist/` and `src-tauri/target/` are generated build output.

## Technology

- Vue 3, TypeScript, Vite, and Pinia
- Tauri 2 and Rust
- russh for SFTP and SSH terminal support

## License

Stelix is released under the [MIT License](LICENSE).

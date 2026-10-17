# BUGFIX TODO（本轮缺陷修复）

基线：`main` @ `90134d7`。执行方式：每个修复组一个独立 git worktree + 临时分支 `wt/<组>`，
subagent 在 worktree 内修复、自测并 commit；主 agent 在主工作区串行合并到 `main` 后焚毁 worktree。
主 agent 不直接修改业务代码。

状态图例：⏳ 进行中 · ✅ 已合并 · ⛔ 失败

## 修复组

| 组 | 缺陷 | 涉及文件 | 严重性 | 分支 | 状态 |
|---|---|---|---|---|---|
| T1 | 路径根判定不规范化（`C:\Windows\..` 绕过）；移动到不存在的自身子目录触发无限递归 | `src-tauri/src/fs_core/mod.rs` | 高 | `wt/fix-fscore-path-guard` | ✅ `c899de1` / merge `de77f7d` |
| T2 | 空筛选约束绕过防误删护栏 → 递归删除；触发时间字段无范围校验 / interval 溢出 | `src-tauri/src/schedule/mod.rs`、`src-tauri/src/ai_agent/mod.rs` | 高/中 | `wt/fix-schedule-guard` | ✅ `e62d069` / merge `db0a7a2` |
| T3 | 检索范围前缀匹配缺目录边界，同名前缀目录误命中 | `src-tauri/src/search_engine/mod.rs` | 中 | `wt/fix-search-scope` | ✅ `ef2e0c2` / merge `a5c4d2e` |
| T4 | 远程列表解析不兼容 ACL/SELinux/macOS 权限标记，整份列表退化为伪条目 | `src-tauri/src/network.rs` | 中 | `wt/fix-remote-listing-parse` | ✅ `e1c2fd0` / merge `617ccb0` |
| T5 | MFT 扫描条目 size 恒为 0；journal 游标循环内 `?` 导致索引缓存永不落盘/每次全量重扫 | `src-tauri/src/index_engine/mft.rs` | 中 | `wt/fix-mft-index` | ✅ `f2b3dbb` / merge `854245b` |
| T6 | 重负载命令未 `async`，在主线程执行导致 UI 卡死 | `src-tauri/src/lib.rs` | 中 | `wt/fix-blocking-commands` | ✅ `27676d8` / merge `07b8140` |
| T7 | AI api_key 明文落盘；配置非原子写致损坏后静默重置；单文件损坏导致对话列表不可用 | `src-tauri/src/persistence/mod.rs`、`src-tauri/src/persistence/ai_history.rs` | 中 | `wt/fix-config-persistence` | ✅ `550efab` / merge `a580a47` |
| T8 | 一轮多个写工具仅最后一个进入确认，其余静默丢弃；索引空结果回退遍历 `C:\`；`strip_any` 切片 panic | `src-tauri/src/ai_agent/mod.rs` | 中/低 | `wt/fix-ai-tool-exec` | ✅ `f0b4bf0` / merge `dfc98a7` |
| T9 | `addTab` 按 path 复用搜索/远程条目；`load` 无请求令牌被过期响应覆盖；UAC 取消被当作成功误删命中；持久化 watch 未跟踪 `kind` | `src/stores/pane.ts` | 中 | `wt/fix-pane-store` | ✅ `1c7876d` / merge `7148bc1` |
| T10 | `dragover` 阶段读 `getData` 致悬停高亮失效；空白右键「重命名」空操作；图标视图 Shift 范围选择索引错位 | `src/components/Pane/FileList.vue` | 中/低 | `wt/fix-filelist-interaction` | ✅ `dd166c1` / merge `0a2e7d0` |

## 最终验收（`main` @ `0a2e7d0`）

- `cargo test --manifest-path src-tauri/Cargo.toml` → **115 passed / 0 failed**
- `cargo check --manifest-path src-tauri/Cargo.toml` → 通过
- `pnpm build`（`vue-tsc --noEmit && vite build`）→ 通过
- `git diff --check` → 通过
- `git worktree list` 仅剩主工作区；`git branch --list 'wt/*'` 无残留
- 未执行 `git push`（默认只合并本地 `main`）

## 验收要求

- 每个修复组在自身 worktree 内通过对应测试：Rust 组 `cargo test`；前端组 `pnpm build`。
- 全部合并后在主工作区运行整体验证：`cargo test`、`cargo check`、`pnpm build`、`git diff --check`。
- 格式检查若仅报基线既有无关文件差异，不顺手格式化，记录事实并确认本轮改动文件已通过格式检查。

## 未纳入本轮

- 前端交互细节与低危项（如 `CommandPalette` 聚焦、`SearchBar` 占位文案、右键菜单快捷键硬编码、
  剪贴板句柄失败路径泄漏、`index_engine` 扫描竞态、watcher 失败目录记入 watched、单色图标提取失败等）
  未纳入本轮 10 组，后续可另行确认。

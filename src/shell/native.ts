/**
 * 桌面外壳行为：屏蔽 WebView 默认的浏览器右键菜单与浏览器级快捷键，
 * 让应用表现得更像原生桌面程序。
 *
 * 约定：
 * - 右键菜单：输入类元素（input / textarea / contenteditable）内保留原生菜单，
 *   以便复制、粘贴、拼写检查等编辑操作；其余区域一律屏蔽。
 * - 快捷键：屏蔽刷新、打印、查看源码、开发者工具、缩放、查找、历史前进/后退等
 *   浏览器行为；不触碰 Ctrl+C/V/X/A/Z/Y 等文本编辑组合键，避免影响正常输入。
 *
 * ⚠ 与本表的关系（应用内快捷键注册表）：`stores/shortcuts.ts` 里的条目**优先于**本表。
 * 若某组合当前是应用内快捷键（或用户正在设置面板里「按下新键位」），本表只
 * `preventDefault()` 阻止浏览器默认行为、**不** `stopPropagation()`，让事件照常
 * 抵达应用层处理器。用户把快捷键改走之后，原组合会自动回落为本表的浏览器级屏蔽。
 */

import { isCapturingShortcut, isRegisteredShortcut } from '../stores/shortcuts'

/** 允许保留原生右键菜单的元素选择器。 */
const CONTEXT_MENU_ALLOWLIST = 'input, textarea, [contenteditable=""], [contenteditable="true"], [data-native-context-menu]'

/** 需要屏蔽的「修饰键 + 按键」组合（键名统一小写）。 */
const BLOCKED_COMBOS: ReadonlyArray<{ ctrl?: boolean; shift?: boolean; alt?: boolean; key: string }> = [
  // 刷新
  { ctrl: true, key: 'r' },
  { ctrl: true, shift: true, key: 'r' },
  // 打印 / 另存
  { ctrl: true, key: 'p' },
  { ctrl: true, key: 's' },
  // 查看源码
  { ctrl: true, key: 'u' },
  // 开发者工具
  { ctrl: true, shift: true, key: 'i' },
  { ctrl: true, shift: true, key: 'j' },
  { ctrl: true, shift: true, key: 'c' },
  // 页面内查找
  { ctrl: true, key: 'f' },
  { ctrl: true, key: 'g' },
  { ctrl: true, key: 'h' },
  // 缩放
  { ctrl: true, key: '=' },
  { ctrl: true, key: '+' },
  { ctrl: true, key: '-' },
  { ctrl: true, key: '0' },
  // 历史前进 / 后退（Alt + 方向键）
  { alt: true, key: 'arrowleft' },
  { alt: true, key: 'arrowright' },
  // 新窗口 / 新标签 / 关闭窗口
  { ctrl: true, key: 'n' },
  { ctrl: true, key: 't' },
  { ctrl: true, key: 'w' },
  { ctrl: true, shift: true, key: 'n' },
  { ctrl: true, shift: true, key: 'w' },
  { ctrl: true, shift: true, key: 't' },
  // 新建隐身窗口（部分环境会触发）
  { ctrl: true, shift: true, key: 'p' },
]

/** 单独按键（无修饰键）屏蔽表，键名统一小写。 */
const BLOCKED_KEYS = new Set(['f5', 'f7', 'f12'])

/** 输入框内需要放行的 Ctrl 组合键（纯文本编辑）。 */
const EDITABLE_SAFE_COMBOS = new Set([
  'a', 'c', 'v', 'x', 'z', 'y',
  'left', 'right', 'up', 'down', 'home', 'end', 'backspace', 'delete',
])

function isEditable(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false
  if (target.isContentEditable) return true
  const tag = target.tagName
  return tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT'
}

function onContextMenu(e: MouseEvent): void {
  const target = e.target as HTMLElement | null
  if (target && target.closest(CONTEXT_MENU_ALLOWLIST)) return
  e.preventDefault()
}

/** 该事件是否命中浏览器级屏蔽组合（供「应用快捷键优先」分支决定要不要阻止默认行为）。 */
function matchesBlockedCombo(e: KeyboardEvent): boolean {
  const key = e.key.toLowerCase()
  const ctrl = e.ctrlKey || e.metaKey
  for (const combo of BLOCKED_COMBOS) {
    if (combo.key !== key) continue
    if (Boolean(combo.ctrl) !== ctrl) continue
    if (Boolean(combo.shift) !== e.shiftKey) continue
    if (Boolean(combo.alt) !== e.altKey) continue
    return true
  }
  return false
}

function onKeyDown(e: KeyboardEvent): void {
  const key = e.key.toLowerCase()
  const ctrl = e.ctrlKey || e.metaKey

  // 「按下新键位」捕获态：放行所有按键到设置面板，只阻止浏览器级默认行为。
  if (isCapturingShortcut()) {
    if (matchesBlockedCombo(e) || (BLOCKED_KEYS.has(key) && !ctrl && !e.altKey)) e.preventDefault()
    return
  }

  // 应用内快捷键优先：只拦浏览器默认行为，放行到应用层。
  // 输入框内同样放行（如 Ctrl+F 打开搜索）；纯文本编辑组合键不在屏蔽表中，不受影响。
  // （用户可把快捷键改到 BLOCKED_COMBOS / BLOCKED_KEYS 里的键位，此时以用户的绑定为准。）
  if (isRegisteredShortcut(e)) {
    if (matchesBlockedCombo(e) || (BLOCKED_KEYS.has(key) && !ctrl && !e.altKey)) e.preventDefault()
    return
  }

  // 输入框内：放行纯文本编辑键，其余浏览器组合键照常屏蔽。
  if (isEditable(e.target)) {
    if (!ctrl && !e.altKey && !BLOCKED_KEYS.has(key)) return
    if (ctrl && !e.shiftKey && !e.altKey && EDITABLE_SAFE_COMBOS.has(key)) return
  }

  if (BLOCKED_KEYS.has(key) && !ctrl && !e.altKey) {
    e.preventDefault()
    e.stopPropagation()
    return
  }

  for (const combo of BLOCKED_COMBOS) {
    if (combo.key !== key) continue
    if (Boolean(combo.ctrl) !== ctrl) continue
    if (Boolean(combo.shift) !== e.shiftKey) continue
    if (Boolean(combo.alt) !== e.altKey) continue
    e.preventDefault()
    e.stopPropagation()
    return
  }
}

/** 阻止把文件拖入页面时 WebView 直接打开该文件。 */
function onDragOver(e: DragEvent): void {
  e.preventDefault()
}

function onDrop(e: DragEvent): void {
  const target = e.target as HTMLElement | null
  if (target && target.closest(CONTEXT_MENU_ALLOWLIST)) return
  e.preventDefault()
}

let installed = false

/** 安装桌面外壳行为；重复调用只会生效一次。 */
export function installNativeShell(): void {
  if (installed) return
  installed = true
  window.addEventListener('contextmenu', onContextMenu)
  window.addEventListener('keydown', onKeyDown, { capture: true })
  window.addEventListener('dragover', onDragOver)
  window.addEventListener('drop', onDrop)
}

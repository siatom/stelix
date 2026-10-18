/**
 * 全局快捷键注册表 —— 应用内键位的唯一事实来源。
 *
 * 设计要点：
 * - 各组件不再硬编码键位判断，统一调用 `match(event)` 查表得到快捷键 id。
 * - 用户改键写入 localStorage（**只存与默认值不同的项**），重启后自动恢复。
 * - `src/shell/native.ts` 的浏览器级拦截表会先问本模块：若该组合当前是应用快捷键
 *   （或正处于「捕获新键位」态），则只 `preventDefault()` 阻止浏览器默认行为、
 *   **不** `stopPropagation()`，事件照常传给应用层。
 *
 * 本模块是普通模块（不是 Pinia store）：`native.ts` 在 Pinia 安装前就装载了
 * 拦截器，用模块级状态可以避免「无 active pinia」的调用问题。
 */
import { ref } from 'vue'

export type ShortcutGroup = 'tabs' | 'files' | 'window' | 'global'

export interface KeyCombo {
  ctrl?: boolean
  shift?: boolean
  alt?: boolean
  /** 小写键名（'a' / 'f2' / 'delete' / 'arrowleft'）；空串表示**未绑定**。 */
  key: string
}

export interface ShortcutDef {
  id: string
  label: string
  group: ShortcutGroup
  default: KeyCombo
}

export interface ShortcutItem extends ShortcutDef {
  current: KeyCombo
}

export const SHORTCUT_GROUP_LABELS: Record<ShortcutGroup, string> = {
  tabs: '标签页',
  files: '文件操作',
  window: '窗口',
  global: '全局',
}

const LS_SHORTCUTS = 'stelix.shortcuts'

/** 快捷键定义表。新增可配置快捷键只需在此追加一条，再在对应组件里按 id 接线。 */
export const SHORTCUT_DEFS: readonly ShortcutDef[] = [
  { id: 'new-tab', label: '新建标签页', group: 'tabs', default: { ctrl: true, key: 't' } },
  { id: 'close-tab', label: '关闭当前标签页', group: 'tabs', default: { ctrl: true, key: 'w' } },
  { id: 'copy', label: '复制', group: 'files', default: { ctrl: true, key: 'c' } },
  { id: 'cut', label: '剪切', group: 'files', default: { ctrl: true, key: 'x' } },
  { id: 'paste', label: '粘贴', group: 'files', default: { ctrl: true, key: 'v' } },
  { id: 'rename', label: '重命名', group: 'files', default: { key: 'f2' } },
  { id: 'delete', label: '删除', group: 'files', default: { key: 'delete' } },
  { id: 'close-window', label: '关闭窗口', group: 'window', default: { alt: true, key: 'f4' } },
  { id: 'search', label: '打开搜索面板', group: 'global', default: { ctrl: true, key: 'f' } },
  { id: 'command-palette', label: '打开命令面板', group: 'global', default: { ctrl: true, key: 'k' } },
]

/** 修饰键自身（按下时尚未构成组合键）。 */
const MODIFIER_KEYS = new Set(['control', 'shift', 'alt', 'meta', 'os'])

/** 允许「不带 Ctrl / Alt」单独绑定的按键（功能键与编辑键）。 */
const BARE_KEYS = new Set([
  'f1', 'f2', 'f3', 'f4', 'f5', 'f6', 'f7', 'f8', 'f9', 'f10', 'f11', 'f12',
  'delete', 'insert', 'home', 'end', 'pageup', 'pagedown',
])

const KEY_LABELS: Record<string, string> = {
  arrowleft: '←',
  arrowright: '→',
  arrowup: '↑',
  arrowdown: '↓',
  escape: 'Esc',
  backspace: 'Backspace',
  delete: 'Delete',
  insert: 'Insert',
  enter: 'Enter',
  tab: 'Tab',
  home: 'Home',
  end: 'End',
  pageup: 'PageUp',
  pagedown: 'PageDown',
  ' ': 'Space',
}

function normalize(raw: KeyCombo | undefined): KeyCombo {
  const key = (raw?.key ?? '').toLowerCase()
  if (!key) return { ctrl: false, shift: false, alt: false, key: '' }
  return { ctrl: !!raw?.ctrl, shift: !!raw?.shift, alt: !!raw?.alt, key }
}

export function comboEquals(a: KeyCombo | undefined, b: KeyCombo | undefined): boolean {
  const x = normalize(a)
  const y = normalize(b)
  return x.ctrl === y.ctrl && x.shift === y.shift && x.alt === y.alt && x.key === y.key
}

/** 键位显示文案，如 `Ctrl+Shift+W`、`F2`、`未设置`。 */
export function formatCombo(combo: KeyCombo | undefined): string {
  const c = normalize(combo)
  if (!c.key) return '未设置'
  const parts: string[] = []
  if (c.ctrl) parts.push('Ctrl')
  if (c.shift) parts.push('Shift')
  if (c.alt) parts.push('Alt')
  parts.push(labelOfKey(c.key))
  return parts.join('+')
}

function labelOfKey(key: string): string {
  const known = KEY_LABELS[key]
  if (known) return known
  if (/^f\d{1,2}$/.test(key)) return key.toUpperCase()
  return key.length === 1 ? key.toUpperCase() : key.charAt(0).toUpperCase() + key.slice(1)
}

/** 从键盘事件取出组合键；纯修饰键按下返回 null。 */
export function eventToCombo(event: KeyboardEvent): KeyCombo | null {
  const key = event.key.toLowerCase()
  if (MODIFIER_KEYS.has(key)) return null
  return normalize({ ctrl: event.ctrlKey || event.metaKey, shift: event.shiftKey, alt: event.altKey, key })
}

function readStored(): Record<string, KeyCombo> {
  try {
    const raw = localStorage.getItem(LS_SHORTCUTS)
    if (!raw) return {}
    const parsed: unknown = JSON.parse(raw)
    return parsed && typeof parsed === 'object' ? (parsed as Record<string, KeyCombo>) : {}
  } catch {
    return {}
  }
}

function buildInitial(): Record<string, KeyCombo> {
  const stored = readStored()
  const out: Record<string, KeyCombo> = {}
  for (const def of SHORTCUT_DEFS) out[def.id] = normalize(stored[def.id] ?? def.default)
  return out
}

/** 当前生效的键位（含默认值，已合并用户自定义）。 */
export const combos = ref<Record<string, KeyCombo>>(buildInitial())

/** 正在等待用户按下新键位的条目 id（null = 未处于捕获态）。 */
export const capturingId = ref<string | null>(null)

export function setCapturing(id: string | null): void {
  capturingId.value = id
}

export function isCapturingShortcut(): boolean {
  return capturingId.value !== null
}

function persist(): void {
  const overrides: Record<string, KeyCombo> = {}
  for (const def of SHORTCUT_DEFS) {
    const current = combos.value[def.id]
    if (current && !comboEquals(current, def.default)) overrides[def.id] = current
  }
  try {
    if (Object.keys(overrides).length) localStorage.setItem(LS_SHORTCUTS, JSON.stringify(overrides))
    else localStorage.removeItem(LS_SHORTCUTS)
  } catch {
    /* 持久化失败不影响本次会话 */
  }
}

function setCombo(id: string, combo: KeyCombo): void {
  combos.value = { ...combos.value, [id]: combo }
  persist()
}

/** 全部条目（含分组与当前键位），供设置面板渲染。 */
export function shortcutItems(): ShortcutItem[] {
  return SHORTCUT_DEFS.map((def) => ({ ...def, current: combos.value[def.id] ?? normalize(def.default) }))
}

/** 该条目是否已被用户改过（与默认键位不同）。 */
export function isCustomized(id: string): boolean {
  const def = SHORTCUT_DEFS.find((d) => d.id === id)
  if (!def) return false
  return !comboEquals(combos.value[id], def.default)
}

export type AssignResult =
  | { ok: true }
  | { ok: false; reason: 'invalid'; message: string }
  | { ok: false; reason: 'conflict'; message: string; conflictId: string }

/**
 * 改键。冲突时**不修改**并返回冲突条目，由调用方提示用户。
 * `key` 为空串表示「解除绑定」（退格清除）。
 */
export function assignCombo(id: string, raw: KeyCombo): AssignResult {
  const def = SHORTCUT_DEFS.find((d) => d.id === id)
  if (!def) return { ok: false, reason: 'invalid', message: '未知快捷键条目' }

  const combo = normalize(raw)
  if (!combo.key) {
    setCombo(id, combo)
    return { ok: true }
  }
  if (!combo.ctrl && !combo.alt && !BARE_KEYS.has(combo.key)) {
    return { ok: false, reason: 'invalid', message: '请使用 Ctrl / Alt 组合键，或 F1–F12、Delete 等功能键' }
  }

  const conflict = SHORTCUT_DEFS.find((other) => {
    if (other.id === id) return false
    const c = combos.value[other.id]
    return !!c?.key && comboEquals(c, combo)
  })
  if (conflict) {
    return {
      ok: false,
      reason: 'conflict',
      message: `与「${conflict.label}」冲突，未修改`,
      conflictId: conflict.id,
    }
  }

  setCombo(id, combo)
  return { ok: true }
}

/** 把单条恢复为默认键位。 */
export function resetShortcut(id: string): void {
  const def = SHORTCUT_DEFS.find((d) => d.id === id)
  if (!def) return
  setCombo(id, normalize(def.default))
}

/** 全部恢复默认并清除持久化键。 */
export function resetAllShortcuts(): void {
  const next: Record<string, KeyCombo> = {}
  for (const def of SHORTCUT_DEFS) next[def.id] = normalize(def.default)
  combos.value = next
  try {
    localStorage.removeItem(LS_SHORTCUTS)
  } catch {
    /* ignore */
  }
}

/**
 * 查表：该键盘事件命中哪条快捷键（未命中返回 null）。
 * 组件内统一用它替代原先硬编码的 `ctrl && key === 'x'` 判断。
 */
export function match(event: KeyboardEvent): string | null {
  const key = event.key.toLowerCase()
  if (MODIFIER_KEYS.has(key)) return null
  const ctrl = event.ctrlKey || event.metaKey

  for (const def of SHORTCUT_DEFS) {
    const combo = combos.value[def.id]
    if (!combo || !combo.key) continue
    if (combo.key !== key) continue
    if (!!combo.ctrl !== ctrl) continue
    if (!!combo.shift !== event.shiftKey) continue
    if (!!combo.alt !== event.altKey) continue
    return def.id
  }
  return null
}

/** 该事件当前是否属于应用内快捷键（供 native.ts 决定是否放行）。 */
export function isRegisteredShortcut(event: KeyboardEvent): boolean {
  return match(event) !== null
}

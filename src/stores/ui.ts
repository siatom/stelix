import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { api, events } from '../api'
import { resetAllShortcuts } from './shortcuts'
import type { IndexProgress } from '../types'

/** 左侧栏面板类型 */
export type PanelKey = 'files' | 'tasks' | 'search' | 'ai'

/** 设置面板分类 */
export type SettingsTab = 'general' | 'appearance' | 'index' | 'shortcuts' | 'ai' | 'about'

/** 主题模式 */
export type ThemeMode = 'dark' | 'light'

/** 在两个窗格之间拖动文件时的默认操作 */
export type PaneDragMode = 'move' | 'copy'

/** 系统默认字体栈，与 tokens.css 的 --font 保持一致；设置为空时回落到这里 */
export const DEFAULT_FONT_STACK = "'Segoe UI Variable', 'Segoe UI', 'Microsoft YaHei', -apple-system, sans-serif"

/** 字体候选，仅用于设置面板的输入补全；实际保存的是用户输入的字体列表 */
export interface UiFontPreset {
  id: string
  name: string
  stack: string
}

export const UI_FONT_PRESETS: readonly UiFontPreset[] = [
  { id: 'system', name: '系统默认', stack: DEFAULT_FONT_STACK },
  { id: 'yahei', name: '微软雅黑', stack: "'Microsoft YaHei UI', 'Microsoft YaHei', 'Segoe UI', sans-serif" },
  { id: 'pingfang', name: '苹方', stack: "'PingFang SC', 'Microsoft YaHei', sans-serif" },
  { id: 'source-han', name: '思源黑体', stack: "'Source Han Sans SC', 'Noto Sans SC', 'Microsoft YaHei', sans-serif" },
  { id: 'dengxian', name: '等线', stack: "'DengXian', 'Microsoft YaHei', sans-serif" },
  { id: 'simhei', name: '黑体', stack: "'SimHei', 'Microsoft YaHei', sans-serif" },
  { id: 'simsun', name: '宋体', stack: "'SimSun', 'Songti SC', serif" },
  { id: 'kaiti', name: '楷体', stack: "'KaiTi', 'STKaiti', serif" },
  { id: 'fangsong', name: '仿宋', stack: "'FangSong', 'STFangsong', serif" },
  { id: 'segoe', name: 'Segoe UI', stack: "'Segoe UI Variable', 'Segoe UI', 'Microsoft YaHei', sans-serif" },
  { id: 'inter', name: 'Inter', stack: "'Inter', 'Segoe UI', 'Microsoft YaHei', sans-serif" },
]

/** 详细视图的列（表头可拖动调整宽度） */
export type DetailColumnKey = 'name' | 'path' | 'size' | 'time' | 'type'

/** 详细视图各列宽度（px）；`name: null` 表示自动占满剩余空间。 */
export interface DetailColumnWidths {
  name: number | null
  path: number
  size: number
  time: number
  type: number
}

/** 回收站视图的列（表头可拖动调整宽度） */
export type TrashColumnKey = 'name' | 'origin' | 'deleted' | 'size'

/** 回收站各列宽度（px）；`name: null` 表示自动占满剩余空间。 */
export interface TrashColumnWidths {
  name: number | null
  origin: number
  deleted: number
  size: number
}

// ---- 本地偏好持久化（localStorage）----
const LS_THEME = 'stelix.theme'
const LS_ACCENT_COLOR = 'stelix.accentColor'
const LS_LIGHT_ACCENT_COLOR = 'stelix.lightAccentColor'
const LS_DARK_ACCENT_COLOR = 'stelix.darkAccentColor'
const LS_UI_OPACITY = 'stelix.uiOpacity'
const LS_SETTINGS_BACKGROUND = 'stelix.settingsBackground'
const SETTINGS_DB_NAME = 'stelix-ui-settings'
const SETTINGS_DB_STORE = 'preferences'
const SETTINGS_BACKGROUND_KEY = 'backgroundImage'
const LS_HIDE_PROTECTED = 'stelix.hideProtected'
const LS_SHOW_HIDDEN = 'stelix.showHidden'
const LS_HIDE_FILE_EXTENSIONS = 'stelix.hideFileExtensions'
const LS_PANE_DRAG_MODE = 'stelix.paneDragMode'
const LS_SIDEBAR_COLLAPSED = 'stelix.sidebarCollapsed'
const LS_FONT_SIZE = 'stelix.fontSize'
const LS_UI_FONT = 'stelix.uiFont'
const LS_SIDEBAR_FONT_SIZE = 'stelix.sidebarFontSize'
const LS_FILE_FONT_SIZE = 'stelix.fileFontSize'
const LS_SIDEBAR_ROW_H = 'stelix.sidebarRowHeight'
const LS_FILE_ROW_H = 'stelix.fileRowHeight'
const LS_DETAIL_COLUMNS = 'stelix.detailColumnWidths'
const LS_TRASH_COLUMNS = 'stelix.trashColumnWidths'

// 详细视图列宽：默认值与 FileList 原有布局一致；名称列默认自适应（null）。
const DETAIL_COL_DEF: DetailColumnWidths = { name: null, path: 240, size: 90, time: 140, type: 90 }
const DETAIL_COL_MIN = 56
const DETAIL_NAME_MIN = 120
const DETAIL_COL_MAX = 1600

// 回收站列宽：与旧版固定列宽一致；名称列默认自适应（null）。
const TRASH_COL_DEF: TrashColumnWidths = { name: null, origin: 240, deleted: 145, size: 96 }

function clampColumnWidth(value: number, min: number): number {
  return Math.min(DETAIL_COL_MAX, Math.max(min, Math.round(value)))
}

/** 校验并读取持久化的列宽（无效 / 缺失的字段回落到默认值）。 */
function readDetailColumnWidths(): DetailColumnWidths {
  try {
    const raw = localStorage.getItem(LS_DETAIL_COLUMNS)
    if (!raw) return { ...DETAIL_COL_DEF }
    const saved = JSON.parse(raw) as Partial<Record<DetailColumnKey, unknown>>
    const width = (key: DetailColumnKey, min: number): number => {
      const value = saved[key]
      return typeof value === 'number' && Number.isFinite(value)
        ? clampColumnWidth(value, min)
        : (DETAIL_COL_DEF[key] as number)
    }
    return {
      name:
        typeof saved.name === 'number' && Number.isFinite(saved.name)
          ? clampColumnWidth(saved.name, DETAIL_NAME_MIN)
          : null,
      path: width('path', DETAIL_COL_MIN),
      size: width('size', DETAIL_COL_MIN),
      time: width('time', DETAIL_COL_MIN),
      type: width('type', DETAIL_COL_MIN),
    }
  } catch {
    return { ...DETAIL_COL_DEF }
  }
}

/** 校验并读取回收站列宽（无效 / 缺失的字段回落到默认值）。 */
function readTrashColumnWidths(): TrashColumnWidths {
  try {
    const raw = localStorage.getItem(LS_TRASH_COLUMNS)
    if (!raw) return { ...TRASH_COL_DEF }
    const saved = JSON.parse(raw) as Partial<Record<TrashColumnKey, unknown>>
    const width = (key: Exclude<TrashColumnKey, 'name'>): number => {
      const value = saved[key]
      return typeof value === 'number' && Number.isFinite(value)
        ? clampColumnWidth(value, DETAIL_COL_MIN)
        : TRASH_COL_DEF[key]
    }
    return {
      name:
        typeof saved.name === 'number' && Number.isFinite(saved.name)
          ? clampColumnWidth(saved.name, DETAIL_NAME_MIN)
          : null,
      origin: width('origin'),
      deleted: width('deleted'),
      size: width('size'),
    }
  } catch {
    return { ...TRASH_COL_DEF }
  }
}

// 行高可选范围与默认值（px）。默认值与 tokens.css 的 --sidebar-row-h / --file-row-h 保持一致。
const SIDEBAR_ROW_MIN = 20
const SIDEBAR_ROW_MAX = 32
const SIDEBAR_ROW_DEF = 26
const FILE_ROW_MIN = 22
const FILE_ROW_MAX = 40
const FILE_ROW_DEF = 26
const SIDEBAR_FONT_SIZE_DEF = 13
const FILE_FONT_SIZE_DEF = 13

// 视图设置默认值（「恢复默认」回到这些值）
const FONT_SIZE_MIN = 11
const FONT_SIZE_MAX = 16
const FONT_SIZE_DEF = 12
const HIDE_PROTECTED_DEF = true
const SHOW_HIDDEN_DEF = true
const HIDE_FILE_EXTENSIONS_DEF = false
const PANE_DRAG_MODE_DEF: PaneDragMode = 'move'

function readFontSize(key = LS_FONT_SIZE, fallback = FONT_SIZE_DEF): number {
  try {
    const saved = Number(localStorage.getItem(key))
    return Number.isInteger(saved) && saved >= FONT_SIZE_MIN && saved <= FONT_SIZE_MAX ? saved : fallback
  } catch {
    return fallback
  }
}

function sanitizeFontStack(value: string): string {
  return value
    .replace(/[;{}<>\\]/g, ' ')
    .replace(/\s+/g, ' ')
    .trim()
    .slice(0, 200)
}

function readUiFont(): string {
  try {
    const saved = localStorage.getItem(LS_UI_FONT) ?? ''
    // 兼容早期按预设 id 保存的值
    const preset = UI_FONT_PRESETS.find((font) => font.id === saved)
    return preset ? preset.stack : sanitizeFontStack(saved)
  } catch {
    return ''
  }
}

function readRowHeight(key: string, min: number, max: number, def: number): number {
  try {
    const saved = Number(localStorage.getItem(key))
    return Number.isInteger(saved) && saved >= min && saved <= max ? saved : def
  } catch {
    return def
  }
}

function readBool(key: string, fallback: boolean): boolean {
  try {
    const v = localStorage.getItem(key)
    return v === null ? fallback : v === '1'
  } catch {
    return fallback
  }
}

function readPaneDragMode(): PaneDragMode {
  try {
    return localStorage.getItem(LS_PANE_DRAG_MODE) === 'copy' ? 'copy' : PANE_DRAG_MODE_DEF
  } catch {
    return PANE_DRAG_MODE_DEF
  }
}

function readTheme(): ThemeMode {
  try {
    const saved = localStorage.getItem(LS_THEME)
    return saved === 'light' ? 'light' : 'dark'
  } catch {
    return 'dark'
  }
}

const DEFAULT_ACCENT_COLORS: Record<ThemeMode, string> = { light: '#f2ebd9', dark: '#66363c' }
function readAccentColor(mode: ThemeMode): string {
  try {
    const legacyColor = readTheme() === mode ? localStorage.getItem(LS_ACCENT_COLOR) : null
    const saved = localStorage.getItem(mode === 'light' ? LS_LIGHT_ACCENT_COLOR : LS_DARK_ACCENT_COLOR)
      ?? legacyColor
    return saved && /^#[\da-f]{6}$/i.test(saved) ? saved.toLowerCase() : DEFAULT_ACCENT_COLORS[mode]
  } catch {
    return DEFAULT_ACCENT_COLORS[mode]
  }
}

function readUiOpacity(): number {
  try {
    const value = Number(localStorage.getItem(LS_UI_OPACITY))
    return Number.isInteger(value) && value >= 60 && value <= 100 ? value : 100
  } catch { return 100 }
}

function readSettingsBackground(): string {
  try {
    const saved = localStorage.getItem(LS_SETTINGS_BACKGROUND) ?? ''
    return /^data:image\/(png|jpeg|webp|gif);base64,[\da-z+/=]+$/i.test(saved) ? saved : ''
  } catch { return '' }
}

let settingsDbPromise: Promise<IDBDatabase> | undefined
function openSettingsDb(): Promise<IDBDatabase> {
  if (!settingsDbPromise) {
    settingsDbPromise = new Promise((resolve, reject) => {
      const request = indexedDB.open(SETTINGS_DB_NAME, 1)
      request.onupgradeneeded = () => request.result.createObjectStore(SETTINGS_DB_STORE)
      request.onsuccess = () => resolve(request.result)
      request.onerror = () => reject(request.error)
    })
  }
  return settingsDbPromise
}

function readBackgroundFromDb(): Promise<string> {
  return openSettingsDb().then((db) => new Promise((resolve, reject) => {
    const request = db.transaction(SETTINGS_DB_STORE, 'readonly').objectStore(SETTINGS_DB_STORE).get(SETTINGS_BACKGROUND_KEY)
    request.onsuccess = () => resolve(typeof request.result === 'string' && /^data:image\/(png|jpeg|webp|gif);base64,[\da-z+/=]+$/i.test(request.result) ? request.result : '')
    request.onerror = () => reject(request.error)
  }))
}

function writeBackgroundToDb(value: string): Promise<void> {
  return openSettingsDb().then((db) => new Promise((resolve, reject) => {
    const transaction = db.transaction(SETTINGS_DB_STORE, 'readwrite')
    const store = transaction.objectStore(SETTINGS_DB_STORE)
    if (value) store.put(value, SETTINGS_BACKGROUND_KEY)
    else store.delete(SETTINGS_BACKGROUND_KEY)
    transaction.oncomplete = () => resolve()
    transaction.onerror = () => reject(transaction.error)
    transaction.onabort = () => reject(transaction.error)
  }))
}

async function loadSettingsBackground(): Promise<string> {
  try {
    const saved = await readBackgroundFromDb()
    if (saved) return saved
  } catch {
    // Fall back to the previous localStorage setting and migrate it if possible.
  }
  const legacy = readSettingsBackground()
  if (legacy) {
    try {
      await writeBackgroundToDb(legacy)
      localStorage.removeItem(LS_SETTINGS_BACKGROUND)
    } catch {
      /* Keep the legacy value if migration cannot be completed. */
    }
  }
  return legacy
}

function applyTheme(mode: ThemeMode) {
  document.documentElement.setAttribute('data-theme', mode)
}

function applyAccentColor(color: string, mode: ThemeMode) {
  const root = document.documentElement.style
  const hex = color.slice(1)
  const r = parseInt(hex.slice(0, 2), 16) / 255
  const g = parseInt(hex.slice(2, 4), 16) / 255
  const b = parseInt(hex.slice(4, 6), 16) / 255
  const luminance = [r, g, b].map((v) => v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4)
    .reduce((sum, v, i) => sum + v * [0.2126, 0.7152, 0.0722][i]!, 0)
  const effectiveColor = mode === 'light' && luminance > 0.32
    ? `color-mix(in srgb, ${color} 35%, #212121)`
    : color
  root.setProperty('--theme-tint', color)
  root.setProperty('--accent', effectiveColor)
  root.setProperty('--accent-fg', mode === 'light' ? '#ffffff' : luminance > 0.42 ? '#171717' : '#ffffff')
  root.setProperty('--accent-soft', `color-mix(in srgb, ${color} 18%, transparent)`)
}

function applyFontSize(size: number) {
  document.documentElement.style.setProperty('--ui-font-size', `${size}px`)
}

function applyUiFont(value: string) {
  const stack = value.trim() || DEFAULT_FONT_STACK
  document.documentElement.style.setProperty('--font', stack)
}

function applySidebarFontSize(size: number) {
  document.documentElement.style.setProperty('--sidebar-font-size', `${size}px`)
}

function applyFileFontSize(size: number) {
  document.documentElement.style.setProperty('--file-font-size', `${size}px`)
}

function applyRowHeights(sidebar: number, file: number) {
  const style = document.documentElement.style
  style.setProperty('--sidebar-row-h', `${sidebar}px`)
  style.setProperty('--file-row-h', `${file}px`)
}

/**
 * UI 全局状态 —— 由 **WS-C (feat/ui-shell)** 实现。
 * ⚠️ App.vue（Foundation 拥有）依赖以下字段名，WS-C 不得改名：
 * `sidebarCollapsed`
 */
export const useUiStore = defineStore('ui', () => {
  const sidebarCollapsed = ref(readBool(LS_SIDEBAR_COLLAPSED, false))
  const theme = ref<ThemeMode>(readTheme())
  const lightAccentColor = ref(readAccentColor('light'))
  const darkAccentColor = ref(readAccentColor('dark'))
  const accentColor = computed(() => theme.value === 'light' ? lightAccentColor.value : darkAccentColor.value)
  const uiOpacity = ref(readUiOpacity())
  const settingsBackgroundImage = ref(readSettingsBackground())
  let backgroundImageRevision = 0
  void loadSettingsBackground().then((saved) => {
    if (backgroundImageRevision === 0 && saved) {
      settingsBackgroundImage.value = saved
      applySurfaceOpacity()
    }
  })
  const fontSize = ref(readFontSize())
  const uiFont = ref(readUiFont())
  const sidebarFontSize = ref(readFontSize(LS_SIDEBAR_FONT_SIZE, SIDEBAR_FONT_SIZE_DEF))
  const fileFontSize = ref(readFontSize(LS_FILE_FONT_SIZE, FILE_FONT_SIZE_DEF))
  const sidebarRowHeight = ref(readRowHeight(LS_SIDEBAR_ROW_H, SIDEBAR_ROW_MIN, SIDEBAR_ROW_MAX, SIDEBAR_ROW_DEF))
  const fileRowHeight = ref(readRowHeight(LS_FILE_ROW_H, FILE_ROW_MIN, FILE_ROW_MAX, FILE_ROW_DEF))
  const resolvedTheme = computed(() => theme.value)
  const hideProtected = ref(readBool(LS_HIDE_PROTECTED, HIDE_PROTECTED_DEF))
  /** 是否显示隐藏（FILE_ATTRIBUTE_HIDDEN）条目；关闭后连同受保护系统文件一起隐藏。 */
  const showHidden = ref(readBool(LS_SHOW_HIDDEN, SHOW_HIDDEN_DEF))
  const hideFileExtensions = ref(readBool(LS_HIDE_FILE_EXTENSIONS, HIDE_FILE_EXTENSIONS_DEF))
  /** 在两个窗格之间拖动文件时的默认操作（默认移动，可改为复制）。 */
  const paneDragMode = ref<PaneDragMode>(readPaneDragMode())
  /** 详细视图列宽（表头拖动后持久化） */
  const detailColumnWidths = ref<DetailColumnWidths>(readDetailColumnWidths())
  /** 回收站列宽（表头拖动后持久化） */
  const trashColumnWidths = ref<TrashColumnWidths>(readTrashColumnWidths())
  // 每次启动都落在「文件导航」面板（不持久化上次停留的面板）
  const activePanel = ref<PanelKey>('files')
  const searchScope = ref<'global' | 'current'>('global')
  const searchCurrentRequest = ref(0)
  /** 一次性标记：下一次搜索面板挂载/响应时应聚焦输入框（消费后清除） */
  const searchFocusPending = ref(false)
  /** 每次 +1 表示「请把焦点交给搜索框」，用于面板已挂载时也能重复聚焦 */
  const searchFocusRequest = ref(0)
  const settingsOpen = ref(false)
  const settingsTab = ref<SettingsTab>('general')
  const feedbackOpen = ref(false)
  /** 文件夹同步对话框是否打开 */
  const syncOpen = ref(false)
  const statusText = ref('就绪')
  const indexTotal = ref(0)

  function applySurfaceOpacity() {
    const root = document.documentElement.style
    root.setProperty('--ui-opacity', `${uiOpacity.value}%`)
    root.setProperty('--ui-image-opacity-offset', theme.value === 'light' && settingsBackgroundImage.value ? '18%' : '0%')
  }

  // 首次进入时应用已持久化的主题
  applyTheme(theme.value)
  applyAccentColor(accentColor.value, theme.value)
  applySurfaceOpacity()
  applyFontSize(fontSize.value)
  applyUiFont(uiFont.value)
  applySidebarFontSize(sidebarFontSize.value)
  applyFileFontSize(fileFontSize.value)
  applyRowHeights(sidebarRowHeight.value, fileRowHeight.value)

  /** 索引根目录列表（由设置面板维护） */
  const indexRoots = ref<string[]>([])
  /** 是否正在扫描 / 建立索引 */
  const scanning = ref(false)
  /** 最近一次索引已扫描条目数 */
  const scanned = ref(0)
  const usingEverything = ref(false)
  const waitingForEverything = ref(false)

  function toggleSidebar() {
    sidebarCollapsed.value = !sidebarCollapsed.value
    persistSidebar()
  }

  function setPanel(panel: PanelKey) {
    if (activePanel.value === panel && !sidebarCollapsed.value) {
      sidebarCollapsed.value = true
    } else {
      activePanel.value = panel
      sidebarCollapsed.value = false
      // 打开搜索面板时聚焦输入框（图标栏、命令面板、右键「搜索此文件夹」统一走这条）
      if (panel === 'search') searchFocusPending.value = true
    }
    persistSidebar()
  }

  function openCurrentDirectorySearch() {
    searchScope.value = 'current'
    searchCurrentRequest.value += 1
    searchFocusPending.value = true
    searchFocusRequest.value += 1
    activePanel.value = 'search'
    sidebarCollapsed.value = false
    persistSidebar()
  }

  /** Ctrl+F：打开搜索面板并聚焦输入框（不改变当前搜索范围） */
  function openSearch() {
    activePanel.value = 'search'
    sidebarCollapsed.value = false
    searchFocusPending.value = true
    searchFocusRequest.value += 1
    persistSidebar()
  }

  function persistSidebar() {
    try {
      localStorage.setItem(LS_SIDEBAR_COLLAPSED, sidebarCollapsed.value ? '1' : '0')
    } catch {
      /* ignore */
    }
  }

  function openSettings(tab: SettingsTab = 'general') {
    settingsTab.value = tab
    settingsOpen.value = true
  }

  function closeSettings() {
    settingsOpen.value = false
  }

  function openFeedback() {
    feedbackOpen.value = true
  }

  function closeFeedback() {
    feedbackOpen.value = false
  }

  function openSync() {
    syncOpen.value = true
  }

  function closeSync() {
    syncOpen.value = false
  }

  function setTheme(mode: ThemeMode) {
    theme.value = mode
    applyTheme(mode)
    applyAccentColor(accentColor.value, mode)
    applySurfaceOpacity()
    try {
      localStorage.setItem(LS_THEME, mode)
    } catch {
      /* ignore */
    }
  }

  function toggleTheme() {
    setTheme(theme.value === 'dark' ? 'light' : 'dark')
  }

  function setAccentColor(color: string) {
    setThemeAccentColor(theme.value, color)
  }

  function setThemeAccentColor(mode: ThemeMode, color: string) {
    if (!/^#[\da-f]{6}$/i.test(color)) return
    const normalized = color.toLowerCase()
    if (mode === 'light') lightAccentColor.value = normalized
    else darkAccentColor.value = normalized
    if (mode === theme.value) applyAccentColor(normalized, mode)
    try {
      localStorage.setItem(mode === 'light' ? LS_LIGHT_ACCENT_COLOR : LS_DARK_ACCENT_COLOR, normalized)
      localStorage.removeItem(LS_ACCENT_COLOR)
    } catch { /* ignore */ }
  }

  function setUiOpacity(value: number) {
    if (!Number.isInteger(value) || value < 60 || value > 100) return
    uiOpacity.value = value
    applySurfaceOpacity()
    try { localStorage.setItem(LS_UI_OPACITY, String(value)) } catch { /* ignore */ }
  }

  async function setSettingsBackgroundImage(value: string): Promise<boolean> {
    if (value && !/^data:image\/(png|jpeg|webp|gif);base64,[\da-z+/=]+$/i.test(value)) return false
    backgroundImageRevision += 1
    settingsBackgroundImage.value = value
    applySurfaceOpacity()
    try {
      await writeBackgroundToDb(value)
      localStorage.removeItem(LS_SETTINGS_BACKGROUND)
      return true
    } catch {
      return false
    }
  }

  function setFontSize(size: number) {
    if (!Number.isInteger(size) || size < FONT_SIZE_MIN || size > FONT_SIZE_MAX) return
    fontSize.value = size
    applyFontSize(size)
    try {
      localStorage.setItem(LS_FONT_SIZE, String(size))
    } catch {
      /* ignore */
    }
  }

  /** 保存用户输入的字体列表（如 VS Code 的 fontFamily），空字符串表示系统默认 */
  function setUiFont(value: string) {
    const stack = sanitizeFontStack(value)
    if (stack && typeof CSS !== 'undefined' && !CSS.supports('font-family', stack)) return false
    uiFont.value = stack
    applyUiFont(stack)
    try {
      if (stack) localStorage.setItem(LS_UI_FONT, stack)
      else localStorage.removeItem(LS_UI_FONT)
    } catch {
      /* ignore */
    }
    return true
  }

  function setSidebarFontSize(size: number) {
    if (!Number.isInteger(size) || size < FONT_SIZE_MIN || size > FONT_SIZE_MAX) return
    sidebarFontSize.value = size
    applySidebarFontSize(size)
    try {
      localStorage.setItem(LS_SIDEBAR_FONT_SIZE, String(size))
    } catch {
      /* ignore */
    }
  }

  function setFileFontSize(size: number) {
    if (!Number.isInteger(size) || size < FONT_SIZE_MIN || size > FONT_SIZE_MAX) return
    fileFontSize.value = size
    applyFileFontSize(size)
    try {
      localStorage.setItem(LS_FILE_FONT_SIZE, String(size))
    } catch {
      /* ignore */
    }
  }

  function setSidebarRowHeight(size: number) {
    if (!Number.isInteger(size) || size < SIDEBAR_ROW_MIN || size > SIDEBAR_ROW_MAX) return
    sidebarRowHeight.value = size
    applyRowHeights(sidebarRowHeight.value, fileRowHeight.value)
    try {
      localStorage.setItem(LS_SIDEBAR_ROW_H, String(size))
    } catch {
      /* ignore */
    }
  }

  function setFileRowHeight(size: number) {
    if (!Number.isInteger(size) || size < FILE_ROW_MIN || size > FILE_ROW_MAX) return
    fileRowHeight.value = size
    applyRowHeights(sidebarRowHeight.value, fileRowHeight.value)
    try {
      localStorage.setItem(LS_FILE_ROW_H, String(size))
    } catch {
      /* ignore */
    }
  }

  function persistDetailColumnWidths() {
    try {
      localStorage.setItem(LS_DETAIL_COLUMNS, JSON.stringify(detailColumnWidths.value))
    } catch {
      /* ignore */
    }
  }

  /**
   * 设置某列宽度；`name` 传 null 表示恢复自适应。
   * 宽度会被夹到 [最小值, 上限]，写入 localStorage 以便下次启动沿用。
   */
  function setDetailColumnWidth(key: DetailColumnKey, width: number | null) {
    const min = key === 'name' ? DETAIL_NAME_MIN : DETAIL_COL_MIN
    const next: DetailColumnWidths = { ...detailColumnWidths.value }
    if (key === 'name') {
      next.name = width === null ? null : clampColumnWidth(width, min)
    } else {
      next[key] = clampColumnWidth(width ?? DETAIL_COL_DEF[key], min)
    }
    if (next[key] === detailColumnWidths.value[key]) return
    detailColumnWidths.value = next
    persistDetailColumnWidths()
  }

  /** 双击表头分隔线：该列恢复默认宽度（名称列回到自适应）。 */
  function resetDetailColumnWidth(key: DetailColumnKey) {
    if (detailColumnWidths.value[key] === DETAIL_COL_DEF[key]) return
    detailColumnWidths.value = { ...detailColumnWidths.value, [key]: DETAIL_COL_DEF[key] }
    persistDetailColumnWidths()
  }

  function persistTrashColumnWidths() {
    try {
      localStorage.setItem(LS_TRASH_COLUMNS, JSON.stringify(trashColumnWidths.value))
    } catch {
      /* ignore */
    }
  }

  /** 设置回收站某列宽度；`name` 传 null 表示恢复自适应。 */
  function setTrashColumnWidth(key: TrashColumnKey, width: number | null) {
    const min = key === 'name' ? DETAIL_NAME_MIN : DETAIL_COL_MIN
    const next: TrashColumnWidths = { ...trashColumnWidths.value }
    if (key === 'name') {
      next.name = width === null ? null : clampColumnWidth(width, min)
    } else {
      next[key] = clampColumnWidth(width ?? TRASH_COL_DEF[key], min)
    }
    if (next[key] === trashColumnWidths.value[key]) return
    trashColumnWidths.value = next
    persistTrashColumnWidths()
  }

  /** 双击回收站表头分隔线：该列恢复默认宽度（名称列回到自适应）。 */
  function resetTrashColumnWidth(key: TrashColumnKey) {
    if (trashColumnWidths.value[key] === TRASH_COL_DEF[key]) return
    trashColumnWidths.value = { ...trashColumnWidths.value, [key]: TRASH_COL_DEF[key] }
    persistTrashColumnWidths()
  }

  function setHideProtected(value: boolean) {
    hideProtected.value = value
    try {
      localStorage.setItem(LS_HIDE_PROTECTED, value ? '1' : '0')
    } catch {
      /* ignore */
    }
  }

  function toggleHideProtected() {
    setHideProtected(!hideProtected.value)
  }

  function setShowHidden(value: boolean) {
    showHidden.value = value
    try {
      localStorage.setItem(LS_SHOW_HIDDEN, value ? '1' : '0')
    } catch {
      /* ignore */
    }
  }

  function toggleShowHidden() {
    setShowHidden(!showHidden.value)
  }

  function setHideFileExtensions(value: boolean) {
    hideFileExtensions.value = value
    try {
      localStorage.setItem(LS_HIDE_FILE_EXTENSIONS, value ? '1' : '0')
    } catch {
      /* ignore */
    }
  }

  function setPaneDragMode(value: PaneDragMode) {
    paneDragMode.value = value
    try {
      localStorage.setItem(LS_PANE_DRAG_MODE, value)
    } catch {
      /* ignore */
    }
  }

  /** 恢复「视图 / 操作 / 快捷键」相关设置的默认值（不含索引根目录 / 开机自启动等系统级设置） */
  function resetViewDefaults() {
    try {
      localStorage.removeItem(LS_FONT_SIZE)
      localStorage.removeItem(LS_UI_FONT)
      localStorage.removeItem(LS_SIDEBAR_FONT_SIZE)
      localStorage.removeItem(LS_FILE_FONT_SIZE)
      localStorage.removeItem(LS_SIDEBAR_ROW_H)
      localStorage.removeItem(LS_FILE_ROW_H)
      localStorage.removeItem(LS_HIDE_PROTECTED)
      localStorage.removeItem(LS_SHOW_HIDDEN)
      localStorage.removeItem(LS_HIDE_FILE_EXTENSIONS)
      localStorage.removeItem(LS_PANE_DRAG_MODE)
      localStorage.removeItem(LS_DETAIL_COLUMNS)
      localStorage.removeItem(LS_TRASH_COLUMNS)
    } catch {
      /* ignore */
    }
    fontSize.value = FONT_SIZE_DEF
    uiFont.value = ''
    sidebarFontSize.value = SIDEBAR_FONT_SIZE_DEF
    fileFontSize.value = FILE_FONT_SIZE_DEF
    sidebarRowHeight.value = SIDEBAR_ROW_DEF
    fileRowHeight.value = FILE_ROW_DEF
    hideProtected.value = HIDE_PROTECTED_DEF
    showHidden.value = SHOW_HIDDEN_DEF
    hideFileExtensions.value = HIDE_FILE_EXTENSIONS_DEF
    paneDragMode.value = PANE_DRAG_MODE_DEF
    detailColumnWidths.value = { ...DETAIL_COL_DEF }
    trashColumnWidths.value = { ...TRASH_COL_DEF }
    applyFontSize(fontSize.value)
    applyUiFont(uiFont.value)
    applySidebarFontSize(sidebarFontSize.value)
    applyFileFontSize(fileFontSize.value)
    applyRowHeights(sidebarRowHeight.value, fileRowHeight.value)
    resetAllShortcuts()
    setAccentColor(DEFAULT_ACCENT_COLOR)
    setUiOpacity(100)
    setSettingsBackgroundImage('')
  }

  function displayFileName(name: string, isDir: boolean): string {
    if (!hideFileExtensions.value || isDir) return name
    const dot = name.lastIndexOf('.')
    return dot > 0 ? name.slice(0, dot) : name
  }

  function setIndexRoots(roots: string[]) {
    indexRoots.value = [...roots]
  }

  function setStatusText(text: string) {
    statusText.value = text
  }

  // ---- 索引状态：全局订阅（供侧边栏搜索面板的状态栏展示）----
  let indexWatchStarted = false

  async function refreshIndexStatus() {
    try {
      const s = await api.indexStatus()
      indexTotal.value = s.total
      scanning.value = s.scanning
      scanned.value = s.scanned
      usingEverything.value = s.usingEverything
      waitingForEverything.value = s.waitingForEverything
      if (!indexRoots.value.length && s.roots.length) indexRoots.value = [...s.roots]
    } catch {
      /* 索引状态暂不可用（例如后端未就绪） */
    }
  }

  /** 订阅索引进度事件，仅在首次调用时建立监听（幂等）。 */
  async function startIndexStatusWatch() {
    if (indexWatchStarted) return
    indexWatchStarted = true
    await refreshIndexStatus()
    await events.onIndexProgress((p) => {
      const prog = p as IndexProgress
      scanned.value = prog.scanned
      if (typeof prog.total === 'number' && prog.total > 0) indexTotal.value = prog.total
      scanning.value = !prog.done
    })
    await events.onIndexDone(() => {
      scanning.value = false
      void refreshIndexStatus()
    })
  }

  return {
    sidebarCollapsed,
    theme,
    accentColor,
    lightAccentColor,
    darkAccentColor,
    setAccentColor,
    setThemeAccentColor,
    uiOpacity,
    setUiOpacity,
    settingsBackgroundImage,
    setSettingsBackgroundImage,
    fontSize,
    uiFont,
    sidebarFontSize,
    fileFontSize,
    sidebarRowHeight,
    fileRowHeight,
    resolvedTheme,
    hideProtected,
    showHidden,
    setShowHidden,
    toggleShowHidden,
    hideFileExtensions,
    paneDragMode,
    setPaneDragMode,
    activePanel,
    searchScope,
    searchCurrentRequest,
    searchFocusPending,
    searchFocusRequest,
    settingsOpen,
    settingsTab,
    feedbackOpen,
    syncOpen,
    statusText,
    indexTotal,
    indexRoots,
    scanning,
    scanned,
    usingEverything,
    waitingForEverything,
    toggleSidebar,
    setPanel,
    openCurrentDirectorySearch,
    openSearch,
    openSettings,
    closeSettings,
    openFeedback,
    closeFeedback,
    openSync,
    closeSync,
    setTheme,
    toggleTheme,
    setFontSize,
    setUiFont,
    setSidebarFontSize,
    setFileFontSize,
    setSidebarRowHeight,
    setFileRowHeight,
    setHideProtected,
    toggleHideProtected,
    setHideFileExtensions,
    detailColumnWidths,
    setDetailColumnWidth,
    resetDetailColumnWidth,
    trashColumnWidths,
    setTrashColumnWidth,
    resetTrashColumnWidth,
    resetViewDefaults,
    displayFileName,
    setIndexRoots,
    setStatusText,
    refreshIndexStatus,
    startIndexStatusWatch,
  }
})

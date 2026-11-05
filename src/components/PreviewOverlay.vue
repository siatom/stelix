<script setup lang="ts">
// 文件预览弹窗（空格预览，类似 Finder Quick Look）。
//
// - 图片走 asset 协议直接加载，支持缩放 / 适应窗口 / 原始尺寸 / 滚轮缩放 / 拖拽平移；
// - 文本走后端 read_text_preview（带 1 MiB 上限、二进制与编码识别）；
// - PDF 交给 WebView2 内置阅读器（iframe + asset 协议）；
// - 其余格式给出占位提示，保留「用默认应用打开」。
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { convertFileSrc } from '@tauri-apps/api/core'
import { openPath } from '@tauri-apps/plugin-opener'
import { api } from '../api'
import { usePreviewStore } from '../stores/preview'
import { useUiStore } from '../stores/ui'
import type { TextPreview } from '../types'
import Icon from './icons/Icon.vue'
import type { IconName } from './icons/paths'
import { formatSize, formatTime } from './Pane/utils'

/** 超过这个大小的图片不解码：避免一次性占满显存 / 内存。 */
const MAX_IMAGE_BYTES = 64 * 1024 * 1024
/** PDF 交给内置阅读器，上限放宽一些。 */
const MAX_PDF_BYTES = 128 * 1024 * 1024
/** 文本读取上限（与后端默认值一致）。 */
const TEXT_LIMIT = 1024 * 1024

const store = usePreviewStore()
const ui = useUiStore()

const item = computed(() => store.current)
const kind = computed(() => store.kind)
const assetUrl = computed(() => (item.value ? convertFileSrc(item.value.path) : ''))

const kindIcon = computed<IconName>(() => {
  switch (kind.value) {
    case 'image':
      return 'image'
    case 'pdf':
    case 'text':
      return 'file-text'
    default:
      return 'file'
  }
})

// ---- 文本 ----
const textPreview = ref<TextPreview | null>(null)
const textLoading = ref(false)
const textError = ref<string | null>(null)
const wrap = ref(false)
/** 刚复制完内容的短暂反馈（按钮变成「已复制」）。 */
const copied = ref(false)
let copiedTimer: number | undefined
let loadToken = 0

// ---- 图片 ----
const canvasRef = ref<HTMLElement | null>(null)
const natural = ref({ w: 0, h: 0 })
const canvasSize = ref({ w: 0, h: 0 })
const zoom = ref<number | 'fit'>('fit')
const imageLoading = ref(false)
const imageError = ref(false)
const panning = ref(false)

const fitScale = computed(() => {
  const n = natural.value
  const c = canvasSize.value
  if (!n.w || !n.h || !c.w || !c.h) return 1
  return Math.min(1, c.w / n.w, c.h / n.h)
})
const displayScale = computed(() => (zoom.value === 'fit' ? fitScale.value : zoom.value))
const percent = computed(() => Math.max(1, Math.round(displayScale.value * 100)))
const imageStyle = computed(() => {
  const n = natural.value
  if (!n.w || !n.h) return undefined
  const scale = displayScale.value
  return {
    width: `${Math.max(1, Math.round(n.w * scale))}px`,
    height: `${Math.max(1, Math.round(n.h * scale))}px`,
  }
})

const lineCount = computed(() => {
  const text = textPreview.value?.text
  if (!text) return 0
  let count = 1
  for (let i = 0; i < text.length; i += 1) if (text.charCodeAt(i) === 10) count += 1
  return count
})

const counter = computed(() =>
  store.canNavigate ? `${store.index + 1} / ${store.items.length}` : '',
)

function resetView() {
  natural.value = { w: 0, h: 0 }
  zoom.value = 'fit'
  imageLoading.value = false
  imageError.value = false
  textPreview.value = null
  textError.value = null
  textLoading.value = false
  wrap.value = false
  copied.value = false
}

function loadText(path: string) {
  const token = ++loadToken
  textLoading.value = true
  void api
    .readTextPreview(path, TEXT_LIMIT)
    .then((result) => {
      if (token === loadToken) textPreview.value = result
    })
    .catch((error: unknown) => {
      if (token === loadToken) textError.value = String(error)
    })
    .finally(() => {
      if (token === loadToken) textLoading.value = false
    })
}

/**
 * 复制当前文本预览的内容。
 *
 * 只复制已加载的部分：超大文件被截断时页脚会提示，复制结果同样是前 1 MiB。
 * 选区内的 Ctrl+C 由浏览器原生处理（`.pv-text` 允许选中），两者互不影响。
 */
async function copyText() {
  const text = textPreview.value?.text
  if (!text) return
  try {
    await navigator.clipboard.writeText(text)
    copied.value = true
    if (copiedTimer !== undefined) window.clearTimeout(copiedTimer)
    copiedTimer = window.setTimeout(() => {
      copied.value = false
      copiedTimer = undefined
    }, 1500)
  } catch (error) {
    ui.setStatusText(`复制失败：${String(error)}`)
  }
}

watch(
  () => item.value?.path,
  () => {
    resetView()
    const target = item.value
    if (!target) return
    if (target.kind === 'text') loadText(target.path)
    if (target.kind === 'image') imageLoading.value = true
  },
  { immediate: true },
)

// 打开时（从关闭 → 打开）重新拍一次画布尺寸：关闭期间元素不在 DOM 里。
watch(
  () => store.open,
  (open) => {
    if (!open) {
      loadToken += 1
      return
    }
    void nextTick(() => {
      measureCanvas()
      if (kind.value === 'image') imageLoading.value = true
    })
  },
)

// ---- 图片尺寸与缩放 ----

let observer: ResizeObserver | null = null

function measureCanvas() {
  const el = canvasRef.value
  if (!el) return
  canvasSize.value = { w: el.clientWidth, h: el.clientHeight }
}

watch(canvasRef, (el) => {
  observer?.disconnect()
  observer = null
  if (!el) return
  measureCanvas()
  observer = new ResizeObserver(() => measureCanvas())
  observer.observe(el)
})

function onImageLoad(event: Event) {
  const image = event.target as HTMLImageElement
  natural.value = {
    w: image.naturalWidth || image.width || 0,
    h: image.naturalHeight || image.height || 0,
  }
  imageLoading.value = false
  void nextTick(measureCanvas)
}

function onImageError() {
  imageLoading.value = false
  imageError.value = true
}

function zoomIn() {
  zoom.value = Math.min(16, displayScale.value * 1.25)
}

function zoomOut() {
  zoom.value = Math.max(0.05, displayScale.value / 1.25)
}

function fitToWindow() {
  zoom.value = 'fit'
}

function actualSize() {
  zoom.value = 1
}

function onWheel(event: WheelEvent) {
  if (kind.value !== 'image') return
  event.preventDefault()
  if (event.deltaY < 0) zoomIn()
  else zoomOut()
}

let panOrigin: { x: number; y: number; left: number; top: number } | null = null

function onCanvasDown(event: MouseEvent) {
  const el = canvasRef.value
  if (!el || event.button !== 0) return
  if (el.scrollWidth <= el.clientWidth && el.scrollHeight <= el.clientHeight) return
  panOrigin = { x: event.clientX, y: event.clientY, left: el.scrollLeft, top: el.scrollTop }
  panning.value = true
  window.addEventListener('mousemove', onPanMove)
  window.addEventListener('mouseup', onPanEnd)
}

function onPanMove(event: MouseEvent) {
  const el = canvasRef.value
  if (!el || !panOrigin) return
  el.scrollLeft = panOrigin.left - (event.clientX - panOrigin.x)
  el.scrollTop = panOrigin.top - (event.clientY - panOrigin.y)
}

function onPanEnd() {
  panOrigin = null
  panning.value = false
  window.removeEventListener('mousemove', onPanMove)
  window.removeEventListener('mouseup', onPanEnd)
}

async function openExternal() {
  const target = item.value
  if (!target) return
  try {
    await openPath(target.path)
  } catch (error) {
    console.error('无法用默认应用打开', error)
  }
}

// ---- 键盘 ----

function onKeydown(event: KeyboardEvent) {
  if (!store.open) return
  if (event.isComposing) return
  // 事件已被更早的处理器消费（例如列表刚用空格打开预览）：不要把它当成「关闭」。
  if (event.defaultPrevented) return
  const key = event.key
  if (key === 'Escape' || key === ' ') {
    event.preventDefault()
    event.stopPropagation()
    store.close()
    return
  }
  if (key === 'ArrowLeft') {
    event.preventDefault()
    event.stopPropagation()
    store.prev()
    return
  }
  if (key === 'ArrowRight') {
    event.preventDefault()
    event.stopPropagation()
    store.next()
    return
  }
  if (kind.value !== 'image') return
  if (key === '+' || key === '=') {
    event.preventDefault()
    zoomIn()
  } else if (key === '-') {
    event.preventDefault()
    zoomOut()
  } else if (key === '0') {
    event.preventDefault()
    fitToWindow()
  } else if (key === '1') {
    event.preventDefault()
    actualSize()
  }
}

onMounted(() => window.addEventListener('keydown', onKeydown, true))
onBeforeUnmount(() => {
  window.removeEventListener('keydown', onKeydown, true)
  observer?.disconnect()
  observer = null
  onPanEnd()
  if (copiedTimer !== undefined) window.clearTimeout(copiedTimer)
})
</script>

<template>
  <div v-if="store.open && item" class="mask" @mousedown.self="store.close()">
    <section class="preview-card" role="dialog" aria-modal="true" :aria-label="`预览 ${item.name}`">
      <header class="pv-head">
        <div class="pv-title">
          <Icon :name="kindIcon" :size="15" class="pv-kind-icon" />
          <span class="pv-name" :title="item.path">{{ item.name }}</span>
        </div>

        <div class="pv-meta">
          <span>{{ formatSize(item.size) }}</span>
          <span v-if="item.modified">{{ formatTime(item.modified) }}</span>
          <span v-if="counter" class="pv-counter">{{ counter }}</span>
        </div>

        <div class="pv-actions">
          <template v-if="kind === 'image' && !imageError">
            <button type="button" title="缩小（-）" @click="zoomOut"><Icon name="minus" :size="15" /></button>
            <span class="pv-percent">{{ percent }}%</span>
            <button type="button" title="放大（+）" @click="zoomIn"><Icon name="plus" :size="15" /></button>
            <button type="button" class="pv-text-btn" title="适应窗口（0）" @click="fitToWindow">适应</button>
            <button type="button" class="pv-text-btn" title="原始尺寸（1）" @click="actualSize">1:1</button>
          </template>
          <span v-else-if="kind === 'text' && textPreview" class="pv-text-meta">
            {{ lineCount }} 行 · {{ textPreview.encoding.toUpperCase() }}
          </span>
          <button type="button" class="pv-text-btn" title="用默认应用打开" @click="openExternal">
            <Icon name="external-link" :size="13" /><span>打开</span>
          </button>
          <button type="button" class="pv-close" title="关闭（Esc）" aria-label="关闭预览" @click="store.close()">
            <Icon name="x" :size="15" />
          </button>
        </div>
      </header>

      <div class="pv-body">
        <button
          v-if="store.canNavigate"
          type="button"
          class="pv-nav prev"
          title="上一个（←）"
          aria-label="上一个"
          @click.stop="store.prev()"
        >
          <Icon name="chevron-left" :size="22" />
        </button>

        <div class="pv-stage">
          <!-- 远程（FTP / SFTP）：路径不在本地，无法读取 -->
          <div v-if="store.isRemote" class="pv-empty">
            <Icon name="network" :size="30" />
            <p class="pv-empty-title">远程文件暂不支持预览</p>
            <p class="pv-empty-desc">请下载到本地后再预览，或用外部程序打开</p>
          </div>

          <!-- 图片 -->
          <template v-else-if="kind === 'image'">
            <div v-if="item.size > MAX_IMAGE_BYTES" class="pv-empty">
              <Icon name="image" :size="30" />
              <p class="pv-empty-title">图片过大，暂不预览</p>
              <p class="pv-empty-desc">文件大小 {{ formatSize(item.size) }}，超过 {{ formatSize(MAX_IMAGE_BYTES) }} 上限</p>
              <button type="button" class="pv-primary" @click="openExternal">用默认应用打开</button>
            </div>
            <div v-else-if="imageError" class="pv-empty">
              <Icon name="alert-triangle" :size="28" />
              <p class="pv-empty-title">无法加载这张图片</p>
              <p class="pv-empty-desc">格式可能不受支持，或文件已被移动</p>
              <button type="button" class="pv-primary" @click="openExternal">用默认应用打开</button>
            </div>
            <div
              v-else
              ref="canvasRef"
              class="pv-canvas"
              :class="{ panning }"
              @wheel="onWheel"
              @mousedown="onCanvasDown"
            >
              <img
                :src="assetUrl"
                :style="imageStyle"
                alt=""
                draggable="false"
                @load="onImageLoad"
                @error="onImageError"
              />
              <span v-if="imageLoading" class="pv-loading">正在加载…</span>
            </div>
          </template>

          <!-- 文本 -->
          <template v-else-if="kind === 'text'">
            <div v-if="textLoading" class="pv-empty">
              <span class="pv-loading">正在读取…</span>
            </div>
            <div v-else-if="textError" class="pv-empty">
              <Icon name="alert-triangle" :size="28" />
              <p class="pv-empty-title">无法读取文件内容</p>
              <p class="pv-empty-desc">{{ textError }}</p>
            </div>
            <div v-else-if="textPreview?.binary" class="pv-empty">
              <Icon name="file" :size="30" />
              <p class="pv-empty-title">这不是文本文件</p>
              <p class="pv-empty-desc">内容包含二进制数据，无法以文本方式预览</p>
              <button type="button" class="pv-primary" @click="openExternal">用默认应用打开</button>
            </div>
            <div v-else class="pv-text-wrap">
              <pre class="pv-text" :class="{ wrap }" data-native-context-menu>{{ textPreview?.text ?? '' }}</pre>
              <div v-if="textPreview?.truncated" class="pv-truncated">
                文件较大，仅显示前 {{ formatSize(TEXT_LIMIT) }}（共 {{ formatSize(textPreview.size) }}），复制内容也只含已加载部分
              </div>
            </div>
          </template>

          <!-- PDF -->
          <template v-else-if="kind === 'pdf'">
            <div v-if="item.size > MAX_PDF_BYTES" class="pv-empty">
              <Icon name="file-text" :size="30" />
              <p class="pv-empty-title">PDF 过大，暂不预览</p>
              <p class="pv-empty-desc">文件大小 {{ formatSize(item.size) }}</p>
              <button type="button" class="pv-primary" @click="openExternal">用默认应用打开</button>
            </div>
            <iframe v-else class="pv-pdf" :src="assetUrl" :title="item.name" />
          </template>

          <!-- 其他格式 -->
          <div v-else class="pv-empty">
            <Icon name="file" :size="30" />
            <p class="pv-empty-title">该格式暂不支持预览</p>
            <p class="pv-empty-desc">扩展名：{{ item.ext || '无' }}</p>
            <button type="button" class="pv-primary" @click="openExternal">用默认应用打开</button>
          </div>
        </div>

        <button
          v-if="store.canNavigate"
          type="button"
          class="pv-nav next"
          title="下一个（→）"
          aria-label="下一个"
          @click.stop="store.next()"
        >
          <Icon name="chevron-right" :size="22" />
        </button>
      </div>

      <footer class="pv-foot">
        <template v-if="kind === 'text' && textPreview && !textPreview.binary">
          <button type="button" class="pv-foot-btn" @click="wrap = !wrap">
            <Icon name="view-list" :size="13" />{{ wrap ? '取消换行' : '自动换行' }}
          </button>
          <button type="button" class="pv-foot-btn" :title="copied ? '已复制' : '复制全部内容（Ctrl+C 可复制选中部分）'" @click="copyText">
            <Icon :name="copied ? 'check' : 'copy'" :size="13" />{{ copied ? '已复制' : '复制内容' }}
          </button>
        </template>
        <span class="pv-path" :title="item.path">{{ item.path }}</span>
        <span class="pv-hints">
          <template v-if="store.canNavigate">← → 切换 · </template>
          Esc / 空格 关闭
        </span>
      </footer>
    </section>
  </div>
</template>

<style scoped>
.mask {
  position: fixed;
  inset: 0;
  z-index: 340;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 32px;
  background: color-mix(in srgb, var(--mask) 82%, transparent);
  backdrop-filter: blur(2px);
}

.preview-card {
  display: flex;
  flex-direction: column;
  width: min(1180px, 100%);
  height: min(820px, 100%);
  min-height: 0;
  border: 1px solid var(--border-strong);
  border-radius: var(--radius-lg);
  background: var(--bg-elevated);
  box-shadow: var(--shadow-lg);
  overflow: hidden;
}

.pv-head {
  display: flex;
  align-items: center;
  gap: 12px;
  flex: none;
  height: 42px;
  padding: 0 10px 0 14px;
  border-bottom: 1px solid var(--border);
  background: var(--bg-panel);
}

.pv-title {
  display: flex;
  align-items: center;
  gap: 7px;
  min-width: 0;
  flex: 0 1 auto;
}

.pv-kind-icon {
  flex: none;
  color: var(--fg-dim);
}

.pv-name {
  min-width: 0;
  overflow: hidden;
  font-size: 0.9167rem;
  font-weight: 600;
  color: var(--fg);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.pv-meta {
  display: flex;
  align-items: center;
  gap: 10px;
  flex: none;
  color: var(--fg-dim);
  font-size: 0.8333rem;
  font-variant-numeric: tabular-nums;
}

.pv-counter {
  padding: 1px 7px;
  border-radius: 999px;
  background: var(--bg-hover);
  color: var(--fg-muted);
}

.pv-actions {
  display: flex;
  align-items: center;
  gap: 2px;
  margin-left: auto;
  flex: none;
}

.pv-actions button {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  height: 26px;
  padding: 0 7px;
  border-radius: var(--radius);
  color: var(--fg-muted);
  font-size: 0.8333rem;
}

.pv-actions button:hover {
  background: var(--bg-hover);
  color: var(--fg);
}

.pv-percent {
  min-width: 44px;
  color: var(--fg-muted);
  font-size: 0.7917rem;
  font-variant-numeric: tabular-nums;
  text-align: center;
}

.pv-text-meta {
  margin-right: 4px;
  color: var(--fg-dim);
  font-size: 0.7917rem;
  font-variant-numeric: tabular-nums;
}

.pv-close {
  margin-left: 4px;
}

.pv-body {
  position: relative;
  display: flex;
  flex: 1;
  min-height: 0;
  background: var(--bg-app);
}

.pv-stage {
  position: relative;
  flex: 1;
  min-width: 0;
  min-height: 0;
  display: flex;
}

.pv-nav {
  position: absolute;
  top: 50%;
  z-index: 3;
  display: flex;
  align-items: center;
  justify-content: center;
  width: 34px;
  height: 54px;
  border-radius: var(--radius);
  background: color-mix(in srgb, var(--bg-elevated) 78%, transparent);
  border: 1px solid var(--border);
  color: var(--fg-muted);
  transform: translateY(-50%);
  transition: background var(--t-fast), color var(--t-fast), opacity var(--t-fast);
}

.pv-nav:hover {
  background: var(--bg-hover);
  color: var(--fg);
}

.pv-nav.prev {
  left: 10px;
}

.pv-nav.next {
  right: 10px;
}

/* ---- 图片 ---- */
.pv-canvas {
  flex: 1;
  min-width: 0;
  min-height: 0;
  display: grid;
  overflow: auto;
  padding: 12px;
}

.pv-canvas.panning {
  cursor: grabbing;
}

.pv-canvas img {
  display: block;
  margin: auto;
  max-width: none;
  user-select: none;
  box-shadow: var(--shadow-md);
  background:
    conic-gradient(from 90deg, color-mix(in srgb, var(--fg) 6%, transparent) 25%, transparent 0 50%,
      color-mix(in srgb, var(--fg) 6%, transparent) 0 75%, transparent 0) 0 0 / 16px 16px;
}

.pv-loading {
  position: absolute;
  inset: 0;
  display: grid;
  place-items: center;
  color: var(--fg-dim);
  font-size: 0.9167rem;
  pointer-events: none;
}

/* ---- 文本 ---- */
.pv-text-wrap {
  display: flex;
  flex-direction: column;
  flex: 1;
  min-width: 0;
  min-height: 0;
}

.pv-text {
  flex: 1;
  min-height: 0;
  margin: 0;
  padding: 14px 16px;
  overflow: auto;
  color: var(--fg);
  font-family: var(--font-mono, 'Cascadia Mono', 'Consolas', monospace);
  font-size: 0.8333rem;
  line-height: 1.55;
  tab-size: 4;
  white-space: pre;
  /* 全局 body 禁用了选中；预览文本要能拖选后用 Ctrl+C 复制。 */
  user-select: text;
  cursor: text;
}

.pv-text.wrap {
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}

.pv-truncated {
  flex: none;
  padding: 7px 16px;
  border-top: 1px solid var(--border);
  background: var(--bg-panel);
  color: var(--warn);
  font-size: 0.7917rem;
}

/* ---- PDF ---- */
.pv-pdf {
  flex: 1;
  width: 100%;
  height: 100%;
  border: 0;
  background: var(--bg-app);
}

/* ---- 占位 ---- */
.pv-empty {
  display: flex;
  flex: 1;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  padding: 24px;
  color: var(--fg-dim);
  text-align: center;
}

.pv-empty-title {
  margin: 4px 0 0;
  color: var(--fg);
  font-size: 0.9583rem;
  font-weight: 600;
}

.pv-empty-desc {
  max-width: 460px;
  margin: 0;
  color: var(--fg-dim);
  font-size: 0.8333rem;
  word-break: break-all;
}

.pv-primary {
  margin-top: 8px;
  height: 30px;
  padding: 0 14px;
  border-radius: var(--radius);
  background: var(--accent);
  color: #fff;
  font-size: 0.875rem;
}

.pv-primary:hover {
  background: var(--accent-hover, var(--accent));
  filter: brightness(1.08);
}

/* ---- 底部 ---- */
.pv-foot {
  display: flex;
  align-items: center;
  gap: 10px;
  flex: none;
  height: 30px;
  padding: 0 12px;
  border-top: 1px solid var(--border);
  background: var(--bg-panel);
  color: var(--fg-dim);
  font-size: 0.7917rem;
}

.pv-foot-btn {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  height: 22px;
  padding: 0 6px;
  border-radius: var(--radius-sm, var(--radius));
  color: var(--fg-muted);
  font-size: 0.7917rem;
}

.pv-foot-btn:hover {
  background: var(--bg-hover);
  color: var(--fg);
}

.pv-path {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  direction: rtl;
  text-align: left;
}

.pv-hints {
  flex: none;
  color: var(--fg-dim);
}
</style>

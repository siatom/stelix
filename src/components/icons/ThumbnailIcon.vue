<script setup lang="ts">
/**
 * 图标视图中的图片单元格。
 *
 * 图片进入视口附近后才向后端请求缩略图：加载完成前显示系统文件图标，
 * 失败（损坏 / 无权限 / 远程路径 / 超大图）时继续显示系统图标，不出现破图。
 */
import { onBeforeUnmount, onMounted, ref, watch } from 'vue'
import FileIcon from './FileIcon.vue'
import { isThumbnailCandidate, requestThumbnail } from './thumbnailCache'

const props = withDefaults(
  defineProps<{
    path: string
    isDir: boolean
    ext: string | null
    /** 修改时间（Unix 毫秒），参与缓存失效。 */
    modified?: number
    /** 文件大小（字节），参与缓存失效。 */
    fileSize?: number
    /** 缩略图显示边长（px）。 */
    size?: number
    /** 系统图标占位边长（px）。 */
    fallbackSize?: number
    /** 缩略图圆角（px）。 */
    radius?: number
  }>(),
  { size: 48, fallbackSize: 38, radius: 8, modified: 0, fileSize: 0 },
)

const host = ref<HTMLElement | null>(null)
const url = ref<string | null>(null)
const loaded = ref(false)

let observer: IntersectionObserver | null = null
/** 请求代次：路径变化或组件卸载后丢弃过期结果。 */
let generation = 0

function supported(): boolean {
  return isThumbnailCandidate(props.path, props.ext, props.isDir)
}

/** 请求边长：按 DPR 放大，避免高分屏发虚；后端会夹紧到 32~512。 */
function requestEdge(): number {
  const dpr = typeof window === 'undefined' ? 1 : window.devicePixelRatio || 1
  return Math.min(512, Math.max(64, Math.round(props.size * Math.max(2, dpr))))
}

async function load() {
  if (!supported()) return
  const current = ++generation
  const result = await requestThumbnail(
    props.path,
    props.modified,
    props.fileSize,
    requestEdge(),
  )
  // 组件已切换路径 / 卸载，或期间又发起了新请求：丢弃这次结果。
  if (current !== generation) return
  url.value = result
}

function stopObserving() {
  observer?.disconnect()
  observer = null
}

function watchVisibility() {
  stopObserving()
  generation += 1
  url.value = null
  loaded.value = false
  if (!supported()) return

  // 旧环境没有 IntersectionObserver 时直接加载，功能优先。
  if (typeof IntersectionObserver === 'undefined') {
    void load()
    return
  }
  observer = new IntersectionObserver(
    (entries) => {
      if (!entries.some((entry) => entry.isIntersecting)) return
      stopObserving()
      void load()
    },
    // 提前一屏开始生成，滚动时缩略图已就绪。
    { rootMargin: '200px' },
  )
  if (host.value) observer.observe(host.value)
}

onMounted(watchVisibility)
onBeforeUnmount(() => {
  stopObserving()
  generation += 1
})

// 路径 / 修改时间变化时重新判断并请求；文件内容被覆盖后旧缩略图自动失效。
watch(
  () => [props.path, props.modified, props.fileSize, props.ext] as const,
  watchVisibility,
)
</script>

<template>
  <span ref="host" class="thumb" :style="{ width: `${size}px`, height: `${size}px` }">
    <FileIcon
      v-if="!loaded"
      :path="path"
      :is-dir="isDir"
      :ext="ext"
      :size="fallbackSize"
    />
    <img
      v-if="url"
      class="thumb-img"
      :class="{ ready: loaded }"
      :src="url"
      :style="{ borderRadius: `${radius}px` }"
      alt=""
      draggable="false"
      @load="loaded = true"
      @error="
        url = null;
        loaded = false
      "
    />
  </span>
</template>

<style scoped>
.thumb {
  position: relative;
  display: flex;
  flex: none;
  align-items: center;
  justify-content: center;
}
.thumb-img {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
  object-fit: cover;
  background: var(--bg-hover);
  box-shadow: 0 0 0 1px var(--border);
  opacity: 0;
  transition: opacity 140ms ease;
  /* 不干扰整格的点击 / 拖拽 */
  pointer-events: none;
  user-select: none;
}
.thumb-img.ready {
  opacity: 1;
}
</style>

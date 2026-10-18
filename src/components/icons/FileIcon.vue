<script setup lang="ts">
/**
 * 文件 / 文件夹图标。
 *
 * 优先显示 Windows 系统图标（由后端 `get_file_icon` 提供）；
 * 加载中或提取失败时回退到内置矢量图标（`Icon`），避免出现空白。
 */
import { computed } from 'vue'
import Icon from './Icon.vue'
import { fileIconRef } from './fileIconCache'
import { fileIcon } from '../Pane/utils'
import type { IconName } from './paths'

const props = withDefaults(
  defineProps<{
    path: string
    isDir: boolean
    ext: string | null
    isDrive?: boolean
    usePathIcon?: boolean
    fallback?: IconName
    size?: number
  }>(),
  { size: 20 },
)

// computed：虚拟滚动的行会被复用（key 为索引），path 变化时需要重新取图标。
const url = computed(() => fileIconRef(props.path, props.isDir, props.isDrive, props.usePathIcon).value)
</script>

<template>
  <img
    v-if="url"
    class="file-icon"
    :src="url"
    :style="{ width: `${size}px`, height: `${size}px` }"
    alt=""
    draggable="false"
  />
  <Icon v-else :name="fallback ?? fileIcon({ isDir, ext })" :size="size" />
</template>

<style scoped>
.file-icon {
  display: inline-block;
  flex: none;
  object-fit: contain;
  vertical-align: -0.15em;
  /* 不干扰整行的点击 / 拖拽 */
  pointer-events: none;
  user-select: none;
}
</style>

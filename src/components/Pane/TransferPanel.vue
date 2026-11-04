<script setup lang="ts">
// 文件操作进度面板：浮在主区右下角，展示当前任务进度与排队情况。
// 数据全部来自 pane store 的操作队列（同一时刻只有一个任务在跑）。
import { computed } from 'vue'
import { usePaneStore, type FsQueueItem } from '../../stores/pane'
import { basename, formatSize } from './utils'

const pane = usePaneStore()

/**
 * 面板要展示的条目：优先正在执行的任务；没有在跑的则展示最后一个已结束的任务
 * （队列会把它保留约 2 秒，用于提示「已完成 / 已取消 / 失败」）。
 */
const current = computed<FsQueueItem | null>(() => {
  const running = pane.fsQueue.find((item) => item.state === 'running')
  if (running) return running
  const finished = [...pane.fsQueue].reverse().find((item) => item.state !== 'waiting')
  return finished ?? null
})

const waiting = computed(() => pane.pendingFsCount)

const isRunning = computed(() => current.value?.state === 'running')

const percent = computed<number | null>(() => {
  const progress = current.value?.progress
  if (!progress) return null
  if (progress.bytesTotal > 0) {
    return Math.max(0, Math.min(100, Math.round((progress.bytesDone / progress.bytesTotal) * 100)))
  }
  if (progress.filesTotal > 0) {
    return Math.max(0, Math.min(100, Math.round((progress.filesDone / progress.filesTotal) * 100)))
  }
  return null
})

const stateText = computed(() => {
  switch (current.value?.state) {
    case 'running':
      return '进行中'
    case 'done':
      return '已完成'
    case 'failed':
      return '失败'
    case 'cancelled':
      return '已取消'
    default:
      return ''
  }
})

/** 右侧的计数信息：有字节总量时显示大小，否则显示文件数。 */
const detail = computed(() => {
  const progress = current.value?.progress
  if (!progress) return ''
  if (progress.bytesTotal > 0) return `${formatSize(progress.bytesDone)} / ${formatSize(progress.bytesTotal)}`
  if (progress.filesTotal > 0) return `${progress.filesDone} / ${progress.filesTotal} 个文件`
  return ''
})

const currentName = computed(() => {
  const raw = current.value?.progress?.current
  if (!raw) return ''
  return basename(raw)
})

function cancel() {
  void pane.cancelActiveFsOp()
}
</script>

<template>
  <section v-if="current" class="transfer-panel" aria-live="polite">
    <header class="tp-head">
      <span class="tp-label" :title="current.label">{{ current.label }}</span>
      <span class="tp-state" :class="`is-${current.state}`">{{ stateText }}</span>
      <button v-if="isRunning" class="tp-cancel" type="button" title="取消当前操作" @click="cancel">
        取消
      </button>
    </header>

    <div
      class="tp-bar"
      :class="{ 'is-indeterminate': isRunning && percent === null }"
      role="progressbar"
      aria-valuemin="0"
      aria-valuemax="100"
      :aria-valuenow="percent ?? undefined"
    >
      <div
        class="tp-fill"
        :class="`is-${current.state}`"
        :style="percent === null ? undefined : { width: `${percent}%` }"
      />
    </div>

    <div class="tp-meta">
      <span class="tp-percent">{{ percent === null ? (isRunning ? '正在统计…' : '') : `${percent}%` }}</span>
      <span v-if="detail" class="tp-detail">{{ detail }}</span>
    </div>

    <p v-if="currentName" class="tp-current" :title="current.progress?.current">{{ currentName }}</p>
    <p v-if="waiting > 0" class="tp-waiting">还有 {{ waiting }} 项等待</p>
  </section>
</template>

<style scoped>
.transfer-panel {
  position: fixed;
  z-index: 900;
  right: 16px;
  /* 浮在状态栏之上，不遮挡状态栏的点击区域 */
  bottom: calc(var(--status-bar-h) + 14px);
  width: 322px;
  padding: 10px 12px 11px;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--bg-elevated);
  box-shadow: var(--shadow-md);
  color: var(--fg);
  font-family: var(--font);
  font-size: 12px;
  user-select: none;
  animation: tp-enter var(--t) both;
}

@keyframes tp-enter {
  from {
    opacity: 0;
    transform: translateY(6px);
  }
  to {
    opacity: 1;
    transform: translateY(0);
  }
}

.tp-head {
  display: flex;
  align-items: center;
  gap: 8px;
}

.tp-label {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-weight: 600;
  color: var(--fg);
}

.tp-state {
  flex: none;
  padding: 1px 7px;
  border-radius: 999px;
  background: var(--accent-soft);
  color: var(--fg-muted);
  font-size: 11px;
  line-height: 16px;
}

.tp-state.is-done {
  background: var(--ok-soft);
  color: var(--ok);
}

.tp-state.is-failed {
  background: var(--danger-soft);
  color: var(--danger);
}

.tp-state.is-cancelled {
  color: var(--fg-dim);
}

.tp-cancel {
  flex: none;
  height: 22px;
  padding: 0 9px;
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--fg-muted);
  font-size: 11px;
  transition:
    background var(--t-fast),
    color var(--t-fast),
    border-color var(--t-fast);
}

.tp-cancel:hover {
  border-color: var(--border-strong);
  background: var(--bg-hover);
  color: var(--fg);
}

.tp-bar {
  position: relative;
  overflow: hidden;
  height: 6px;
  margin-top: 9px;
  border-radius: 999px;
  background: var(--bg-active);
}

.tp-fill {
  height: 100%;
  width: 0;
  border-radius: inherit;
  background: var(--accent);
  transition: width var(--t) linear;
}

.tp-fill.is-done {
  background: var(--ok);
}

.tp-fill.is-failed {
  background: var(--danger);
}

.tp-fill.is-cancelled {
  background: var(--fg-dim);
}

/* 总量未知（统计中）时用一段往返扫光表示不确定进度 */
.tp-bar.is-indeterminate .tp-fill {
  width: 38%;
  animation: tp-sweep 1.1s ease-in-out infinite;
}

@keyframes tp-sweep {
  0% {
    transform: translateX(-110%);
  }
  100% {
    transform: translateX(300%);
  }
}

.tp-meta {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 8px;
  margin-top: 7px;
  color: var(--fg-muted);
  font-size: 11px;
  font-variant-numeric: tabular-nums;
}

.tp-detail {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--fg-dim);
}

.tp-current {
  margin: 5px 0 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--fg-muted);
  font-size: 11px;
}

.tp-waiting {
  margin: 6px 0 0;
  padding-top: 6px;
  border-top: 1px solid var(--border);
  color: var(--fg-dim);
  font-size: 11px;
}
</style>

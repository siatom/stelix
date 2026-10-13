import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { api } from '../api'
import type { SearchHit, SearchQuery } from '../types'
import { categoryOf } from '../searchFilters'

/** 全局检索状态 —— 由 **WS-D (feat/ui-panes)** 实现。 */
export const useSearchStore = defineStore('search', () => {
  const query = ref('')
  const hits = ref<SearchHit[]>([])
  const searching = ref(false)
  const elapsedMs = ref(0)
  const error = ref<string | null>(null)
  /** 类型筛选（searchFilters 分类 key；null = 全部）。跨范围保持。 */
  const typeKey = ref<string | null>(null)
  const typeLabel = computed(() => categoryOf(typeKey.value)?.label ?? null)
  /** 是否已执行过一次查询（用于只在有结果时才展示命中数） */
  const searched = ref(false)

  /** 类型筛选 → 查询字段（文件夹 → isDir；其余 → 扩展名白名单） */
  function typeFields(): Pick<SearchQuery, 'isDir' | 'ext'> {
    const cat = categoryOf(typeKey.value)
    if (!cat) return {}
    return cat.isDir ? { isDir: true } : { ext: cat.exts }
  }

  function setType(key: string | null) {
    typeKey.value = key === 'all' ? null : key
  }

  async function run(text?: string) {
    if (text !== undefined) query.value = text
    const trimmed = query.value.trim()
    // 无关键词且无类型筛选时清空；只有类型筛选（空关键词）也照常查询
    if (!trimmed && !categoryOf(typeKey.value)) {
      hits.value = []
      elapsedMs.value = 0
      error.value = null
      searched.value = false
      return
    }
    const q: SearchQuery = { text: trimmed, limit: 500, ...typeFields() }
    searched.value = true
    searching.value = true
    error.value = null
    const t0 = performance.now()
    try {
      hits.value = await api.searchFiles(q)
      elapsedMs.value = performance.now() - t0
    } catch (e) {
      error.value = String(e)
      hits.value = []
    } finally {
      searching.value = false
    }
  }

  function clear() {
    query.value = ''
    hits.value = []
    error.value = null
    searched.value = false
  }

  return { query, hits, searching, elapsedMs, error, typeKey, typeLabel, searched, setType, run, clear }
})

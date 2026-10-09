import { defineStore } from 'pinia'
import { ref } from 'vue'
import { api } from '../api'
import type { SearchHit, SearchQuery } from '../types'

/** 全局检索状态 —— 由 **WS-D (feat/ui-panes)** 实现。 */
export const useSearchStore = defineStore('search', () => {
  const query = ref('')
  const hits = ref<SearchHit[]>([])
  const searching = ref(false)
  const elapsedMs = ref(0)
  const error = ref<string | null>(null)

  async function run(text?: string) {
    if (text !== undefined) query.value = text
    const trimmed = query.value.trim()
    if (!trimmed) {
      hits.value = []
      elapsedMs.value = 0
      error.value = null
      return
    }
    const q: SearchQuery = { text: trimmed, limit: 500 }
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
  }

  return { query, hits, searching, elapsedMs, error, run, clear }
})

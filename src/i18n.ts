import { computed, ref, watch } from 'vue'
import { createI18n } from 'vue-i18n'
import enUS from './locales/en-US'
import zhCN from './locales/zh-CN'

export type Locale = 'zh-CN' | 'en-US'
export type LocalePreference = 'system' | Locale

const STORAGE_KEY = 'stelix.locale'

function systemLocale(): Locale {
  const language = typeof navigator === 'undefined' ? '' : navigator.language
  return /^zh(?:-|$)/i.test(language) ? 'zh-CN' : 'en-US'
}

function readPreference(): LocalePreference {
  try {
    const saved = localStorage.getItem(STORAGE_KEY)
    if (saved === 'zh-CN' || saved === 'en-US') return saved
  } catch { /* Storage may be unavailable in restricted webviews. */ }
  return 'system'
}

export const localePreference = ref<LocalePreference>(readPreference())
const detectedSystemLocale = ref<Locale>(systemLocale())
export const locale = computed<Locale>(() => localePreference.value === 'system' ? detectedSystemLocale.value : localePreference.value)

export const i18n = createI18n({
  legacy: false,
  locale: locale.value,
  fallbackLocale: 'zh-CN',
  messages: { 'zh-CN': zhCN, 'en-US': enUS },
  messageResolver: (messages, key) => {
    if (!messages || typeof messages !== 'object') return null
    const value = (messages as Record<string, unknown>)[key]
    return value === undefined ? null : value as string
  },
  missingWarn: false,
  fallbackWarn: false,
})

watch(locale, value => {
  i18n.global.locale.value = value
  if (typeof document !== 'undefined') document.documentElement.lang = value
}, { immediate: true })

export function setLocalePreference(value: LocalePreference) {
  localePreference.value = value
  try {
    if (value === 'system') localStorage.removeItem(STORAGE_KEY)
    else localStorage.setItem(STORAGE_KEY, value)
  } catch { /* Keep the current session setting even if it cannot be persisted. */ }
}

/** Translate catalog keys; paired strings remain available for runtime-generated messages. */
export function t(key: string, english?: string): string {
  if (english !== undefined) return locale.value === 'zh-CN' ? key : english
  return i18n.global.t(key)
}

if (typeof window !== 'undefined') {
  window.addEventListener('languagechange', () => {
    detectedSystemLocale.value = systemLocale()
  })
  document.documentElement.lang = locale.value
}

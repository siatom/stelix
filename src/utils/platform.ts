export const IS_MACOS =
  typeof navigator !== 'undefined' && /Mac/i.test(navigator.userAgent)

export const IS_WINDOWS =
  typeof navigator !== 'undefined' && /Windows/i.test(navigator.userAgent)

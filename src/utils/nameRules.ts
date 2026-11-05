// 文件 / 文件夹名称校验：在提交到后端前拦住平台非法名称，避免只拿到系统错误码。

const IS_WINDOWS =
  typeof navigator !== 'undefined' && /Windows/i.test(navigator.userAgent)

/** Windows 保留设备名（大小写不敏感，带扩展名同样非法，如 `CON.txt`）。 */
const RESERVED_NAME = /^(?:con|prn|aux|nul|com[1-9]|lpt[1-9])(?:\..*)?$/i
/** 文件名非法字符：Windows 限制更多；POSIX 只禁止 `/` 与 NUL（冒号在 Finder 中也有歧义）。 */
const INVALID_CHARS = IS_WINDOWS ? /[<>:"/\\|?*\u0000-\u001f]/ : /[/:\u0000]/
/** C0 控制字符（含制表符、换行）。 */
const CONTROL_CHARS = /[\u0000-\u001f]/

export interface NameRuleOptions {
  /** 允许空名称（例如「留空即取消」的输入框）。 */
  allowEmpty?: boolean
}

/**
 * 校验单个文件 / 文件夹名称。
 *
 * @returns `null` 表示合法；否则返回可直接展示给用户的中文原因。
 */
export function validateEntryName(name: string, options?: NameRuleOptions): string | null {
  if (!name.length) return options?.allowEmpty ? null : '名称不能为空'
  if (name === '.' || name === '..') return '名称不能是「.」或「..」'
  if (INVALID_CHARS.test(name)) {
    return IS_WINDOWS
      ? '名称不能包含 < > : " / \\ | ? * 这些字符'
      : '名称不能包含 / 或 : 字符'
  }
  if (CONTROL_CHARS.test(name)) return '名称不能包含控制字符'
  if (IS_WINDOWS && RESERVED_NAME.test(name)) {
    return '名称不能使用 Windows 保留名（CON / PRN / AUX / NUL / COM1-9 / LPT1-9）'
  }
  if (IS_WINDOWS && /[ .]$/.test(name)) return '名称不能以空格或点结尾'
  if (name.length > 255) return '名称不能超过 255 个字符'
  return null
}

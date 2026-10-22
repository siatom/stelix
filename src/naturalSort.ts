/**
 * 名称的自然排序：数字按数值大小比较。
 *
 * 例如「文件9」排在「文件10」之前，而不是按字符逐位比较（那样「文件10」会排在「文件9」前）。
 *
 * 复用同一个 `Intl.Collator`：`String.prototype.localeCompare` 传 locale/options 时
 * 每次比较都会重建比较器，大目录（上万条）排序会因此慢一个数量级。
 */
const nameCollator = new Intl.Collator('zh-Hans-CN', { numeric: true, sensitivity: 'base' })

/** 按自然顺序比较两个名称（数字按数值大小，其余按中文 / 字母顺序）。 */
export function compareNames(a: string, b: string): number {
  return nameCollator.compare(a, b)
}

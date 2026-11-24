const UNIT_SIZE = { kb: 1024, mb: 1024 * 1024, gb: 1024 * 1024 * 1024 }

export function formatBytes(bytes: number): string {
  if (bytes >= UNIT_SIZE.gb) return `${trimNumber(bytes / UNIT_SIZE.gb)} GB`
  if (bytes >= UNIT_SIZE.mb) return `${trimNumber(bytes / UNIT_SIZE.mb)} MB`
  if (bytes >= UNIT_SIZE.kb) return `${trimNumber(bytes / UNIT_SIZE.kb)} KB`
  return `${bytes} B`
}

function trimNumber(value: number): string {
  return Number(value.toFixed(2)).toString()
}

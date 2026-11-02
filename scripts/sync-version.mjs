#!/usr/bin/env node
// 把发布版本号从 git tag 同步到各处版本字段。
//
// 用法：node scripts/sync-version.mjs v1.2.3
//
// 同步目标：
//   package.json            version
//   src-tauri/tauri.conf.json   version（决定安装包文件名与 exe 元数据）
//   src-tauri/Cargo.toml    [package] version
//   src-tauri/Cargo.lock    本包条目 version（避免下次构建产生无关 diff）
//
// 已同步时不做任何写入，可安全重复执行。

import { readFile, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')

const raw = process.argv[2]
if (!raw || raw === '-h' || raw === '--help') {
  console.error('usage: node scripts/sync-version.mjs <version|vX.Y.Z>')
  process.exit(raw ? 0 : 2)
}

const version = raw.trim().replace(/^v/i, '')
if (!/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/.test(version)) {
  console.error(`[sync-version] invalid semver: ${raw}`)
  process.exit(1)
}

let changed = 0

/** 读取文件、交给 transform 生成新内容，有变化才写回。 */
async function rewrite(relativePath, transform) {
  const file = path.join(root, relativePath)
  const before = await readFile(file, 'utf8')
  const after = transform(before)
  if (after === before) {
    console.log(`[sync-version] ${relativePath}: already ${version}`)
    return
  }
  await writeFile(file, after)
  changed += 1
  console.log(`[sync-version] ${relativePath}: updated`)
}

/** JSON 文件只改 version 字段，保持两空格缩进与末尾换行。 */
function rewriteJson(text) {
  const data = JSON.parse(text)
  data.version = version
  return `${JSON.stringify(data, null, 2)}\n`
}

await rewrite('package.json', rewriteJson)
await rewrite('src-tauri/tauri.conf.json', rewriteJson)

await rewrite('src-tauri/Cargo.toml', (text) => {
  // 只在 [package] 段内替换 version，避免误改依赖里的 version。
  const header = /^\[package\][ \t]*\r?\n/m.exec(text)
  if (!header) throw new Error('[sync-version] [package] section not found in Cargo.toml')
  const sectionStart = header.index
  const bodyStart = sectionStart + header[0].length
  const nextSection = /^\[/m.exec(text.slice(bodyStart))
  const sectionEnd = nextSection ? bodyStart + nextSection.index : text.length
  const block = text.slice(sectionStart, sectionEnd)
  const versionPattern = /(^version[ \t]*=[ \t]*")[^"]*(")/m
  if (!versionPattern.test(block)) {
    throw new Error('[sync-version] version field not found in Cargo.toml [package]')
  }
  const nextBlock = block.replace(versionPattern, `$1${version}$2`)
  return text.slice(0, sectionStart) + nextBlock + text.slice(sectionEnd)
})

await rewrite('src-tauri/Cargo.lock', (text) => {
  const pattern = /(\[\[package\]\]\r?\nname = "stelix-file-manager"\r?\nversion = ")[^"]*(")/
  if (!pattern.test(text)) {
    throw new Error('[sync-version] package entry not found in Cargo.lock')
  }
  return text.replace(pattern, `$1${version}$2`)
})

console.log(`[sync-version] version ${version} (${changed} file(s) updated)`)

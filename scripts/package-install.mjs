import { existsSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { spawnSync } from 'node:child_process'

function findWindowsBash() {
  const gitPaths = spawnSync('where.exe', ['git'], { encoding: 'utf8' })
  const candidates = []

  for (const gitPath of (gitPaths.stdout || '').split(/\r?\n/).filter(Boolean)) {
    const gitDirectory = dirname(gitPath)
    const gitRoot = ['bin', 'cmd'].includes(gitDirectory.split(/[\\/]/).pop().toLowerCase())
      ? dirname(gitDirectory)
      : gitDirectory
    candidates.push(join(gitRoot, 'bin', 'bash.exe'), join(gitRoot, 'usr', 'bin', 'bash.exe'))
  }

  if (process.env.ProgramFiles) {
    candidates.push(join(process.env.ProgramFiles, 'Git', 'bin', 'bash.exe'))
  }

  return candidates.find(existsSync)
}

let bash
if (process.platform === 'win32') {
  bash = findWindowsBash()
  if (!bash) {
    console.error('[package:install] Git Bash was not found; install Git for Windows and retry.')
    process.exit(1)
  }
} else if (process.platform === 'darwin') {
  bash = '/bin/bash'
} else {
  console.error('[package:install] supported platforms are Windows (Git for Windows) and macOS')
  process.exit(1)
}

const result = spawnSync(bash, ['scripts/build-install.sh', ...process.argv.slice(2)], {
  cwd: resolve('.'),
  stdio: 'inherit',
})

if (result.error) {
  console.error(`[package:install] failed to start Git Bash: ${result.error.message}`)
  process.exit(1)
}

process.exit(result.status ?? 1)

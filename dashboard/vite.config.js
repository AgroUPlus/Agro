import { existsSync, readFileSync, statSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

// The dashboard is embedded in the server binary, so the crate's version is the server's version.
// Read from Cargo.toml rather than package.json, which is never bumped.
const cargoToml = readFileSync(new URL('../Cargo.toml', import.meta.url), 'utf8')
const crateVersion = cargoToml.match(/^version\s*=\s*"([^"]+)"/m)?.[1]
if (!crateVersion) throw new Error('No version found in ../Cargo.toml')

/**
 * The commit being built, read from the repository's own files rather than by running `git`.
 *
 * Spawning a program found on `PATH` at build time means trusting whatever is first on it, which
 * is the one thing a build should not do. `HEAD` and its ref are plain files: a branch's commit is
 * either its own file or a line in `packed-refs`, and a worktree's `.git` is a file naming the real
 * directory. CI says it outright in `GITHUB_SHA`. A source tarball has none of these, and shows
 * the version alone rather than a made-up hash.
 */
function readCommit() {
  if (process.env.GITHUB_SHA) return process.env.GITHUB_SHA.slice(0, 7)
  const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
  let gitDir = resolve(root, '.git')
  if (!existsSync(gitDir)) return ''
  if (statSync(gitDir).isFile()) {
    const pointer = readFileSync(gitDir, 'utf8').match(/^gitdir:\s*(.+)$/m)?.[1]
    if (!pointer) return ''
    gitDir = resolve(root, pointer.trim())
  }
  const head = readFileSync(resolve(gitDir, 'HEAD'), 'utf8').trim()
  const ref = head.match(/^ref:\s*(.+)$/)?.[1]
  if (!ref) return head.slice(0, 7)
  // A worktree keeps its own HEAD but shares refs with the main repository.
  const common = existsSync(resolve(gitDir, 'commondir'))
    ? resolve(gitDir, readFileSync(resolve(gitDir, 'commondir'), 'utf8').trim())
    : gitDir
  for (const dir of [gitDir, common]) {
    const loose = resolve(dir, ref)
    if (existsSync(loose)) return readFileSync(loose, 'utf8').trim().slice(0, 7)
  }
  const packed = resolve(common, 'packed-refs')
  if (!existsSync(packed)) return ''
  const line = readFileSync(packed, 'utf8').split('\n').find((l) => l.endsWith(` ${ref}`))
  return line ? line.slice(0, 7) : ''
}

const commit = readCommit()

// https://vite.dev/config/
export default defineConfig({
  plugins: [react()],
  define: {
    'import.meta.env.VITE_AGRO_VERSION': JSON.stringify(
      commit ? `v${crateVersion} · ${commit}` : `v${crateVersion}`
    ),
  },
})

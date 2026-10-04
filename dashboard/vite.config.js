import { execSync } from 'node:child_process'
import { readFileSync } from 'node:fs'
import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

// The dashboard is embedded in the server binary, so the crate's version is the server's version.
// Read from Cargo.toml rather than package.json, which is never bumped.
const cargoToml = readFileSync(new URL('../Cargo.toml', import.meta.url), 'utf8')
const crateVersion = cargoToml.match(/^version\s*=\s*"([^"]+)"/m)?.[1]
if (!crateVersion) throw new Error('No version found in ../Cargo.toml')

// The commit, when there is one to read: a build from a source tarball has no git, and shows the
// version alone rather than a made-up hash.
let commit = ''
try {
  commit = execSync('git rev-parse --short HEAD', { stdio: ['ignore', 'pipe', 'ignore'] })
    .toString()
    .trim()
} catch {
  commit = ''
}

// https://vite.dev/config/
export default defineConfig({
  plugins: [react()],
  define: {
    'import.meta.env.VITE_AGRO_VERSION': JSON.stringify(
      commit ? `v${crateVersion} · ${commit}` : `v${crateVersion}`
    ),
  },
})

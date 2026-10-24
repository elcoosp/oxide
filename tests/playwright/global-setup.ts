// Builds the smoke app once, before any spec runs. `oxide start` alone
// serves SSR-on-the-fly with no client bundle — no boot script, no
// hydration, no client router, no loading boundaries. The island,
// loading, socket, and sse specs all depend on client JS, so the build
// must run first.

import { execSync } from 'node:child_process'
import { existsSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const __dirname = dirname(fileURLToPath(import.meta.url))
const repoRoot = join(__dirname, '..', '..')
const smokeRoot = join(repoRoot, 'examples', 'smoke-app-react')
const oxideBin = join(repoRoot, 'target', 'debug', 'oxide')

export default async function globalSetup(): Promise<void> {
  if (!existsSync(oxideBin)) {
    throw new Error(
      `oxide binary missing at ${oxideBin}; run \`cargo build -p oxide-fw-cli\` first`,
    )
  }
  const clientManifest = join(smokeRoot, 'dist', 'client', 'client.manifest.json')
  if (existsSync(clientManifest)) {
    console.log('[global-setup] smoke-app-react already built — reusing dist/')
    return
  }
  console.log('[global-setup] building smoke-app-react (one-time)…')
  execSync(`"${oxideBin}" build --root "${smokeRoot}"`, { stdio: 'inherit' })
  if (!existsSync(clientManifest)) {
    throw new Error(`build succeeded but ${clientManifest} is missing`)
  }
}

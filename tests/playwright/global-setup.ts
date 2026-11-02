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
  // §4.1: on CI, verify react-refresh is installed in the smoke app.
  // Without it, `HMR_AVAILABLE = existsSync(smokeRoot/node_modules/react-refresh)`
  // evaluates false and EVERY HMR test calls `test.skip()` — the job goes
  // green while testing nothing. That is the silent-failure mode this guard
  // exists to prevent: a runner-provisioning gap becomes a red CI job with
  // a clear message, instead of a green job that tested half the suite.
  //
  // Locally the assertion is a no-op: developers who haven't run
  // `pnpm install` in the smoke app can still run non-HMR specs, and the
  // HMR tests skip as before.
  const reactRefresh = join(smokeRoot, 'node_modules', 'react-refresh')
  if (process.env.CI && !existsSync(reactRefresh)) {
    throw new Error(
      `react-refresh missing at ${reactRefresh}. In CI this means every HMR test will ` +
      `silently skip (HMR_AVAILABLE=false), masking real regressions. Fix the provisioning ` +
      `step: \`pnpm install\` must reach examples/smoke-app-react (see pnpm-workspace.yaml).`,
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

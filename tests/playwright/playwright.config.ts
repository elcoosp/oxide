import { defineConfig, devices } from '@playwright/test'

// §22 real-browser E2E. Boots `oxide start` on the smoke-app, then drives
// the spec set against Chromium, Firefox and WebKit — §22 names browser
// coverage as a release gate, and a single engine is not that.
//
// Locally, run one browser at a time (each needs ~500 MB):
//   pnpm --filter @oxide/playwright install-browsers
//   pnpm --filter @oxide/playwright exec playwright test --project=chromium
//
// CI runs all three via a matrix over `--project` (the `playwright` job), so
// a Firefox-only regression fails the build without paying three engines
// in one job.
export default defineConfig({
  testDir: './tests',
  globalSetup: './global-setup.ts',
  fullyParallel: false,
  // Retries catch genuine test-vs-test timing races (health-check probes,
  // first-request cache-warm races). A test that fails once and passes on
  // retry is reported as `flaky` — worth investigating if it becomes
  // frequent, but not a build break.
  retries: 1,
  // Each spec file spawns its own `oxide start` (server + V8 worker pool +
  // Node shim). Under the default (CPU-count) parallelism on a laptop,
  // ~10 files × ~5 processes each = 50 processes race for cores, and an
  // otherwise-passing test occasionally trips a liveness timeout. Capping
  // at 3 keeps the suite under ~15 processes and each spec gets a real
  // slice of CPU. Wall-clock cost is ~2× vs unbounded on a 10-core
  // machine; correctness is worth it.
  workers: 3,
  reporter: [['list']],
  timeout: 15_000,
  use: {
    baseURL: 'http://127.0.0.1:3100',
    trace: 'retain-on-failure',
  },
  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
    },
    {
      // §22: a WebKit-only or Gecko-only regression is exactly what a
      // single-engine suite cannot see. Firefox is the Gecko check.
      name: 'firefox',
      use: { ...devices['Desktop Firefox'] },
    },
    {
      // WebKit exercises the Safari family — the engine with the most
      // divergent streaming/DOM behaviour, so the most worth covering.
      name: 'webkit',
      use: { ...devices['Desktop Safari'] },
    },
  ],
})

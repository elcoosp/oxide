import { defineConfig, devices } from '@playwright/test'

// §22 real-browser E2E. Boots `oxide start` on the smoke-app, then drives
// Chromium against it. Not part of the default test suite — the workspace's
// dev host has ~4 GB RAM and Playwright's Chromium needs ~500 MB on its own.
//
// Run:
//   pnpm --filter @oxide/playwright install-browsers
//   pnpm --filter @oxide/playwright test
export default defineConfig({
  testDir: './tests',
  globalSetup: './global-setup.ts',
  fullyParallel: false,
  retries: 1,
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
  ],
})

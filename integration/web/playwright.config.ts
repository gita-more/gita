import { defineConfig } from '@playwright/test'

import { E2E_BASE, externalUrl } from './tests/server-env'

// The web E2E configuration.
//
// The suite starts its own server (see `tests/global-setup.ts`), so
// there is nothing to run by hand and nothing to remember to clean up
// afterwards. Each run gets its own Postgres schema and its own data
// directory, both of which are removed when the run ends.
//
// The port is the suite's own, not the development deployment's: a
// server that was already listening would be tested instead of the one
// the suite started, and the failure would look like a broken login.

const GITA_URL = externalUrl() ?? E2E_BASE

export default defineConfig({
  testDir: './tests',
  timeout: 30_000,
  fullyParallel: false,
  // Serial execution: the tests share one running gita-server, and
  // parallel files would race on the repositories they create.
  workers: 1,
  // A retry hides a race: the same test that passed the second time is
  // still broken. Keep failures visible instead.
  retries: 0,
  // `.only` that reaches CI silently disables the rest of the suite.
  forbidOnly: !!process.env.CI,
  reporter: [['list']],
  outputDir: 'test-results',
  expect: {
    timeout: 5_000,
  },
  globalSetup: './tests/global-setup.ts',
  use: {
    baseURL: GITA_URL,
    headless: true,
    viewport: { width: 1280, height: 800 },
    actionTimeout: 10_000,
    navigationTimeout: 15_000,
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    video: 'retain-on-failure',
  },
  projects: [
    {
      name: 'chromium',
      use: { browserName: 'chromium' },
    },
  ],
})

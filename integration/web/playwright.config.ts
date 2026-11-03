import { defineConfig } from '@playwright/test'

// Gita Web E2E 测试配置
// 浏览器使用本机缓存的 Chromium（与 @playwright/test 版本匹配），不重新下载。
// 服务地址默认 http://127.0.0.1:3100（可用环境变量 GITA_URL 覆盖）。
const GITA_URL = process.env.GITA_URL ?? 'http://127.0.0.1:3100'

export default defineConfig({
  testDir: './tests',
  timeout: 30_000,
  fullyParallel: false,
  // Serial execution: tests share the same running gita-server and can
  // create repos/users that later tests rely on; parallel files would
  // race on repo visibility.
  workers: 1,
  // A retry hides a race: the same test that passed on the second try is
  // still broken. Keep failures visible instead.
  retries: 0,
  // `.only` that reaches CI silently disables the rest of the suite.
  forbidOnly: !!process.env.CI,
  reporter: [['list']],
  outputDir: 'test-results',
  expect: {
    timeout: 5_000,
  },
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
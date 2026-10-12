import { defineConfig } from '@playwright/test'

// Gita E2E 测试配置
// 浏览器使用本机缓存的 Chromium（与 @playwright/test 版本匹配），不重新下载。
// 服务地址默认 http://127.0.0.1:3100（可用环境变量 GITA_URL 覆盖）。
const GITA_URL = process.env.GITA_URL ?? 'http://127.0.0.1:3100'

export default defineConfig({
  testDir: './tests',
  timeout: 30_000,
  fullyParallel: false,
  retries: 0,
  reporter: [['list']],
  use: {
    baseURL: GITA_URL,
    headless: true,
    viewport: { width: 1280, height: 800 },
    trace: 'retain-on-failure',
  },
  projects: [
    {
      name: 'chromium',
      use: { browserName: 'chromium' },
    },
  ],
})
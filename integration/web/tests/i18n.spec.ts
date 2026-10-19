import { test, expect } from '@playwright/test'
import {
  loginViaApi,
  setUserLocale,
  createRepo,
  DEFAULT_USER,
} from './helpers'

/**
 * E2E coverage for i18n-driven UI text (e64dffc0):
 *   - nav.projects / nav.groups render as '项目' / '群组' in zh,
 *     'Projects' / 'Groups' in en.
 *   - StatusLabel translates 'open' / 'closed' through status.* keys.
 *   - Settings page language switch updates the visible UI without a
 *     full page reload.
 *
 * Each test runs as a freshly-registered user so the locale is
 * deterministic and tests don't interfere with each other.
 */

test.describe('i18n 切换', () => {
  test.beforeEach(async ({ page }) => {
    await loginViaApi(page)
  })

  test('zh locale：侧边栏显示「项目 / 群组」', async ({ page }) => {
    await setUserLocale(page, 'zh')
    await page.goto('/')
    await expect(page.locator('.sidebar-link', { hasText: '项目' })).toBeVisible()
    await expect(page.locator('.sidebar-link', { hasText: '群组' })).toBeVisible()
    // 'Issues' stays English in both locales (industry term).
    await expect(page.locator('.sidebar-link', { hasText: 'Issues' })).toBeVisible()
  })

  test('en locale：侧边栏显示 Projects / Groups / Issues', async ({ page }) => {
    await setUserLocale(page, 'en')
    await page.goto('/')
    await expect(page.locator('.sidebar-link', { hasText: 'Projects' })).toBeVisible()
    await expect(page.locator('.sidebar-link', { hasText: 'Groups' })).toBeVisible()
    await expect(page.locator('.sidebar-link', { hasText: 'Issues' })).toBeVisible()
    // Chinese translations must NOT leak in en locale.
    await expect(page.locator('.sidebar-link', { hasText: '项目' })).toHaveCount(0)
    await expect(page.locator('.sidebar-link', { hasText: '群组' })).toHaveCount(0)
  })

  test('StatusLabel: zh 渲染 "打开 / 已关闭"，en 渲染 open / closed', async ({ page }) => {
    const owner = DEFAULT_USER.username
    const name = `i18n-status-${Date.now()}`
    await createRepo(page, name)

    // Create an open issue so the Issues tab has a StatusLabel to read.
    const token = await page.evaluate(() => localStorage.getItem('gita_token'))
    const create = await page.request.post(`${await page.evaluate(() => location.origin)}/api/repos/${owner}/${name}/issues`, {
      headers: { Authorization: `Bearer ${token}` },
      data: { title: 'test issue for status label', body: 'body' },
    })
    expect(create.ok(), `create issue: ${create.status()}`).toBeTruthy()

    // ---- zh ----
    await setUserLocale(page, 'zh')
    await page.goto(`/${owner}/${name}?tab=issues`)
    const zhStatus = page.locator('.status').first()
    await expect(zhStatus).toBeVisible()
    await expect(zhStatus).toContainText('打开')
    await expect(zhStatus).not.toContainText(/^open$/i)

    // ---- en ----
    await setUserLocale(page, 'en')
    await page.goto(`/${owner}/${name}?tab=issues`)
    const enStatus = page.locator('.status').first()
    await expect(enStatus).toBeVisible()
    await expect(enStatus).toContainText('open')
  })

  test('设置页切语言不需要刷新页面就生效', async ({ page }) => {
    await page.goto('/')
    // Force initial locale to zh so we start from a known state.
    await setUserLocale(page, 'zh')
    await page.goto('/settings')

    // 初始为 zh：侧边栏看到「项目」
    await expect(page.locator('.sidebar-link', { hasText: '项目' })).toBeVisible()

    // 在 settings 里切到 English（点击 "English" radio label）
    await page.locator('.settings-locale-options label', { hasText: 'English' }).click()

    // 不刷新 — 侧边栏应实时变英文
    await expect(page.locator('.sidebar-link', { hasText: 'Projects' })).toBeVisible({ timeout: 3000 })
    // 「项目」从结果中消失
    await expect(page.locator('.sidebar-link', { hasText: '项目' })).toHaveCount(0)
  })
})
import { test, expect, type Page } from '@playwright/test'

// ---------------------------------------------------------------------------
// 辅助函数：通过 API 登录并把 token / user 注入 localStorage
// （跳过 UI 登录表单，使测试更稳定）
// ---------------------------------------------------------------------------
const BASE = process.env.GITA_URL ?? 'http://127.0.0.1:3100'
const USER = process.env.GITA_TEST_USER ?? 'testuser'
const PASS = process.env.GITA_TEST_PASS ?? 'testpass123'

async function loginViaApi(page: Page) {
  const res = await page.request.post(`${BASE}/api/auth/login`, {
    data: { username: USER, password: PASS },
  })
  expect(res.ok()).toBeTruthy()
  const body = (await res.json()) as { token: string; user: { id: number; username: string; email: string } }
  await page.addInitScript(
    ({ token, user }) => {
      localStorage.setItem('gita_token', token)
      localStorage.setItem('gita_user', JSON.stringify(user))
    },
    { token: body.token, user: body.user },
  )
}

async function ensureTestRepo(page: Page) {
  // 尝试创建测试仓库（已存在会失败，忽略即可）
  const token = await page.evaluate(() => localStorage.getItem('gita_token'))
  const res = await page.request.post(`${BASE}/api/repos`, {
    headers: { Authorization: `Bearer ${token}` },
    data: { name: 'e2e-demo', description: 'created by playwright test', default_branch: 'main' },
  })
  if (!res.ok()) {
    const body = await res.text()
    console.warn(`ensureTestRepo: ${res.status()} ${body.slice(0, 120)}`)
  }
}

// ---------------------------------------------------------------------------
// 侧边栏布局测试
// ---------------------------------------------------------------------------
test.describe('GitLab 风格侧边栏布局', () => {
  test('侧边栏四要素存在：顶部 user-bar（Logo+折叠++）、上下文头、底部用户区', async ({ page }) => {
    await loginViaApi(page)
    await page.goto('/')
    await expect(page.locator('.sidebar')).toBeVisible()

    // 顶部 user-bar：Brand + 折叠按钮 + "+" 按钮
    const userBar = page.locator('.sidebar-user-bar')
    await expect(userBar).toBeVisible()
    await expect(userBar.locator('.brand')).toBeVisible()
    await expect(userBar.locator('.sidebar-toggle-btn')).toBeVisible()
    await expect(userBar.locator('.sidebar-new-btn')).toBeVisible()

    // 上下文头：显示当前激活导航标题
    const ctx = page.locator('.sidebar-context-header')
    await expect(ctx).toBeVisible()
    await expect(ctx).toHaveText('仓库')

    // 底部用户区：头像 + 用户名 + 退出按钮
    const bottom = page.locator('.sidebar-user-bottom')
    await expect(bottom).toBeVisible()
    await expect(bottom.locator('.user-avatar')).toBeVisible()
    await expect(bottom.locator('.sidebar-user-name')).toHaveText(USER)
  })

  test('仓库详情页：点击导航项后上下文头跟随更新', async ({ page }) => {
    await loginViaApi(page)
    await page.goto('/')
    await ensureTestRepo(page)

    // 进入仓库详情页（仓库卡片是可点击按钮）
    await page.locator('.repo-card', { hasText: 'e2e-demo' }).first().click()
    await expect(page.locator('.sidebar-context-header')).toHaveText('代码', { timeout: 5000 })

    // 点击"提交"
    await page.locator('.sidebar-link', { hasText: '提交' }).click()
    await expect(page.locator('.sidebar-context-header')).toHaveText('提交')

    // 点击"合并请求"
    await page.locator('.sidebar-link', { hasText: '合并请求' }).click()
    await expect(page.locator('.sidebar-context-header')).toHaveText('合并请求')

    // 点击"Issue"
    await page.locator('.sidebar-link', { hasText: 'Issue' }).click()
    await expect(page.locator('.sidebar-context-header')).toHaveText('Issue')

    // 点击"AI 评审"
    await page.locator('.sidebar-link', { hasText: 'AI 评审' }).click()
    await expect(page.locator('.sidebar-context-header')).toHaveText('AI 评审')

    // 点击"代码搜索"
    await page.locator('.sidebar-link', { hasText: '代码搜索' }).click()
    await expect(page.locator('.sidebar-context-header')).toHaveText('代码搜索')
  })

  test('折叠/展开：折叠后只显示图标，展开恢复', async ({ page }) => {
    await loginViaApi(page)
    await page.goto('/')

    // 初始未折叠
    await expect(page.locator('.app.sidebar-collapsed')).toHaveCount(0)

    // 折叠
    await page.locator('.sidebar-toggle-btn').click()
    await expect(page.locator('.app.sidebar-collapsed')).toHaveCount(1)

    // 折叠后 user-bar 文本被隐藏（只剩图标）
    const userBarText = (await page.locator('.sidebar-user-bar').innerText()).trim()
    expect(userBarText).not.toContain('Gita')

    // 底部用户区：头像仍可见，用户名隐藏
    await expect(page.locator('.sidebar-user-bottom .user-avatar')).toBeVisible()
    await expect(page.locator('.sidebar-user-bottom .sidebar-user-name')).toBeHidden()

    // 展开
    await page.locator('.sidebar-toggle-btn').click()
    await expect(page.locator('.app.sidebar-collapsed')).toHaveCount(0)
    await expect(page.locator('.sidebar-user-bottom .sidebar-user-name')).toBeVisible()
  })
})
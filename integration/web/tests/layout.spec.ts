import { test, expect, type Page } from '@playwright/test'
import { loginViaApi, setUserLocale, currentToken, BASE } from './helpers'

// ---------------------------------------------------------------------------
// 侧边栏布局测试
// ---------------------------------------------------------------------------
test.describe('侧边栏布局', () => {
  test('顶栏包含品牌、折叠按钮、搜索框；侧边栏显示全局导航', async ({ page }) => {
    await loginViaApi(page)
    await page.goto('/')

    // 顶栏：折叠按钮 + 品牌 + 搜索框
    await expect(page.locator('.topbar')).toBeVisible()
    await expect(page.locator('.sidebar-toggle')).toBeVisible()
    await expect(page.locator('.brand')).toBeVisible()
    await expect(page.locator('.search-box')).toBeVisible()

    // 全局视图侧边栏主导航：Projects / Groups / Issues。
    // （仓库/代码/提交/合并请求/Issue 是仓库详情页的侧边栏，由下一个测试覆盖。）
    await expect(page.locator('.sidebar')).toBeVisible()
    for (const label of ['Projects', 'Groups', 'Issues']) {
      await expect(page.locator('.sidebar-link', { hasText: label }).first()).toBeVisible()
    }
  })

  test('仓库详情页：点击导航项后高亮跟随、内容面板切换', async ({ page }) => {
    await loginViaApi(page)
    // 本用例断言中文标签（代码/提交/合并请求）。账号 locale 是持久化到
    // 数据库的，之前的运行可能把它留在 en，导致这里找不到元素。
    await setUserLocale(page, 'zh')
    await page.goto('/')
    await ensureTestRepo(page)
    // Re-navigate so the repo list re-fetches and includes the
    // just-created e2e-demo repository.
    await page.goto('/')

    // 进入仓库详情页（仓库卡片是可点击按钮）
    await page.locator('.repo-card', { hasText: 'e2e-demo' }).first().click()
    // 仓库页默认高亮"代码"
    await expect(page.locator('.sidebar-link', { hasText: /^代码$/ })).toHaveClass(/active/, { timeout: 5000 })

    // 点击"提交"
    await page.locator('.sidebar-link', { hasText: /^提交$/ }).click()
    await expect(page.locator('.sidebar-link', { hasText: /^提交$/ })).toHaveClass(/active/)

    // 点击"合并请求"
    await page.locator('.sidebar-link', { hasText: /^合并请求$/ }).click()
    await expect(page.locator('.sidebar-link', { hasText: /^合并请求$/ })).toHaveClass(/active/)

    // 点击"Issue"
    await page.locator('.sidebar-link', { hasText: /^Issue$/ }).click()
    await expect(page.locator('.sidebar-link', { hasText: /^Issue$/ })).toHaveClass(/active/)
  })

  test('折叠/展开：折叠后只显示图标，展开恢复', async ({ page }) => {
    await loginViaApi(page)
    await page.goto('/')

    // 初始未折叠
    await expect(page.locator('.app.sidebar-collapsed')).toHaveCount(0)

    // 折叠
    await page.locator('.sidebar-toggle').click()
    await expect(page.locator('.app.sidebar-collapsed')).toHaveCount(1)

    // 折叠后侧边栏文字隐藏
    await expect(page.locator('.sidebar-link span').first()).toBeHidden()

    // 展开
    await page.locator('.sidebar-toggle').click()
    await expect(page.locator('.app.sidebar-collapsed')).toHaveCount(0)
    await expect(page.locator('.sidebar-link span').first()).toBeVisible()
  })

  test('仓库卡片是单行 flex 布局', async ({ page }) => {
    await loginViaApi(page)
    await page.goto('/')
    await ensureTestRepo(page)
    await page.goto('/')

    const card = page.locator('.repo-card', { hasText: 'e2e-demo' }).first()
    await expect(card).toBeVisible()
    // 5adbd7b7 把 .repo-card 从 grid 列改成单行 flex 行；
    // flex-direction 一旦回到 column 就视为回归。
    const dir = await card.evaluate((el) => getComputedStyle(el).flexDirection)
    expect(dir).toBe('row')
  })
})

async function ensureTestRepo(page: Page) {
  // 尝试创建测试仓库（已存在会失败，忽略即可）
  const token = await currentToken(page)
  const res = await page.request.post(`${BASE}/api/repos`, {
    headers: { Authorization: `Bearer ${token}` },
    data: { name: 'e2e-demo', description: 'created by playwright test', default_branch: 'main' },
  })
  if (!res.ok()) {
    const body = await res.text()
    console.warn(`ensureTestRepo: ${res.status()} ${body.slice(0, 120)}`)
  }
}
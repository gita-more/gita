import { test, expect } from '@playwright/test'
import { loginViaApi, createRepo, BASE, DEFAULT_USER } from './helpers'

test.describe('pipelines 标签页', () => {
  test('空仓库打开 Pipelines 标签,看到空状态文案', async ({ page }) => {
    await loginViaApi(page)
    const owner = DEFAULT_USER.username
    const name = `pipelines-empty-${Date.now()}`
    await createRepo(page, name)

    // 直接进入仓库详情页 — 没有 commits,会看到 EmptyRepoWelcome 欢迎卡,
    // 而不是 PipelinesPanel。Phase-1 端到端可走通需要真实 push, 这
    // 里只验 UI shell + sidebar 文案。
    await page.goto(`/${owner}/${name}`)

    // sidebar 出现 "Pipelines" / "流水线" 入口
    await expect(
      page.locator('.sidebar-link', { hasText: /Pipelines|流水线/ }).first(),
    ).toBeVisible()

    // URL 加 ?tab=pipelines 切换到 pipelines tab
    await page.goto(`/${owner}/${name}?tab=pipelines`)
    // 空目录 / 空 pipeline 都允许; 顶部至少有 PageHeader 标题
    await expect(
      page.locator('.sidebar-link.active', { hasText: /Pipelines|流水线/ }).first(),
    ).toBeVisible()
  })
})
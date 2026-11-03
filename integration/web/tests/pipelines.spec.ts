import { test, expect } from '@playwright/test'
import { loginViaApi, createRepo, createRepoWithCommit, BASE, DEFAULT_USER } from './helpers'

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

    // 空仓库会渲染 EmptyRepoWelcome，而不是仓库详情页的侧边栏 —
    // 所以这里没有 `.sidebar-link` 可断言（原来两条断言都依赖它）。
    // 用真实 push 建一个仓库，才是能打开 Pipelines 标签的前提。
    const nameWithCommit = `pipelines-tab-${Date.now()}`
    await createRepoWithCommit(page, nameWithCommit)

    await page.goto(`/${owner}/${nameWithCommit}`)
    // sidebar 出现 "Pipelines" / "流水线" 入口
    await expect(
      page.locator('.sidebar-link', { hasText: /Pipelines|流水线/ }).first(),
    ).toBeVisible()

    // Tab 是路径段（不是查询参数）：/owner/repo/pipelines
    await page.goto(`/${owner}/${nameWithCommit}/pipelines`)
    await expect(
      page.locator('.sidebar-link.active', { hasText: /Pipelines|流水线/ }).first(),
    ).toBeVisible()
  })
})
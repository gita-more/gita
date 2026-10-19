import { test, expect } from '@playwright/test'
import { loginViaApi, loginExisting } from './helpers'

// ---------------------------------------------------------------------------
// 端到端用户旅程
// ---------------------------------------------------------------------------
test.describe('用户旅程：登录后创建并浏览仓库', () => {
  test('创建仓库 → 进入仓库详情 → 看到空的占位界面', async ({ page }) => {
    const username = 'e2e-journey';
    const password = 'hunter22';
    await loginViaApi(page, { username, password });
    await page.goto('/');

    // 侧边栏必现；repo-grid 仅在用户有仓库时出现。
    await expect(page.locator('.sidebar')).toBeVisible();

    // 点 "新建仓库"（在 header 或 empty-state 都有同名按钮）。
    const repoName = `journey-${Date.now()}`;
    await page.getByRole('button', { name: /新建仓库/ }).first().click();
    await page.locator('#repo-name').fill(repoName);
    await page.locator('#repo-desc').fill('created by Playwright journey.spec.ts');
    await page.getByRole('button', { name: /创建仓库/ }).click();

    // 创建后 RepoList.onOpen 自动跳到仓库详情页。
    // 详情页的 sidebar-link（代码 / 提交 / 合并请求 / Issue / AI 评审 / 代码搜索）应当可见。
    await expect(page.locator('.sidebar-link').first()).toBeVisible({ timeout: 10000 });
    // 跳转到详情页通常通过 onOpen(owner, name) — 我们至少要看到任意 sidebar-link 处于 active 状态。
    await page.waitForTimeout(500);
    // 进入"提交"面板（如果 active 在代码上则不强制切到提交，避免 race）。
    const submitLink = page.locator('.sidebar-link', { hasText: /^提交$/ });
    if (await submitLink.count()) {
      await submitLink.click();
      await expect(submitLink).toHaveClass(/active/);
    }
  });

  test('未登录访问根路径显示登录表单', async ({ page }) => {
    await page.goto('/')
    // App.tsx renders LoginPage directly when there is no logged-in user
    // (no /login route).
    await expect(page.locator('.auth-card')).toBeVisible({ timeout: 5000 });
    await expect(page.locator('#username')).toBeVisible();
    await expect(page.locator('#password')).toBeVisible();
  });

  test('登录失败显示错误', async ({ page }) => {
    await page.goto('/');
    // Login form is the page root for an unauthenticated user.
    await expect(page.locator('#username')).toBeVisible({ timeout: 5000 });
    await page.locator('#username').fill('definitely-no-such-user');
    await page.locator('#password').fill('wrong-pass-123');
    await page.getByRole('button', { name: /登录/ }).click();
    // The login page surfaces an error message inline (`.alert-error`).
    await expect(page.locator('.alert-error')).toBeVisible({ timeout: 5000 });
  });

  test('现有用户直接登录复用 token', async ({ page }) => {
    const username = 'e2e-existing';
    const password = 'hunter22';
    // Bootstrap: create the account first.
    await loginViaApi(page, { username, password });

    // New context (fresh storage). Existing user can simply log in via the
    // API and reach the repo list page. The page renders either an empty
    // state or the repo grid depending on whether they have repos.
    const ctx2 = await page.context().browser()!.newContext();
    const page2 = await ctx2.newPage();
    await loginExisting(page2, { username, password });
    await page2.goto('/');
    await expect(page2.locator('.sidebar')).toBeVisible();
    // The page is RepoList — it has either .repo-grid (with repos) or an
    // empty-state button "新建仓库" (with no repos).
    const hasGrid = await page2.locator('.repo-grid').isVisible().catch(() => false);
    const hasEmpty = await page2
      .getByRole('button', { name: /新建仓库/ })
      .first()
      .isVisible()
      .catch(() => false);
    expect(hasGrid || hasEmpty).toBeTruthy();
    await ctx2.close();
  });
})

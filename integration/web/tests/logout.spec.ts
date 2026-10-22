import { test, expect } from '@playwright/test'
import { BASE, currentToken, loginViaApi } from './helpers'

/**
 * 退出登录必须真正让服务端失效 token。
 *
 * 只在浏览器里清掉 localStorage 是不够的：JWT 在服务端仍然是有效的，
 * 拿到它的人（在同一个浏览器、另一台设备、或者日志里）还能继续用它访问
 * API 直到自然过期。`POST /api/auth/logout` 把它加入吊销列表，这才是
 * 真正的登出。
 */
test.describe('退出登录', () => {
  test('登出后原 token 在服务端失效', async ({ page }) => {
    const username = `e2e-logout-${Date.now()}`;
    const password = 'hunter22';
    await loginViaApi(page, { username, password });

    // 登录后的 token 可以在服务端换取用户信息。
    const token = await currentToken(page);
    expect(token, '登录后应当拿到 token').toBeTruthy();

    const before = await page.request.get(`${BASE}/api/auth/me`, {
      headers: { Authorization: `Bearer ${token}` },
    });
    expect(before.ok(), '登出前 token 应当有效').toBeTruthy();

    // 点击用户菜单里的“退出登录”。
    await page.goto('/');
    await page.locator('.user-menu-toggle, .avatar, [aria-label*="用户"]').first().click();
    await page.getByRole('button', { name: /退出登录/ }).click();

    // 页面回到登录态。
    await expect(page.locator('#username')).toBeVisible({ timeout: 10000 });

    // 关键断言：同一个 token 在服务端已经被吊销。
    const after = await page.request.get(`${BASE}/api/auth/me`, {
      headers: { Authorization: `Bearer ${token}` },
    });
    expect(
      after.status(),
      `登出后原 token 应当返回 401，实际 ${after.status()}`,
    ).toBe(401);
  });

  test('登出后本地 token 被清除', async ({ page }) => {
    const username = `e2e-logout-local-${Date.now()}`;
    await loginViaApi(page, { username, password: 'hunter22' });

    await page.goto('/');
    await page.locator('.user-menu-toggle, .avatar, [aria-label*="用户"]').first().click();
    await page.getByRole('button', { name: /退出登录/ }).click();

    await expect(page.locator('#username')).toBeVisible({ timeout: 10000 });
    const token = await page.evaluate(() => localStorage.getItem('gita_token'));
    expect(token, '登出后 localStorage 里不应再有 token').toBeNull();
  });
});

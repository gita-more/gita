import { test, expect, type Page } from '@playwright/test'
import { loginViaApi, injectAuth, BASE } from './helpers'

/**
 * VM 集群面板（设置页）。
 *
 * 两个部署事实决定测试能断言什么：
 * - 面板只对 is_admin 渲染；
 * - 服务器没开 [server.vm] 时，admin 看到的是「未启用」提示而不是报错。
 *
 * 两个测试都不依赖具体部署状态：先探测当前账号的身份与服务器配置，
 * 再断言与之对应的行为。
 */

interface Me {
  is_admin: boolean
}

/**
 * Try to become an admin through /api/auth/init — accepted only while the
 * users table is empty, which is exactly the state of a freshly started
 * e2e server. On an already-initialized server it fails and the caller
 * skips the admin-only assertions.
 */
async function initAdmin(page: Page): Promise<boolean> {
  const username = `vm-admin-${Date.now()}`
  const res = await page.request.post(`${BASE}/api/auth/init`, {
    data: {
      username,
      email: `${username}@e2e.local`,
      password: 'testpass123',
    },
  })
  if (!res.ok()) return false
  const body = (await res.json()) as { token: string; user: Me }
  await page.addInitScript(
    ({ token, user }) => {
      localStorage.setItem('gita_token', token)
      localStorage.setItem('gita_user', JSON.stringify(user))
    },
    { token: body.token, user: body.user as unknown as Record<string, unknown> },
  )
  return body.user.is_admin === true
}

async function currentUserIsAdmin(page: Page): Promise<boolean> {
  const token = await page.evaluate(() => localStorage.getItem('gita_token'))
  const res = await page.request.get(`${BASE}/api/auth/me`, {
    headers: token ? { authorization: `Bearer ${token}` } : {},
  })
  if (!res.ok()) return false
  const me: Me = await res.json()
  return me.is_admin === true
}

async function clusterEnabled(page: import('@playwright/test').Page): Promise<boolean> {
  const token = await page.evaluate(() => localStorage.getItem('gita_token'))
  const res = await page.request.get(`${BASE}/api/v1/vm/templates`, {
    headers: token ? { authorization: `Bearer ${token}` } : {},
  })
  // 404 = 集群未启用（路由没注册）
  return res.status() !== 404
}

test.describe('VM 集群面板', () => {
  test('admin 看到 VM 面板；集群未启用时显示提示而不是报错', async ({ page }) => {
    // /api/auth/init 只在用户表为空时接受请求：全新起的 e2e 服务器上
    // 这一步成功且该账号就是管理员；复用的旧库上会失败，此时跳过——
    // admin 视图的细节由组件测试（vitest）覆盖。
    const isAdmin = await initAdmin(page)
    test.skip(!isAdmin, '服务器已初始化，无法通过 init 获得管理员账号')

    // The VM cluster is its own administration page now (/admin/vm), not a
    // section of /settings.
    await page.goto('/admin/vm')
    const section = page.getByTestId('vm-section')
    const disabled = page.getByTestId('vm-section-disabled')
    await expect(section.or(disabled).first()).toBeVisible()

    if (!(await clusterEnabled(page))) {
      // 集群关闭：必须是「未启用」提示，且没有任何表格。
      await expect(disabled).toBeVisible()
      await expect(page.getByTestId('vm-templates')).toHaveCount(0)
    } else {
      // 集群开启：三张表区域存在（允许为空态文案）。
      await expect(section).toBeVisible()
      await expect(section.locator('h4').first()).toBeVisible()
    }
  })
  test('普通用户在管理页看不到 VM 集群面板', async ({ page }) => {
    // 每次运行用全新账号：注册接口不授予管理员，所以这个账号一定是
    // 非 admin，断言「面板不可见」不依赖部署里已有用户的角色。
    const username = `vmnonadmin-${Date.now()}`
    await loginViaApi(page, {
      username,
      password: 'testpass123',
    })

    await page.goto('/admin/vm')
    await expect(page.locator('h3', { hasText: /VM|虚拟机/ })).toHaveCount(0)
    await expect(page.getByTestId('vm-section')).toHaveCount(0)
    await expect(page.getByTestId('vm-section-disabled')).toHaveCount(0)
  })

})

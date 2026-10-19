import { test, expect, type Page } from '@playwright/test'
import {
  loginViaApi,
  createGroup,
  createRepo,
  pageOrigin,
  DEFAULT_USER,
} from './helpers'

/**
 * E2E coverage for EmptyRepoWelcome (web/src/components/EmptyRepoWelcome.tsx).
 *
 * These specs cover the user-facing behavior of the Quick Setup card that
 * appears on the Files tab of a fresh repository: the HTTPS clone URL,
 * the SSH "not configured" placeholder, the substituted command snippets,
 * the copy-to-clipboard button, and the personal-vs-group URL distinction.
 */

test.describe('空仓库 Quick Setup', () => {
  test.beforeEach(async ({ page }) => {
    await loginViaApi(page)
  })

  test('个人仓：HTTPS URL 与命令片段都生效', async ({ page }) => {
    const owner = DEFAULT_USER.username
    const name = `empty-personal-${Date.now()}`
    await createRepo(page, name, { description: 'fresh empty repo for welcome card' })

    await page.goto(`/${owner}/${name}`)

    // 欢迎卡出现
    await expect(page.locator('.quick-setup')).toBeVisible()

    // HTTPS tab 默认 active
    const httpsTab = page.locator('.proto-toggle-btn', { hasText: 'HTTPS' }).first()
    await expect(httpsTab).toHaveClass(/active/)

    // clone URL 输入框值 = <origin>/<owner>/<name>.git
    const origin = await pageOrigin(page)
    const expectedUrl = `${origin}/${owner}/${name}.git`
    await expect(page.locator('.clone-url-input')).toHaveValue(expectedUrl)

    // 两个命令片段都包含完整 URL，且不再含 <REMOTE_URL> 占位符
    const blocks = page.locator('.cmd-block-body code')
    await expect(blocks).toHaveCount(2)
    for (let i = 0; i < 2; i++) {
      const text = await blocks.nth(i).innerText()
      expect(text).toContain(expectedUrl)
      expect(text).not.toContain('<REMOTE_URL>')
    }
  })

  test('SSH tab：URL 输入框清空、复制按钮 disabled、显示未配置提示', async ({ page }) => {
    const owner = DEFAULT_USER.username
    const name = `empty-ssh-${Date.now()}`
    await createRepo(page, name)

    await page.goto(`/${owner}/${name}`)
    await expect(page.locator('.quick-setup')).toBeVisible()

    // 切到 SSH
    await page.locator('.proto-toggle-btn', { hasText: 'SSH' }).first().click()

    // SSH tab 激活、HTTPS 失活
    await expect(page.locator('.proto-toggle-btn.active', { hasText: 'SSH' })).toHaveCount(1)
    await expect(page.locator('.proto-toggle-btn.active', { hasText: 'HTTPS' })).toHaveCount(0)

    // URL 输入框在 SSH 模式下应清空（EmptyRepoWelcome.tsx:proto === 'ssh' 分支）
    await expect(page.locator('.clone-url-input')).toHaveValue('')

    // 复制按钮 disabled
    const copyBtn = page.locator('.clone-url').getByRole('button')
    await expect(copyBtn).toBeDisabled()

    // 提示 alert 出现（"SSH is not configured yet" / "SSH 暂未配置"）
    await expect(
      page.locator('.alert', { hasText: /SSH/i }).first(),
    ).toBeVisible()
  })

  test('复制按钮：点击后变 "已复制!" / "Copied!" 并在 ~1.5s 后还原', async ({ page, context }) => {
    await context.grantPermissions(['clipboard-read', 'clipboard-write'])
    const owner = DEFAULT_USER.username
    const name = `empty-copy-${Date.now()}`
    await createRepo(page, name)

    await page.goto(`/${owner}/${name}`)
    const copyBtn = page.locator('.clone-url').getByRole('button')
    await expect(copyBtn).toBeEnabled()

    await copyBtn.click()
    // 点击后文案变化（zh: 已复制!；en: Copied!）
    await expect(copyBtn).toHaveText(/复制|Copied/i)

    // ~1.5s 后还原
    await expect(copyBtn).toHaveText(/复制|Copy/i, { timeout: 3000 })
  })

  test('群组仓：clone URL 形态是 /groups/<g>/<r>.git 而不是 /<owner>/<r>.git', async ({ page }) => {
    const groupName = `g-${Date.now()}`
    const repoName = `grepo-w`
    await createGroup(page, groupName, 'group for empty repo URL test')
    await createRepo(page, repoName, { group: groupName })

    await page.goto(`/groups/${groupName}/${repoName}`)
    await expect(page.locator('.quick-setup')).toBeVisible()

    const origin = await pageOrigin(page)
    // 注意：不是 /<owner>/<repository>.git，而是 /groups/<group>/<repository>.git
    const expected = `${origin}/groups/${groupName}/${repoName}.git`
    await expect(page.locator('.clone-url-input')).toHaveValue(expected)
  })
})
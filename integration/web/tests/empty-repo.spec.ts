import { test, expect, type Page } from '@playwright/test'
import { execFileSync } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import {
  loginViaApi,
  createGroup,
  createRepo,
  currentToken,
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
    // Git smart-HTTP is mounted at the root path, so the clone URL carries
    // no /git prefix — matching GitLab/GitHub.
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

  /**
   * The clone URL the UI shows must actually work.
   *
   * The previous specs only asserted the *shape* of the URL, so a URL that
   * pointed at the SPA fallback instead of the git endpoint passed the
   * whole suite while `git clone` failed for every user. This spec runs a
   * real `git clone` against the URL the UI displays.
   */
  test('个人仓：UI 显示的 clone URL 真的能被 git clone 使用', async ({ page }) => {
    const owner = DEFAULT_USER.username
    const name = `clone-usable-${Date.now()}`
    await createRepo(page, name, { description: 'clone URL usability check' })

    await page.goto(`/${owner}/${name}`)
    await expect(page.locator('.quick-setup')).toBeVisible()

    const shown = await page.locator('.clone-url-input').inputValue()
    // Clean clone URL: http://host/owner/repo.git (no /git prefix)
    expect(shown).toMatch(/\.git$/)

    // Clone the (empty) repository with the token as the Basic-auth
    // password, the way a user would after pasting their PAT into the
    // remote URL. Both the anonymous and the authenticated form must work,
    // so clone anonymously first and then with credentials.
    //
    // The protocol is kept as-is (the deployment may be plain HTTP) and
    // only the credentials are inserted.
    //
    // `git clone <url> <dir>` requires the destination to *not* exist, so
    // the target is a fresh name inside a temporary directory.
    const token = await currentToken(page)
    const authed = shown.replace(/^(https?):\/\//, `$1://x-access-token:${token}@`)
    const parent = mkdtempSync(join(tmpdir(), 'gita-clone-'))
    try {
      execFileSync('git', ['clone', '--quiet', shown, join(parent, 'repo')], {
        stdio: 'pipe',
        timeout: 30_000,
      })
    } catch (err) {
      throw new Error(
        `git clone failed for the URL the UI showed (${shown}): ${String(err)}`,
      )
    }

    // The same URL with credentials embedded must work too.
    try {
      execFileSync('git', ['clone', '--quiet', authed, join(parent, 'repo-authed')], {
        stdio: 'pipe',
        timeout: 30_000,
      })
    } catch (err) {
      throw new Error(
        `git clone with credentials failed for the URL the UI showed (${shown}): ${String(err)}`,
      )
    } finally {
      rmSync(parent, { recursive: true, force: true })
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

  test('群组仓：clone URL 形态是 /<g>/<r>.git 而不是 /<owner>/<r>.git', async ({ page }) => {
    const groupName = `g-${Date.now()}`
    const repoName = `grepo-w`
    await createGroup(page, groupName, 'group for empty repo URL test')
    await createRepo(page, repoName, { group: groupName })

    await page.goto(`/${groupName}/${repoName}`)
    await expect(page.locator('.quick-setup')).toBeVisible()

    const origin = await pageOrigin(page)
    // A group's repositories are addressed by the group's own name (the
    // namespace segment is the group name, not the literal word "groups").
    const expected = `${origin}/${groupName}/${repoName}.git`
    await expect(page.locator('.clone-url-input')).toHaveValue(expected)
  })
})
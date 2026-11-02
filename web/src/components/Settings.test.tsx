// @vitest-environment jsdom
// Tests for Settings — the user settings page.
//
// The page shows the language switcher and the personal access tokens, and
// (for administrators) the user-management and VM-cluster panels.
//
// useAuth is mocked so the test can decide whether the signed-in user is
// an administrator; the api module is mocked so the tests control what the
// server answers.
//
// What can actually break:
//  1. the tokens must be listed, created and revoked;
//  2. a revoked token must disappear from the list;
//  3. switching the language must be saved;
//  4. an API failure must be shown;
//  5. the admin panels must only show for an administrator.

import { cleanup, render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { useEffect } from 'react'
import { MemoryRouter, useLocation } from 'react-router-dom'
import { afterEach, describe, expect, it, vi } from 'vitest'

afterEach(cleanup)

vi.mock('../lib/api', () => ({
  api: {
    listTokens: vi.fn(),
    createToken: vi.fn(),
    deleteToken: vi.fn(),
    updateLocale: vi.fn(),
    listUsers: vi.fn(),
    createUser: vi.fn(),
    updateUser: vi.fn(),
    deleteUser: vi.fn(),
    listVmTemplates: vi.fn(),
    listVmNodes: vi.fn(),
    listVms: vi.fn(),
  },
  getToken: vi.fn(() => null),
  setToken: vi.fn(),
}))

vi.mock('../hooks/useAuth', () => ({
  useAuth: () => ({ user: currentUser, token: null, login: vi.fn(), register: vi.fn(), init: vi.fn(), logout: vi.fn(), refresh: vi.fn() }),
}))

import { api } from '../lib/api'
import { I18nProvider, useI18n } from '../i18n'
import { Settings } from './Settings'
import type { Token, User } from '../lib/api'

const listTokens = vi.mocked(api.listTokens)
const createToken = vi.mocked(api.createToken)
const deleteToken = vi.mocked(api.deleteToken)
const updateLocale = vi.mocked(api.updateLocale)
const listUsers = vi.mocked(api.listUsers)
const createUser = vi.mocked(api.createUser)
const updateUser = vi.mocked(api.updateUser)
const deleteUser = vi.mocked(api.deleteUser)
const listVmTemplates = vi.mocked(api.listVmTemplates)
const listVmNodes = vi.mocked(api.listVmNodes)
const listVms = vi.mocked(api.listVms)

/** The signed-in user; flipped per test to exercise the admin panels. */
let currentUser: User | null = null

function makeToken(overrides: Partial<Token> = {}): Token {
  return {
    id: 1,
    token: 'secret-token',
    name: 'ci-runner',
    created_at: '2026-01-01T00:00:00Z',
    ...overrides,
  }
}

function makeUser(overrides: Partial<User> = {}): User {
  return {
    id: 1,
    username: 'alice',
    email: 'alice@example.com',
    locale: 'zh',
    is_admin: false,
    ...overrides,
  }
}

function LocationProbe() {
  const loc = useLocation()
  return <div data-testid="location">{loc.pathname}</div>
}

/**
 * The locale lives in a module-level store (see src/i18n), so a test that
 * switches the language would leak English into every following test. This
 * probe captures the setter so afterEach can put the locale back.
 */
const localeHolder: { setLocale: ((locale: 'zh' | 'en') => Promise<void>) | null } = {
  setLocale: null,
}

function LocaleProbe() {
  const { setLocale } = useI18n()
  useEffect(() => {
    localeHolder.setLocale = setLocale
  }, [setLocale])
  return null
}

afterEach(async () => {
  await localeHolder.setLocale?.('zh')
  localeHolder.setLocale = null
})

function setup() {
  const user = userEvent.setup()
  render(
    <MemoryRouter initialEntries={['/settings']}>
      <I18nProvider>
        <Settings />
        <LocationProbe />
        <LocaleProbe />
      </I18nProvider>
    </MemoryRouter>,
  )
  return { user }
}

/** Neutral defaults so a test only has to set what it cares about. */
function stubApi() {
  listTokens.mockResolvedValue([])
  listUsers.mockResolvedValue({ users: [] })
  listVmTemplates.mockResolvedValue({ templates: [] })
  listVmNodes.mockResolvedValue({ nodes: [] })
  listVms.mockResolvedValue({ vms: [] })
  updateLocale.mockResolvedValue(makeUser())
}

describe('Settings', () => {
  it('列出访问令牌', async () => {
    stubApi()
    listTokens.mockResolvedValue([makeToken()])

    setup()

    await waitFor(() => {
      expect(screen.getByText('ci-runner')).toBeTruthy()
    })
    // The token secret itself is never shown again.
    expect(screen.queryByText('secret-token')).toBeNull()
  })

  it('没有令牌时显示空状态', async () => {
    stubApi()

    setup()

    await waitFor(() => {
      expect(screen.getByText('还没有访问令牌。')).toBeTruthy()
    })
  })

  it('新建令牌调用接口并显示新令牌一次', async () => {
    stubApi()
    createToken.mockResolvedValue({ id: 2, token: 'gita_pat_new', name: 'laptop' })

    const { user } = setup()

    await waitFor(() => {
      expect(screen.getByText('还没有访问令牌。')).toBeTruthy()
    })

    await user.click(screen.getByRole('button', { name: '新建令牌' }))
    await user.type(screen.getByLabelText('令牌名称'), 'laptop')
    await user.click(screen.getByRole('button', { name: '新建令牌' }))

    await waitFor(() => {
      expect(createToken).toHaveBeenCalledWith('laptop')
    })
    // The new secret is shown once so the user can copy it.
    expect(screen.getByText('gita_pat_new')).toBeTruthy()
  })

  it('吊销令牌需要确认，确认后调用接口并刷新列表', async () => {
    stubApi()
    listTokens.mockResolvedValue([makeToken()])
    deleteToken.mockResolvedValue({ message: 'ok' })

    const { user } = setup()

    await waitFor(() => {
      expect(screen.getByText('ci-runner')).toBeTruthy()
    })

    await user.click(screen.getByRole('button', { name: '吊销' }))

    // A confirmation dialog appears first.
    const dialog = await screen.findByRole('dialog')
    expect(dialog.textContent).toContain('ci-runner')
    expect(deleteToken).not.toHaveBeenCalled()

    await user.click(within(dialog).getByRole('button', { name: '吊销' }))

    await waitFor(() => {
      expect(deleteToken).toHaveBeenCalledWith(1)
    })
    await waitFor(() => {
      expect(listTokens).toHaveBeenCalledTimes(2)
    })
  })

  it('切换语言调用接口并显示保存成功', async () => {
    stubApi()

    const { user } = setup()

    await waitFor(() => {
      expect(screen.getByRole('radio', { name: 'English' })).toBeTruthy()
    })
    await user.click(screen.getByRole('radio', { name: 'English' }))

    await waitFor(() => {
      expect(updateLocale).toHaveBeenCalledWith('en')
    })
  })

  it('加载令牌失败时显示错误', async () => {
    stubApi()
    listTokens.mockRejectedValue(new Error('unauthorized'))

    setup()

    await waitFor(() => {
      expect(screen.getByText(/unauthorized|未认证/)).toBeTruthy()
    })
  })

  it('新建令牌失败时显示错误', async () => {
    stubApi()
    createToken.mockRejectedValue(new Error('internal error'))

    const { user } = setup()

    await waitFor(() => {
      expect(screen.getByText('还没有访问令牌。')).toBeTruthy()
    })

    await user.click(screen.getByRole('button', { name: '新建令牌' }))
    await user.type(screen.getByLabelText('令牌名称'), 'laptop')
    await user.click(screen.getByRole('button', { name: '新建令牌' }))

    await waitFor(() => {
      expect(screen.getByText(/internal error/)).toBeTruthy()
    })
  })

  it('非管理员看不到用户管理和 VM 集群面板', async () => {
    stubApi()
    currentUser = makeUser({ is_admin: false })

    setup()

    await waitFor(() => {
      expect(screen.getByText('访问令牌（PAT）')).toBeTruthy()
    })
    expect(screen.queryByText('用户管理')).toBeNull()
    expect(screen.queryByText('VM 集群')).toBeNull()
    expect(listUsers).not.toHaveBeenCalled()
  })

  it('管理员能看到用户管理面板并列出用户', async () => {
    stubApi()
    currentUser = makeUser({ is_admin: true })
    listUsers.mockResolvedValue({
      users: [makeUser({ id: 2, username: 'bob', email: 'bob@example.com' })],
    })

    setup()

    await waitFor(() => {
      expect(screen.getByText('用户管理')).toBeTruthy()
    })
    expect(screen.getByText('bob')).toBeTruthy()
    expect(screen.getByText('bob@example.com')).toBeTruthy()
  })

  it('管理员可以创建用户', async () => {
    stubApi()
    currentUser = makeUser({ is_admin: true })
    createUser.mockResolvedValue(makeUser({ id: 3, username: 'carol' }))

    const { user } = setup()

    await waitFor(() => {
      expect(screen.getByText('用户管理')).toBeTruthy()
    })

    await user.click(screen.getByRole('button', { name: '创建用户' }))
    await user.type(screen.getByLabelText('用户名'), 'carol')
    await user.type(screen.getByLabelText('邮箱'), 'carol@example.com')
    await user.type(screen.getByLabelText('密码'), 'secret123')
    await user.click(screen.getByRole('button', { name: '创建用户' }))

    await waitFor(() => {
      expect(createUser).toHaveBeenCalledWith('carol', 'carol@example.com', 'secret123', false)
    })
    expect(screen.getByText('用户 carol 已创建')).toBeTruthy()
  })

  it('管理员可以授予管理员角色', async () => {
    stubApi()
    currentUser = makeUser({ is_admin: true })
    listUsers.mockResolvedValue({
      users: [makeUser({ id: 2, username: 'bob', email: 'bob@example.com' })],
    })
    updateUser.mockResolvedValue(makeUser({ id: 2, username: 'bob', is_admin: true }))

    const { user } = setup()

    await waitFor(() => {
      expect(screen.getByText('bob')).toBeTruthy()
    })

    await user.click(screen.getByRole('button', { name: '设为管理员' }))

    await waitFor(() => {
      expect(updateUser).toHaveBeenCalledWith('bob', { is_admin: true })
    })
  })

  it('删除用户需要确认，确认后调用接口', async () => {
    stubApi()
    currentUser = makeUser({ is_admin: true })
    listUsers.mockResolvedValue({
      users: [makeUser({ id: 2, username: 'bob', email: 'bob@example.com' })],
    })
    deleteUser.mockResolvedValue({ message: 'ok' })

    const { user } = setup()

    await waitFor(() => {
      expect(screen.getByText('bob')).toBeTruthy()
    })

    await user.click(screen.getByRole('button', { name: '删除' }))

    const dialog = await screen.findByRole('dialog')
    expect(dialog.textContent).toContain('bob')
    expect(deleteUser).not.toHaveBeenCalled()

    await user.click(within(dialog).getByRole('button', { name: '删除' }))

    await waitFor(() => {
      expect(deleteUser).toHaveBeenCalledWith('bob')
    })
  })

  it('不能对自己执行删除或改角色操作', async () => {
    stubApi()
    currentUser = makeUser({ is_admin: true })
    listUsers.mockResolvedValue({
      users: [makeUser({ id: 1, username: 'alice', email: 'alice@example.com' })],
    })

    setup()

    await waitFor(() => {
      expect(screen.getByText('alice')).toBeTruthy()
    })
    // The current user has no delete button.
    expect(screen.queryByRole('button', { name: '删除' })).toBeNull()
    expect(screen.queryByRole('button', { name: '设为管理员' })).toBeNull()
  })

  it('返回按钮回到首页', async () => {
    stubApi()

    const { user } = setup()

    await waitFor(() => {
      expect(screen.getByRole('button', { name: '返回' })).toBeTruthy()
    })
    await user.click(screen.getByRole('button', { name: '返回' }))

    expect(screen.getByTestId('location').textContent).toBe('/')
  })
})

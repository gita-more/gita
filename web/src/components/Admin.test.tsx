// Tests for the Administration pages — one page per section.
//
// /admin/users shows user management; /admin/vm shows the VM cluster.
//
// useAuth is mocked so the test can decide whether the signed-in user is
// an administrator; the api module is mocked so the tests control what the
// server answers.
//
// What can actually break:
//  1. non-administrators cannot reach the pages;
//  2. users must be listed, created, granted admin role and deleted;
//  3. VM templates / nodes / instances must render (and the "not enabled"
//     notice shows when the server returns 404).

import { cleanup, render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { useEffect } from 'react'
import { MemoryRouter, useLocation } from 'react-router-dom'
import { afterEach, describe, expect, it, vi } from 'vitest'


vi.mock('../lib/api', () => ({
  api: {
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
  useAuth: () => ({ user: currentUser, token: null, login: vi.fn(), register: vi.fn(), init: vi.fn(), logout: vi.fn(), refresh: vi.fn(), sync: vi.fn() }),
}))

import { api } from '../lib/api'
import { I18nProvider, useI18n } from '../i18n'
import { AdminUsersPage, AdminVmPage } from './Admin'
import type { User } from '../lib/api'

const listUsers = vi.mocked(api.listUsers)
const createUser = vi.mocked(api.createUser)
const updateUser = vi.mocked(api.updateUser)
const deleteUser = vi.mocked(api.deleteUser)
const listVmTemplates = vi.mocked(api.listVmTemplates)
const listVmNodes = vi.mocked(api.listVmNodes)
const listVms = vi.mocked(api.listVms)

/** The signed-in user; flipped per test to exercise the admin pages. */
let currentUser: User | null = null

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

function setupUsers() {
  const user = userEvent.setup()
  render(
    <MemoryRouter initialEntries={['/admin/users']}>
      <I18nProvider>
        <AdminUsersPage />
        <LocationProbe />
        <LocaleProbe />
      </I18nProvider>
    </MemoryRouter>,
  )
  return { user }
}

function setupVm() {
  const user = userEvent.setup()
  render(
    <MemoryRouter initialEntries={['/admin/vm']}>
      <I18nProvider>
        <AdminVmPage />
        <LocationProbe />
        <LocaleProbe />
      </I18nProvider>
    </MemoryRouter>,
  )
  return { user }
}

/** Neutral defaults: empty user list, empty VM cluster. */
function stubApi() {
  listUsers.mockResolvedValue({ users: [] })
  listVmTemplates.mockResolvedValue({ templates: [] })
  listVmNodes.mockResolvedValue({ nodes: [] })
  listVms.mockResolvedValue({ vms: [] })
}

describe('AdminUsersPage', () => {
  it('非管理员不应看到用户管理内容', async () => {
    stubApi()
    currentUser = makeUser({ is_admin: false })

    setupUsers()

    await waitFor(() => {
      expect(screen.queryByText('用户管理')).toBeNull()
    })
    expect(listUsers).not.toHaveBeenCalled()
  })

  it('管理员能看到用户管理面板并列出用户', async () => {
    stubApi()
    currentUser = makeUser({ is_admin: true })
    listUsers.mockResolvedValue({
      users: [makeUser({ id: 2, username: 'bob', email: 'bob@example.com' })],
    })

    setupUsers()

    await waitFor(() => {
      expect(screen.getByText('用户管理')).toBeTruthy()
    })
    await waitFor(() => {
      expect(screen.getByText('bob')).toBeTruthy()
    })
    expect(screen.getByText('bob@example.com')).toBeTruthy()
  })

  it('管理员可以创建用户', async () => {
    stubApi()
    currentUser = makeUser({ is_admin: true })
    createUser.mockResolvedValue(makeUser({ id: 3, username: 'carol' }))

    const { user } = setupUsers()

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

    const { user } = setupUsers()

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

    const { user } = setupUsers()

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

    setupUsers()

    await waitFor(() => {
      expect(screen.getByText('alice')).toBeTruthy()
    })
    // The current user has no delete button.
    expect(screen.queryByRole('button', { name: '删除' })).toBeNull()
    expect(screen.queryByRole('button', { name: '设为管理员' })).toBeNull()
  })
})

describe('AdminVmPage', () => {
  it('VM 集群面板：未启用时只显示提示', async () => {
    stubApi()
    currentUser = makeUser({ is_admin: true })
    // 404 = 集群未启用。
    listVmTemplates.mockRejectedValue(Object.assign(new Error('not found'), { status: 404 }))
    listVmNodes.mockRejectedValue(Object.assign(new Error('not found'), { status: 404 }))
    listVms.mockRejectedValue(Object.assign(new Error('not found'), { status: 404 }))

    setupVm()

    await waitFor(() => {
      expect(screen.getByText('VM 集群')).toBeTruthy()
    })
    expect(screen.getByText(/VM 集群未启用/)).toBeTruthy()
    expect(screen.queryByTestId('vm-templates')).toBeNull()
  })

  it('VM 集群面板：启用后显示模板、节点、实例三块', async () => {
    stubApi()
    currentUser = makeUser({ is_admin: true })
    listVmTemplates.mockResolvedValue({ templates: [] })
    listVmNodes.mockResolvedValue({ nodes: [] })
    listVms.mockResolvedValue({ vms: [] })

    setupVm()

    await waitFor(() => {
      expect(screen.getByText('VM 集群')).toBeTruthy()
    })
    expect(screen.getByText('模板')).toBeTruthy()
    expect(screen.getByText('节点')).toBeTruthy()
    expect(screen.getByText('实例')).toBeTruthy()
  })

  it('非管理员不应看到 VM 集群内容', async () => {
    stubApi()
    currentUser = makeUser({ is_admin: false })

    setupVm()

    await waitFor(() => {
      expect(screen.queryByText('VM 集群')).toBeNull()
    })
  })
})

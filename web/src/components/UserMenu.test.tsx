// Tests for the topbar user menu.
//
// What can actually break:
//  1. the administrator entry must appear only for administrators;
//  2. the menu order must be Settings → Administration → Sign out, so the
//     admin entry sits directly above "sign out";
//  3. the admin entry must not reuse the settings gear icon — the two
//     entries lead to different pages and must be told apart at a glance.

import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, describe, expect, it, vi } from 'vitest'

vi.mock('../lib/api', () => ({
  api: {
    logout: vi.fn(),
    search: vi.fn(),
  },
  getToken: vi.fn(() => null),
  setToken: vi.fn(),
}))

vi.mock('../hooks/useAuth', () => ({
  useAuth: () => ({ user: currentUser, token: null, login: vi.fn(), register: vi.fn(), init: vi.fn(), logout: vi.fn(), refresh: vi.fn(), sync: vi.fn() }),
}))

import { Topbar } from './Layout'
import { I18nProvider } from '../i18n'
import type { User } from '../lib/api'

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


function setup() {
  const user = userEvent.setup()
  render(
    <MemoryRouter>
      <I18nProvider>
        <Topbar />
      </I18nProvider>
    </MemoryRouter>,
  )
  return { user }
}

/** Open the user menu (the avatar button in the topbar). */
async function openMenu(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByRole('button', { name: '用户菜单' }))
  await waitFor(() => {
    expect(screen.getByRole('menu')).toBeTruthy()
  })
}

describe('user menu', () => {
  it('普通用户看不到管理入口', async () => {
    currentUser = makeUser({ is_admin: false })
    const { user } = setup()

    await openMenu(user)

    expect(screen.queryByRole('link', { name: '管理' })).toBeNull()
    expect(screen.getByRole('link', { name: '设置' })).toBeTruthy()
  })

  it('管理员能看到管理入口', async () => {
    currentUser = makeUser({ is_admin: true })
    const { user } = setup()

    await openMenu(user)

    expect(screen.getByRole('link', { name: '管理' })).toBeTruthy()
  })

  it('菜单顺序是设置 → 管理 → 退出登录', async () => {
    currentUser = makeUser({ is_admin: true })
    const { user } = setup()

    await openMenu(user)

    const items = screen.getAllByRole('listitem').map((li) => li.textContent)
    expect(items).toEqual(['设置', '管理', '退出登录'])
  })

  it('管理入口的图标与设置不同', async () => {
    currentUser = makeUser({ is_admin: true })
    const { user } = setup()

    await openMenu(user)

    const settingsIcon = screen.getByRole('link', { name: '设置' }).querySelector('svg')
    const adminIcon = screen.getByRole('link', { name: '管理' }).querySelector('svg')
    // The gear and the shield are different shapes; compare their paths.
    expect(settingsIcon?.innerHTML).not.toBe(adminIcon?.innerHTML)
  })
})

// @vitest-environment jsdom
// Tests for the topbar breadcrumbs.
//
// What can actually break:
//  1. a global page must show its localized name, not the raw URL segment
//     ("Settings", not "settings");
//  2. the entry must link to the page it names;
//  3. a group path (e.g. /gita) is a name, not a route — it must be shown
//     verbatim;
//  4. a repo breadcrumb must keep pointing at the namespace and the repo.

import { cleanup, render, screen } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, describe, expect, it, vi } from 'vitest'

vi.mock('../lib/api', () => ({
  api: { search: vi.fn() },
  getToken: vi.fn(() => null),
  setToken: vi.fn(),
}))

vi.mock('../hooks/useAuth', () => ({
  useAuth: () => ({ user: null, token: null, login: vi.fn(), register: vi.fn(), init: vi.fn(), logout: vi.fn(), refresh: vi.fn(), sync: vi.fn() }),
}))

import { Topbar } from './Layout'
import { I18nProvider } from '../i18n'
import type { Crumb } from './Layout'

afterEach(cleanup)

function setup(crumbs: Crumb[]) {
  render(
    <MemoryRouter>
      <I18nProvider>
        <Topbar crumbs={crumbs} />
      </I18nProvider>
    </MemoryRouter>,
  )
}

describe('breadcrumbs', () => {
  it('显示本地化名称而不是原始路径段', () => {
    setup([{ label: '设置', path: '/settings' }])

    expect(screen.getByText('设置')).toBeTruthy()
    expect(screen.queryByText('settings')).toBeNull()
  })

  it('入口链接到它所命名的页面', () => {
    setup([
      { label: '群组', path: '/groups' },
      { label: '设置', path: '/settings' },
    ])

    expect(screen.getByRole('link', { name: '群组' }).getAttribute('href')).toBe('/groups')
    // The last entry is the current page: no link, just the label.
    expect(screen.getByText('设置').getAttribute('href')).toBeNull()
  })

  it('群组路径原样显示', () => {
    setup([{ label: 'gita', path: '/gita' }])

    expect(screen.getByText('gita')).toBeTruthy()
  })

  it('仓库面包屑保留命名空间与仓库两段', () => {
    setup([
      { label: 'gita', path: '/gita' },
      { label: 'tests', path: '/gita/tests' },
    ])

    expect(screen.getByRole('link', { name: 'gita' }).getAttribute('href')).toBe('/gita')
    expect(screen.getByText('tests')).toBeTruthy()
  })
})

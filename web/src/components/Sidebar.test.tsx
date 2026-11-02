// @vitest-environment jsdom
// Tests for Sidebar page-local navigation sections.
//
// What can actually break:
//  1. each section renders its group label + items with localized labels;
//  2. each item links to its own page (/settings/language, …);
//  3. the item whose path the browser is on is highlighted.

import { cleanup, render, screen } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, describe, expect, it, vi } from 'vitest'

vi.mock('../hooks/useAuth', () => ({
  useAuth: () => ({ user: null, token: null, login: vi.fn(), register: vi.fn(), init: vi.fn(), logout: vi.fn(), refresh: vi.fn(), sync: vi.fn() }),
}))

import { Sidebar } from './Layout'
import type { SidebarSection } from './Layout'
import { SettingsIcon, GlobeIcon, CodeIcon } from './Icons'
import { I18nProvider } from '../i18n'

afterEach(cleanup)

function makeSections(): SidebarSection[] {
  return [
    {
      groupLabelKey: 'common.settings',
      icon: SettingsIcon,
      items: [
        { key: 'language', labelKey: 'settings.language', icon: GlobeIcon },
        { key: 'tokens', labelKey: 'settings.tokensTitle', icon: CodeIcon },
      ],
    },
  ]
}

function setup(sections: SidebarSection[], path = '/settings/language') {
  render(
    <MemoryRouter initialEntries={[path]}>
      <I18nProvider>
        <Sidebar variant="global" sections={sections} />
      </I18nProvider>
    </MemoryRouter>,
  )
}

describe('Sidebar page-local navigation', () => {
  it('renders each section with group label and items', () => {
    setup(makeSections())

    expect(screen.getByText('设置')).toBeTruthy() // group label
    expect(screen.getByRole('link', { name: /语言/ })).toBeTruthy()
    expect(screen.getByRole('link', { name: /访问令牌/ })).toBeTruthy()
  })

  it('每个菜单项链接到自己的页面', () => {
    setup(makeSections())

    expect(screen.getByRole('link', { name: /语言/ }).getAttribute('href')).toBe(
      '/settings/language',
    )
    expect(screen.getByRole('link', { name: /访问令牌/ }).getAttribute('href')).toBe(
      '/settings/tokens',
    )
  })

  it('高亮当前路径对应的项', () => {
    setup(makeSections(), '/settings/tokens')

    expect(screen.getByRole('link', { name: /访问令牌/ }).className).toContain('active')
    expect(screen.getByRole('link', { name: /语言/ }).className).not.toContain('active')
  })

  it('管理页的菜单项链接到自己的页面', () => {
    const sections: SidebarSection[] = [
      {
        groupLabelKey: 'nav.admin',
        icon: SettingsIcon,
        items: [
          { key: 'users', labelKey: 'admin.title', icon: GlobeIcon },
          { key: 'vm', labelKey: 'vm.title', icon: CodeIcon },
        ],
      },
    ]
    setup(sections, '/admin/vm')

    expect(screen.getByText('管理')).toBeTruthy()
    expect(screen.getByRole('link', { name: /用户管理/ }).getAttribute('href')).toBe(
      '/admin/users',
    )
    expect(screen.getByRole('link', { name: /VM 集群/ }).getAttribute('href')).toBe(
      '/admin/vm',
    )
    expect(screen.getByRole('link', { name: /VM 集群/ }).className).toContain('active')
  })
})

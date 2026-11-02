// @vitest-environment jsdom
// Tests for the global-page breadcrumb mapping (src/App.tsx).
//
// What can actually break:
//  1. the known top-level routes must map to their localized names;
//  2. each entry must link to the URL it lives at;
//  3. an unknown segment is a group path — a name, not a route — and must
//     be shown verbatim;
//  4. a page with sub-sections shows two levels (section + section item),
//     each carrying the icon of the page it points at.

import { describe, expect, it } from 'vitest'
import { globalCrumbs } from './App'
import { messages } from './i18n/en'
import { CircleDotIcon, FolderIcon, SettingsIcon, ShieldIcon } from './components/Icons'

/** The English catalog, used as the translation function. */
const t = (key: string): string =>
  key.split('.').reduce<unknown>((acc, part) => {
    if (acc && typeof acc === 'object' && part in (acc as Record<string, unknown>)) {
      return (acc as Record<string, unknown>)[part]
    }
    return undefined
  }, messages) as string

describe('globalCrumbs', () => {
  it('设置页显示本地化名称和自己的图标', () => {
    expect(globalCrumbs(['settings'], t)).toEqual([
      { label: 'Settings', path: '/settings', icon: SettingsIcon },
    ])
  })

  it('管理页显示本地化名称和自己的图标', () => {
    expect(globalCrumbs(['admin'], t)).toEqual([
      { label: 'Administration', path: '/admin', icon: ShieldIcon },
    ])
  })

  it('群组和 issue 页面显示本地化名称和自己的图标', () => {
    expect(globalCrumbs(['groups'], t)).toEqual([
      { label: 'Groups', path: '/groups', icon: FolderIcon },
    ])
    expect(globalCrumbs(['issues'], t)).toEqual([
      { label: 'Issues', path: '/issues', icon: CircleDotIcon },
    ])
  })

  it('未知路径段原样显示（群组名）', () => {
    expect(globalCrumbs(['gita'], t)).toEqual([
      { label: 'gita', path: '/gita', icon: undefined },
    ])
  })

  it('多段路径逐级累积链接', () => {
    expect(globalCrumbs(['gita', 'tests'], t)).toEqual([
      { label: 'gita', path: '/gita', icon: undefined },
      { label: 'tests', path: '/gita/tests', icon: undefined },
    ])
  })

  it('设置子页面显示两级面包屑：设置 > 语言', () => {
    const crumbs = globalCrumbs(['settings', 'language'], t)
    expect(crumbs.map((c) => c.label)).toEqual(['Settings', 'Language'])
    expect(crumbs.map((c) => c.path)).toEqual(['/settings/language', '/settings/language'])
    // 只有一级带图标，二级不带。
    expect(crumbs[0].icon).toBe(SettingsIcon)
    expect(crumbs[1].icon).toBeUndefined()
  })

  it('管理子页面显示两级面包屑：管理 > 用户管理', () => {
    const crumbs = globalCrumbs(['admin', 'users'], t)
    expect(crumbs.map((c) => c.label)).toEqual(['Administration', 'User administration'])
    expect(crumbs[0].path).toBe('/admin/users')
    expect(crumbs[1].path).toBe('/admin/users')
  })

  it('未知的子页面段只显示一级', () => {
    const crumbs = globalCrumbs(['settings', 'nope'], t)
    expect(crumbs.map((c) => c.label)).toEqual(['Settings'])
  })

  it('群组详情显示两级面包屑：群组 > 群组名', () => {
    const crumbs = globalCrumbs(['groups', 'gita'], t)
    expect(crumbs.map((c) => c.label)).toEqual(['Groups', 'gita'])
    expect(crumbs[0].path).toBe('/groups')
    expect(crumbs[1].path).toBe('/groups/gita')
    // 只有一级带图标。
    expect(crumbs[0].icon).toBe(FolderIcon)
    expect(crumbs[1].icon).toBeUndefined()
  })

  it('嵌套群组的多段路径合成一级面包屑', () => {
    const crumbs = globalCrumbs(['groups', 'gita', 'tests'], t)
    expect(crumbs.map((c) => c.label)).toEqual(['Groups', 'gita', 'tests'])
    expect(crumbs.map((c) => c.path)).toEqual(['/groups', '/groups/gita', '/groups/gita/tests'])
  })
})

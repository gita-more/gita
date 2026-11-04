// Tests for BranchSelector — the GitHub-style "switch branches/tags" popover.
//
// What can actually break:
//  1. the popover must open, filter and close like a popover (Escape, click
//     outside);
//  2. the branches/tags tabs must show different lists;
//  3. picking a ref must call onSelect with exactly that ref and close;
//  4. the default branch must be marked;
//  5. "view all" must navigate to the settings section of the active tab.
//
// The component uses useNavigate, so it needs a router; a small location
// probe captures where it navigated to.

import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { MemoryRouter, useLocation } from 'react-router-dom'
import { afterEach, describe, expect, it, vi } from 'vitest'


import { I18nProvider } from '../i18n'
import { BranchSelector } from './BranchSelector'

function LocationProbe() {
  const loc = useLocation()
  return <div data-testid="location">{loc.pathname}</div>
}

function setup(props: Partial<React.ComponentProps<typeof BranchSelector>> = {}) {
  const user = userEvent.setup()
  const onSelect = props.onSelect ?? vi.fn()
  render(
    <MemoryRouter initialEntries={['/alice/widgets']}>
      <I18nProvider>
        <BranchSelector
          owner="alice"
          name="widgets"
          rev="main"
          branches={['main', 'feature/login', 'feature/logout']}
          tags={['v1.0.0', 'v1.1.0']}
          defaultBranch="main"
          onSelect={onSelect}
          {...props}
        />
        <LocationProbe />
      </I18nProvider>
    </MemoryRouter>,
  )
  return { user, onSelect }
}

const panel = () => screen.queryByRole('dialog')

describe('BranchSelector', () => {
  it('触发按钮显示当前 ref，点击后展开面板', async () => {
    const { user } = setup()

    expect(panel()).toBeNull()
    await user.click(screen.getByRole('button', { name: 'main' }))

    expect(panel()).not.toBeNull()
    // The search box and the two tabs are there.
    expect(screen.getByRole('textbox')).toBeTruthy()
    expect(screen.getByRole('tab', { name: /分支/ })).toBeTruthy()
    expect(screen.getByRole('tab', { name: /标签/ })).toBeTruthy()
  })

  it('默认分支带"默认"标记', async () => {
    const { user } = setup()

    await user.click(screen.getByRole('button', { name: 'main' }))

    const defaultOption = screen.getByRole('option', { name: /main/ })
    expect(defaultOption.textContent).toContain('默认')
    // A non-default branch has no badge.
    const other = screen.getByRole('option', { name: /feature\/login/ })
    expect(other.textContent).not.toContain('默认')
  })

  it('输入查询过滤分支，没有匹配时给出空状态', async () => {
    const { user } = setup()

    await user.click(screen.getByRole('button', { name: 'main' }))

    await user.type(screen.getByRole('textbox'), 'logout')
    expect(screen.queryByRole('option', { name: /feature\/login/ })).toBeNull()
    expect(screen.getByRole('option', { name: /feature\/logout/ })).toBeTruthy()

    await user.clear(screen.getByRole('textbox'))
    await user.type(screen.getByRole('textbox'), 'nothing-here')
    expect(screen.queryAllByRole('option')).toHaveLength(0)
    expect(screen.getByText('没有匹配项')).toBeTruthy()
  })

  it('切到标签 tab 显示标签而不是分支', async () => {
    const { user } = setup()

    await user.click(screen.getByRole('button', { name: 'main' }))

    expect(screen.getByRole('option', { name: /feature\/login/ })).toBeTruthy()

    await user.click(screen.getByRole('tab', { name: /标签/ }))

    expect(screen.getByRole('option', { name: 'v1.0.0' })).toBeTruthy()
    expect(screen.queryByRole('option', { name: /feature\/login/ })).toBeNull()
  })

  it('选中一个 ref 时调用 onSelect 并关闭面板', async () => {
    const { user, onSelect } = setup()

    await user.click(screen.getByRole('button', { name: 'main' }))
    await user.click(screen.getByRole('option', { name: /feature\/login/ }))

    expect(onSelect).toHaveBeenCalledTimes(1)
    expect(onSelect).toHaveBeenCalledWith('feature/login')
    expect(panel()).toBeNull()
  })

  it('Escape 关闭面板', async () => {
    const { user } = setup()

    await user.click(screen.getByRole('button', { name: 'main' }))
    expect(panel()).not.toBeNull()

    await user.keyboard('{Escape}')
    expect(panel()).toBeNull()
  })

  it('点击面板外部关闭面板', async () => {
    const { user } = setup()

    await user.click(screen.getByRole('button', { name: 'main' }))
    expect(panel()).not.toBeNull()

    await user.click(document.body)
    expect(panel()).toBeNull()
  })

  it('关闭按钮关闭面板', async () => {
    const { user } = setup()

    await user.click(screen.getByRole('button', { name: 'main' }))
    await user.click(screen.getByRole('button', { name: '取消' }))

    expect(panel()).toBeNull()
  })

  it('重新打开时清空上一次的查询', async () => {
    const { user } = setup()

    await user.click(screen.getByRole('button', { name: 'main' }))
    await user.type(screen.getByRole('textbox'), 'logout')
    await user.keyboard('{Escape}')

    await user.click(screen.getByRole('button', { name: 'main' }))
    expect((screen.getByRole('textbox') as HTMLInputElement).value).toBe('')
    // And the branches tab is active again, not the tags tab.
    expect(screen.getByRole('tab', { name: /分支/ }).getAttribute('aria-selected')).toBe('true')
  })

  it('当前 ref 标记为选中', async () => {
    const { user } = setup({ rev: 'feature/login' })

    await user.click(screen.getByRole('button', { name: 'feature/login' }))

    expect(
      screen.getByRole('option', { name: /feature\/login/ }).getAttribute('aria-selected'),
    ).toBe('true')
  })

  it('"查看所有分支"跳到仓库设置的分支页', async () => {
    const { user } = setup()

    await user.click(screen.getByRole('button', { name: 'main' }))
    await user.click(screen.getByRole('button', { name: '查看所有分支' }))

    expect(screen.getByTestId('location').textContent).toBe('/alice/widgets/settings/branches')
    expect(panel()).toBeNull()
  })

  it('"查看所有标签"跳到仓库设置的标签页', async () => {
    const { user } = setup()

    await user.click(screen.getByRole('button', { name: 'main' }))
    await user.click(screen.getByRole('tab', { name: /标签/ }))
    await user.click(screen.getByRole('button', { name: '查看所有标签' }))

    expect(screen.getByTestId('location').textContent).toBe('/alice/widgets/settings/tags')
  })
})

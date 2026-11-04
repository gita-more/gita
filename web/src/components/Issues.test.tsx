// Tests for Issues — the global issue feed.
//
// The feed reads GET /api/issues?status=all&limit=50 and lists every issue
// of every repository the user can see. Clicking an issue opens the
// repository it belongs to.
//
// What can actually break:
//  1. the issues must be listed with their repository, number and author;
//  2. an empty feed must show the empty state;
//  3. clicking an issue must open the right repository;
//  4. an API failure must be shown.

import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { MemoryRouter, useLocation } from 'react-router-dom'
import { afterEach, describe, expect, it, vi } from 'vitest'


vi.mock('../lib/api', () => ({
  api: {
    listGlobalIssues: vi.fn(),
  },
}))

import { api } from '../lib/api'
import { I18nProvider } from '../i18n'
import { Issues } from './Issues'
import type { GlobalIssueItem } from '../lib/api'

const listGlobalIssues = vi.mocked(api.listGlobalIssues)

function makeIssue(overrides: Partial<GlobalIssueItem> = {}): GlobalIssueItem {
  return {
    id: 1,
    number: 7,
    title: 'the widget explodes',
    body: '',
    status: 'open',
    author: 'alice',
    repo_owner: 'acme',
    repo_name: 'widgets',
    full_name: 'acme/widgets',
    created_at: '2026-01-01T00:00:00Z',
    ...overrides,
  }
}

function LocationProbe() {
  const loc = useLocation()
  return <div data-testid="location">{loc.pathname}</div>
}

function setup(onOpenRepo?: (owner: string, name: string) => void) {
  const user = userEvent.setup()
  render(
    <MemoryRouter initialEntries={['/issues']}>
      <I18nProvider>
        <Issues onOpenRepo={onOpenRepo} />
        <LocationProbe />
      </I18nProvider>
    </MemoryRouter>,
  )
  return { user }
}

describe('Issues', () => {
  it('列出 issue 并显示仓库、编号和作者', async () => {
    listGlobalIssues.mockResolvedValue({ issues: [makeIssue()] })

    setup()

    await waitFor(() => {
      expect(screen.getByText('the widget explodes')).toBeTruthy()
    })
    expect(screen.getByText('acme/widgets #7')).toBeTruthy()
    expect(screen.getByText(/alice/)).toBeTruthy()
    expect(screen.getByText('共 1 个 issue')).toBeTruthy()
  })

  it('调用接口时请求所有状态的 issue', async () => {
    listGlobalIssues.mockResolvedValue({ issues: [] })

    setup()

    await waitFor(() => {
      expect(listGlobalIssues).toHaveBeenCalledWith('all', 50)
    })
  })

  it('空列表显示空状态', async () => {
    listGlobalIssues.mockResolvedValue({ issues: [] })

    setup()

    await waitFor(() => {
      expect(screen.getByText('还没有 Issue')).toBeTruthy()
    })
  })

  it('点击 issue 打开它所属的仓库', async () => {
    listGlobalIssues.mockResolvedValue({ issues: [makeIssue()] })
    const onOpenRepo = vi.fn()
    const { user } = setup(onOpenRepo)

    await waitFor(() => {
      expect(screen.getByText('the widget explodes')).toBeTruthy()
    })
    await user.click(screen.getByText('the widget explodes'))

    expect(onOpenRepo).toHaveBeenCalledWith('acme', 'widgets')
  })

  it('没有 onOpenRepo 回调时导航到仓库页面', async () => {
    listGlobalIssues.mockResolvedValue({ issues: [makeIssue()] })
    const { user } = setup()

    await waitFor(() => {
      expect(screen.getByText('the widget explodes')).toBeTruthy()
    })
    await user.click(screen.getByText('the widget explodes'))

    expect(screen.getByTestId('location').textContent).toBe('/acme/widgets')
  })

  it('关闭的 issue 显示关闭状态', async () => {
    listGlobalIssues.mockResolvedValue({
      issues: [makeIssue({ id: 2, number: 8, title: 'old bug', status: 'closed' })],
    })

    setup()

    await waitFor(() => {
      expect(screen.getByText('old bug')).toBeTruthy()
    })
    expect(screen.getByText('已关闭')).toBeTruthy()
  })

  it('加载失败时显示错误', async () => {
    listGlobalIssues.mockRejectedValue(new Error('unauthorized'))

    setup()

    await waitFor(() => {
      expect(screen.getByText(/unauthorized|未认证/)).toBeTruthy()
    })
  })
})

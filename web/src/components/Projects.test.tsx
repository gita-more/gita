// Tests for Projects — the user's repository list.
//
// The page reads GET /api/repos and shows one card per repository, plus a
// create form.
//
// What can actually break:
//  1. every repository must be listed with its name, default branch and
//     visibility;
//  2. an empty list must show the empty state;
//  3. clicking a card must open the repository;
//  4. creating a repository must call the API with name/description/branch
//     and open the new repository;
//  5. an API failure must be shown.

import { cleanup, render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { MemoryRouter, useLocation } from 'react-router-dom'
import { afterEach, describe, expect, it, vi } from 'vitest'


vi.mock('../lib/api', () => ({
  api: {
    listRepos: vi.fn(),
    createRepo: vi.fn(),
  },
}))

import { api } from '../lib/api'
import { I18nProvider } from '../i18n'
import { Projects } from './Projects'
import type { Repository } from '../lib/api'

const listRepos = vi.mocked(api.listRepos)
const createRepo = vi.mocked(api.createRepo)

function makeRepo(overrides: Partial<Repository> = {}): Repository {
  return {
    id: 1,
    owner: 'alice',
    name: 'widgets',
    full_name: 'alice/widgets',
    description: 'the widgets repo',
    is_private: false,
    default_branch: 'main',
    created_at: '2026-01-01T00:00:00Z',
    clone_url_https: '/alice/widgets.git',
    clone_url_ssh: null,
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
    <MemoryRouter initialEntries={['/']}>
      <I18nProvider>
        <Projects onOpenRepo={onOpenRepo} />
        <LocationProbe />
      </I18nProvider>
    </MemoryRouter>,
  )
  return { user }
}

/** The create-repository form. Scoped so the header button with the same
 *  label does not collide. */
function createForm() {
  const form = document.querySelector('form.create-form')
  if (!form) throw new Error('create form is not open')
  return within(form as HTMLElement)
}

/** The header's "new repository" toggle. The empty state has a button with
 *  the same label, so the header is scoped explicitly. */
function openCreateFormButton() {
  const header = document.querySelector('.page-header')
  if (!header) throw new Error('page header is missing')
  return within(header as HTMLElement).getByRole('button', { name: '新建仓库' })
}

describe('Projects', () => {
  it('列出仓库并显示默认分支和可见性', async () => {
    listRepos.mockResolvedValue([
      makeRepo(),
      makeRepo({ id: 2, name: 'private-repo', full_name: 'alice/private-repo', is_private: true, default_branch: 'trunk' }),
    ])

    setup()

    await waitFor(() => {
      expect(screen.getByText('alice/widgets')).toBeTruthy()
    })
    expect(screen.getByText('alice/private-repo')).toBeTruthy()
    expect(screen.getByText('trunk')).toBeTruthy()
    expect(screen.getByText('公开')).toBeTruthy()
    expect(screen.getByText('私有')).toBeTruthy()
    expect(screen.getByText('共 2 个仓库')).toBeTruthy()
  })

  it('没有描述时给出占位文案', async () => {
    listRepos.mockResolvedValue([makeRepo({ description: '' })])

    setup()

    await waitFor(() => {
      expect(screen.getByText('alice/widgets')).toBeTruthy()
    })
    expect(screen.getByText('暂无描述')).toBeTruthy()
  })

  it('空列表显示空状态', async () => {
    listRepos.mockResolvedValue([])

    setup()

    await waitFor(() => {
      expect(screen.getByText('还没有仓库')).toBeTruthy()
    })
  })

  it('点击仓库卡片打开仓库', async () => {
    listRepos.mockResolvedValue([makeRepo()])
    const onOpenRepo = vi.fn()
    const { user } = setup(onOpenRepo)

    await waitFor(() => {
      expect(screen.getByText('alice/widgets')).toBeTruthy()
    })
    await user.click(screen.getByText('alice/widgets'))

    expect(onOpenRepo).toHaveBeenCalledWith('alice', 'widgets')
  })

  it('没有 onOpenRepo 回调时导航到仓库页面', async () => {
    listRepos.mockResolvedValue([makeRepo()])
    const { user } = setup()

    await waitFor(() => {
      expect(screen.getByText('alice/widgets')).toBeTruthy()
    })
    await user.click(screen.getByText('alice/widgets'))

    expect(screen.getByTestId('location').textContent).toBe('/alice/widgets')
  })

  it('创建仓库：提交表单调用 API 并打开新仓库', async () => {
    listRepos.mockResolvedValue([])
    createRepo.mockResolvedValue(makeRepo({ name: 'newrepo', full_name: 'alice/newrepo' }))
    const onOpenRepo = vi.fn()
    const { user } = setup(onOpenRepo)

    await waitFor(() => {
      expect(screen.getByText('还没有仓库')).toBeTruthy()
    })

    await user.click(openCreateFormButton())
    await user.type(createForm().getByLabelText('仓库名'), 'newrepo')
    await user.type(createForm().getByLabelText('描述（可选）'), 'a new repo')
    await user.click(createForm().getByRole('button', { name: '创建仓库' }))

    await waitFor(() => {
      expect(createRepo).toHaveBeenCalledWith('newrepo', 'a new repo', 'main', false)
    })
    expect(onOpenRepo).toHaveBeenCalledWith('alice', 'newrepo')
  })

  it('创建仓库时可以改默认分支', async () => {
    listRepos.mockResolvedValue([])
    createRepo.mockResolvedValue(makeRepo({ name: 'newrepo', full_name: 'alice/newrepo' }))
    const { user } = setup()

    await waitFor(() => {
      expect(screen.getByText('还没有仓库')).toBeTruthy()
    })

    await user.click(openCreateFormButton())
    await user.type(createForm().getByLabelText('仓库名'), 'newrepo')
    await user.clear(createForm().getByLabelText('默认分支'))
    await user.type(createForm().getByLabelText('默认分支'), 'trunk')
    await user.click(createForm().getByRole('button', { name: '创建仓库' }))

    await waitFor(() => {
      expect(createRepo).toHaveBeenCalledWith('newrepo', '', 'trunk', false)
    })
  })

  it('创建仓库失败时显示错误，表单保持打开', async () => {
    listRepos.mockResolvedValue([])
    createRepo.mockRejectedValue(new Error('repository already exists'))
    const { user } = setup()

    await waitFor(() => {
      expect(screen.getByText('还没有仓库')).toBeTruthy()
    })

    await user.click(openCreateFormButton())
    await user.type(createForm().getByLabelText('仓库名'), 'widgets')
    await user.click(createForm().getByRole('button', { name: '创建仓库' }))

    await waitFor(() => {
      expect(screen.getByText(/repository already exists|仓库已存在/)).toBeTruthy()
    })
    expect(createForm().getByLabelText('仓库名')).toBeTruthy()
  })

  it('加载失败时显示错误', async () => {
    listRepos.mockRejectedValue(new Error('unauthorized'))

    setup()

    await waitFor(() => {
      expect(screen.getByText(/unauthorized|未认证/)).toBeTruthy()
    })
  })
})

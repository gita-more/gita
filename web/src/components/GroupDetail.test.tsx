// Tests for GroupDetail — the /:group page.
//
// The page reads GET /api/groups/{full_path} and shows the group's
// repositories and subgroups in a tree. It can also create a repository or
// a subgroup inside the group.
//
// The component reads the group path from the route, so it is rendered
// inside a MemoryRouter with a "/:group" route.

import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { MemoryRouter, useLocation } from 'react-router-dom'
import { afterEach, describe, expect, it, vi } from 'vitest'


vi.mock('../lib/api', () => ({
  api: {
    getGroup: vi.fn(),
    createGroupRepo: vi.fn(),
    createSubgroup: vi.fn(),
  },
}))

import { api } from '../lib/api'
import { I18nProvider } from '../i18n'
import { GroupDetail } from './GroupDetail'
import type { Group, Repository } from '../lib/api'

const getGroup = vi.mocked(api.getGroup)
const createGroupRepo = vi.mocked(api.createGroupRepo)
const createSubgroup = vi.mocked(api.createSubgroup)

function makeGroup(overrides: Partial<Group> = {}): Group {
  return {
    id: 1,
    name: 'acme',
    description: 'the acme group',
    owner_id: 10,
    created_at: '2026-01-01T00:00:00Z',
    parent_id: null,
    full_path: 'acme',
    ...overrides,
  }
}

function makeRepo(overrides: Partial<Repository> = {}): Repository {
  return {
    id: 100,
    owner: 'acme',
    name: 'widgets',
    full_name: 'acme/widgets',
    description: 'the widgets repo',
    is_private: false,
    default_branch: 'main',
    created_at: '2026-01-01T00:00:00Z',
    clone_url_https: '/acme/widgets.git',
    clone_url_ssh: null,
    ...overrides,
  }
}

function LocationProbe() {
  const loc = useLocation()
  return <div data-testid="location">{loc.pathname}</div>
}

function setup(groupPath = 'acme') {
  const user = userEvent.setup()
  render(
    <MemoryRouter initialEntries={['/groups']}>
      <I18nProvider>
        <GroupDetail groupPath={groupPath} />
        <LocationProbe />
      </I18nProvider>
    </MemoryRouter>,
  )
  return { user }
}

describe('GroupDetail', () => {
  it('渲染群组名、描述、仓库和子群组', async () => {
    getGroup.mockResolvedValue({
      group: makeGroup(),
      repos: [makeRepo()],
      subgroups: [makeGroup({ id: 2, name: 'team', full_path: 'acme/team', parent_id: 1 })],
    })

    setup()

    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'widgets' })).toBeTruthy()
    })
    expect(screen.getByRole('button', { name: 'team' })).toBeTruthy()
    // The group description shows in the page header.
    expect(screen.getAllByText('the acme group').length).toBeGreaterThan(0)
  })

  it('调用 API 时带上路由里的群组路径', async () => {
    getGroup.mockResolvedValue({ group: makeGroup(), repos: [], subgroups: [] })

    setup('acme')

    await waitFor(() => {
      expect(getGroup).toHaveBeenCalledWith('acme')
    })
  })

  it('嵌套群组的多段路径完整传给 API', async () => {
    // /groups/gita/tests 是三段路径；它曾被 /:owner/:repo 抢走，导致
    // 嵌套群组详情页打不开。
    getGroup.mockResolvedValue({
      group: makeGroup({ full_path: 'gita/tests' }),
      repos: [],
      subgroups: [],
    })

    setup('gita/tests')

    await waitFor(() => {
      expect(getGroup).toHaveBeenCalledWith('gita/tests')
    })
  })

  it('没有仓库和子群组时显示空状态', async () => {
    getGroup.mockResolvedValue({ group: makeGroup(), repos: [], subgroups: [] })

    setup()

    await waitFor(() => {
      expect(screen.getByText('群组里还没有项目')).toBeTruthy()
    })
  })

  it('加载失败时显示错误', async () => {
    getGroup.mockRejectedValue(new Error('group not found'))

    setup()

    await waitFor(() => {
      expect(screen.getByText(/group not found|组不存在/)).toBeTruthy()
    })
  })

  it('点击仓库跳到仓库页面', async () => {
    getGroup.mockResolvedValue({ group: makeGroup(), repos: [makeRepo()], subgroups: [] })
    const { user } = setup()

    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'widgets' })).toBeTruthy()
    })
    await user.click(screen.getByRole('button', { name: 'widgets' }))

    expect(screen.getByTestId('location').textContent).toBe('/acme/widgets')
  })

  it('点击子群组跳到子群组页面', async () => {
    getGroup.mockResolvedValue({
      group: makeGroup(),
      repos: [],
      subgroups: [makeGroup({ id: 2, name: 'team', full_path: 'acme/team', parent_id: 1 })],
    })
    const { user } = setup()

    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'team' })).toBeTruthy()
    })
    await user.click(screen.getByRole('button', { name: 'team' }))

    expect(screen.getByTestId('location').textContent).toBe('/acme/team')
  })

  it('在群组下创建仓库：调用 API 并跳到新仓库', async () => {
    getGroup.mockResolvedValue({ group: makeGroup(), repos: [], subgroups: [] })
    createGroupRepo.mockResolvedValue(makeRepo({ name: 'newrepo', full_name: 'acme/newrepo' }))
    const { user } = setup()

    await waitFor(() => {
      expect(screen.getByText('群组里还没有项目')).toBeTruthy()
    })

    await user.click(screen.getByRole('button', { name: '新建项目' }))
    await user.type(screen.getByLabelText('仓库名'), 'newrepo')
    await user.type(screen.getByLabelText('描述（可选）'), 'a new repo')
    await user.click(screen.getByRole('button', { name: '创建仓库' }))

    await waitFor(() => {
      expect(createGroupRepo).toHaveBeenCalledWith('acme', 'newrepo', 'a new repo', false, 'main')
    })
    expect(screen.getByTestId('location').textContent).toBe('/acme/newrepo')
  })

  it('创建仓库失败时显示错误', async () => {
    getGroup.mockResolvedValue({ group: makeGroup(), repos: [], subgroups: [] })
    createGroupRepo.mockRejectedValue(new Error('repository already exists'))
    const { user } = setup()

    await waitFor(() => {
      expect(screen.getByText('群组里还没有项目')).toBeTruthy()
    })

    await user.click(screen.getByRole('button', { name: '新建项目' }))
    await user.type(screen.getByLabelText('仓库名'), 'widgets')
    await user.click(screen.getByRole('button', { name: '创建仓库' }))

    await waitFor(() => {
      expect(screen.getByText(/repository already exists|仓库已存在/)).toBeTruthy()
    })
  })

  it('创建子群组：调用 API 并刷新列表', async () => {
    getGroup.mockResolvedValue({ group: makeGroup(), repos: [], subgroups: [] })
    createSubgroup.mockResolvedValue(
      makeGroup({ id: 3, name: 'newsub', full_path: 'acme/newsub', parent_id: 1 }),
    )
    const { user } = setup()

    await waitFor(() => {
      expect(screen.getByText('群组里还没有项目')).toBeTruthy()
    })

    await user.click(screen.getByRole('button', { name: '新建子群组' }))
    await user.type(screen.getByLabelText('子群组名'), 'newsub')
    await user.click(screen.getByRole('button', { name: '创建子群组' }))

    await waitFor(() => {
      expect(createSubgroup).toHaveBeenCalledWith('acme', 'newsub', '')
    })
    // The list was reloaded after the subgroup was created.
    await waitFor(() => {
      expect(getGroup).toHaveBeenCalledTimes(2)
    })
  })

  it('创建子群组失败时显示错误', async () => {
    getGroup.mockResolvedValue({ group: makeGroup(), repos: [], subgroups: [] })
    createSubgroup.mockRejectedValue(new Error('group already exists'))
    const { user } = setup()

    await waitFor(() => {
      expect(screen.getByText('群组里还没有项目')).toBeTruthy()
    })

    await user.click(screen.getByRole('button', { name: '新建子群组' }))
    await user.type(screen.getByLabelText('子群组名'), 'team')
    await user.click(screen.getByRole('button', { name: '创建子群组' }))

    await waitFor(() => {
      expect(screen.getByText(/group already exists|组已存在/)).toBeTruthy()
    })
  })

  it('展开子群组时按需加载它的子节点', async () => {
    getGroup.mockResolvedValue({
      group: makeGroup(),
      repos: [],
      subgroups: [makeGroup({ id: 2, name: 'team', full_path: 'acme/team', parent_id: 1 })],
    })
    const { user } = setup()

    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'team' })).toBeTruthy()
    })

    // Before the user opens the subgroup, the page must not have asked the
    // server for the subgroup's own contents.
    expect(getGroup).not.toHaveBeenCalledWith('acme/team')

    await user.click(screen.getByRole('button', { name: '展开' }))

    // The lazy provider calls getGroup for the subgroup's full path.
    await waitFor(() => {
      expect(getGroup).toHaveBeenCalledWith('acme/team')
    })
  })
})

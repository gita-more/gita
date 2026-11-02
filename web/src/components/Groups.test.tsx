// @vitest-environment jsdom
// Tests for Groups — the group list page.
//
// The page is a thin client over GET /api/groups plus POST /api/groups.
// What can actually break:
//  1. the hierarchy must be built client-side from the flat list, including
//     groups whose parent is not visible to the user (they become roots);
//  2. the empty state must show when there are no groups;
//  3. creating a group must call the API with the name and description and
//     then open the new group;
//  4. an API failure must be shown, not swallowed.

import { cleanup, render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { MemoryRouter, useLocation } from 'react-router-dom'
import { afterEach, describe, expect, it, vi } from 'vitest'

afterEach(cleanup)

vi.mock('../lib/api', () => ({
  api: {
    listGroups: vi.fn(),
    createGroup: vi.fn(),
  },
}))

import { api } from '../lib/api'
import { I18nProvider } from '../i18n'
import { Groups } from './Groups'
import type { Group } from '../lib/api'

const listGroups = vi.mocked(api.listGroups)
const createGroup = vi.mocked(api.createGroup)

function makeGroup(overrides: Partial<Group> = {}): Group {
  return {
    id: 1,
    name: 'acme',
    description: '',
    owner_id: 10,
    created_at: '2026-01-01T00:00:00Z',
    parent_id: null,
    full_path: 'acme',
    ...overrides,
  }
}

function LocationProbe() {
  const loc = useLocation()
  return <div data-testid="location">{loc.pathname}</div>
}

function setup(onOpenGroup?: (g: string) => void) {
  const user = userEvent.setup()
  render(
    <MemoryRouter initialEntries={['/groups']}>
      <I18nProvider>
        <Groups onOpenGroup={onOpenGroup} />
        <LocationProbe />
      </I18nProvider>
    </MemoryRouter>,
  )
  return { user }
}

/** The create-group form. Scoped queries avoid the header button, which uses
 *  the same labels ("取消" / "创建群组"). */
function createForm() {
  const form = document.querySelector('form.create-form')
  if (!form) throw new Error('create form is not open')
  return within(form as HTMLElement)
}

describe('Groups', () => {
  it('渲染群组列表', async () => {
    listGroups.mockResolvedValue([
      makeGroup({ id: 1, name: 'acme', full_path: 'acme' }),
      makeGroup({ id: 2, name: 'widgets-labs', full_path: 'widgets-labs' }),
    ])

    setup()

    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'acme' })).toBeTruthy()
    })
    expect(screen.getByRole('button', { name: 'widgets-labs' })).toBeTruthy()
    // The subtitle counts the groups.
    expect(screen.getByText('共 2 个群组')).toBeTruthy()
  })

  it('空列表显示空状态', async () => {
    listGroups.mockResolvedValue([])

    setup()

    await waitFor(() => {
      expect(screen.getByText('还没有群组')).toBeTruthy()
    })
    expect(screen.getByText(/创建一个群组/)).toBeTruthy()
  })

  it('父群组不在列表里的群组也必须可见（当作根节点）', async () => {
    listGroups.mockResolvedValue([
      // parent_id points at a group the user cannot see.
      makeGroup({ id: 5, name: 'secret-sub', full_path: 'secret/sub', parent_id: 999 }),
    ])

    setup()

    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'secret-sub' })).toBeTruthy()
    })
  })

  it('点击群组跳到该群组页面', async () => {
    listGroups.mockResolvedValue([makeGroup({ id: 1, name: 'acme', full_path: 'acme' })])
    const { user } = setup()

    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'acme' })).toBeTruthy()
    })
    await user.click(screen.getByRole('button', { name: 'acme' }))

    expect(screen.getByTestId('location').textContent).toBe('/acme')
  })

  it('加载失败时显示错误', async () => {
    listGroups.mockRejectedValue(new Error('unauthorized'))

    setup()

    await waitFor(() => {
      expect(screen.getByText(/unauthorized|未认证/)).toBeTruthy()
    })
  })

  it('展开后显示子群组', async () => {
    listGroups.mockResolvedValue([
      makeGroup({ id: 1, name: 'acme', full_path: 'acme' }),
      makeGroup({ id: 2, name: 'team', full_path: 'acme/team', parent_id: 1 }),
    ])
    const { user } = setup()

    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'acme' })).toBeTruthy()
    })
    // The child is not shown until the parent is expanded.
    expect(screen.queryByRole('button', { name: 'team' })).toBeNull()

    await user.click(screen.getByRole('button', { name: '展开' }))

    expect(screen.getByRole('button', { name: 'team' })).toBeTruthy()
  })

  it('创建群组：提交表单调用 API 并打开新群组', async () => {
    listGroups.mockResolvedValue([])
    createGroup.mockResolvedValue(
      makeGroup({ id: 7, name: 'newteam', full_path: 'newteam' }),
    )
    const onOpenGroup = vi.fn()
    const { user } = setup(onOpenGroup)

    await waitFor(() => {
      expect(screen.getByText('还没有群组')).toBeTruthy()
    })

    await user.click(screen.getByRole('button', { name: '新建群组' }))
    await user.type(createForm().getByLabelText('群组名'), 'newteam')
    await user.type(createForm().getByLabelText('描述（可选）'), 'the new team')
    await user.click(createForm().getByRole('button', { name: '创建群组' }))

    await waitFor(() => {
      expect(createGroup).toHaveBeenCalledWith('newteam', 'the new team')
    })
    expect(onOpenGroup).toHaveBeenCalledWith('newteam')
  })

  it('创建群组失败时显示错误，表单保持打开', async () => {
    listGroups.mockResolvedValue([])
    createGroup.mockRejectedValue(new Error('group already exists'))
    const { user } = setup()

    await waitFor(() => {
      expect(screen.getByText('还没有群组')).toBeTruthy()
    })

    await user.click(screen.getByRole('button', { name: '新建群组' }))
    await user.type(createForm().getByLabelText('群组名'), 'acme')
    await user.click(createForm().getByRole('button', { name: '创建群组' }))

    await waitFor(() => {
      expect(screen.getByText(/group already exists|组已存在/)).toBeTruthy()
    })
    // The form is still there so the user can fix the name.
    expect(screen.getByLabelText('群组名')).toBeTruthy()
  })

  it('取消按钮关闭创建表单', async () => {
    listGroups.mockResolvedValue([])
    const { user } = setup()

    await waitFor(() => {
      expect(screen.getByText('还没有群组')).toBeTruthy()
    })

    await user.click(screen.getByRole('button', { name: '新建群组' }))
    expect(createForm().getByLabelText('群组名')).toBeTruthy()

    await user.click(createForm().getByRole('button', { name: '取消' }))
    expect(screen.queryByLabelText('群组名')).toBeNull()
  })
})

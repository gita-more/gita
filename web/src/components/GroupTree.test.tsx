// @vitest-environment jsdom
// Tests for GroupTree — the GitLab-style group/project tree.
//
// The tree has two flavours of children provider and both are easy to get
// wrong:
//  - a synchronous provider (the children are already known), where the
//    expand arrow only appears when there are children;
//  - a lazy provider (returns a Promise), where every group row gets an
//    arrow and the children are fetched on first expand.
//
// What is asserted here is user-visible behaviour: which rows show, what
// they say, what happens when you click them.

import { cleanup, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'

afterEach(cleanup)

import { I18nProvider } from '../i18n'
import type { Group, Repository } from '../lib/api'
import { GroupTree, type GroupTreeChild } from './GroupTree'

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

function makeRepo(overrides: Partial<Repository> = {}): Repository {
  return {
    id: 100,
    owner: 'acme',
    name: 'widgets',
    full_name: 'acme/widgets',
    description: '',
    is_private: false,
    default_branch: 'main',
    created_at: '2026-01-01T00:00:00Z',
    clone_url_https: '/acme/widgets.git',
    clone_url_ssh: null,
    ...overrides,
  }
}

function renderTree(
  nodes: GroupTreeChild[],
  props: Partial<React.ComponentProps<typeof GroupTree>> = {},
) {
  const user = userEvent.setup()
  render(
    <I18nProvider>
      <GroupTree nodes={nodes} {...props} />
    </I18nProvider>,
  )
  return { user }
}

describe('GroupTree', () => {
  it('渲染群组行与仓库行', () => {
    renderTree([
      { kind: 'group', group: makeGroup() },
      { kind: 'repo', repo: makeRepo() },
    ])

    expect(screen.getByRole('button', { name: 'acme' })).toBeTruthy()
    expect(screen.getByRole('button', { name: 'widgets' })).toBeTruthy()
  })

  it('仓库行显示默认分支和公开/私有', () => {
    renderTree([
      { kind: 'repo', repo: makeRepo({ is_private: true, default_branch: 'trunk' }) },
    ])

    expect(screen.getByText('trunk')).toBeTruthy()
    expect(screen.getByText('私有')).toBeTruthy()
  })

  it('没有描述时给出占位文案', () => {
    renderTree([{ kind: 'group', group: makeGroup({ description: '' }) }])

    expect(screen.getByText('暂无描述')).toBeTruthy()
  })

  it('点击群组行回调完整路径，点击仓库行回调 owner 和名字', async () => {
    const onOpenGroup = vi.fn()
    const onOpenRepo = vi.fn()
    const { user } = renderTree(
      [
        { kind: 'group', group: makeGroup({ full_path: 'acme/team' }) },
        { kind: 'repo', repo: makeRepo() },
      ],
      { onOpenGroup, onOpenRepo },
    )

    await user.click(screen.getByRole('button', { name: 'acme' }))
    expect(onOpenGroup).toHaveBeenCalledWith('acme/team')

    await user.click(screen.getByRole('button', { name: 'widgets' }))
    expect(onOpenRepo).toHaveBeenCalledWith('acme', 'widgets')
  })

  it('同步 children provider：有子节点才显示展开箭头', () => {
    renderTree([{ kind: 'group', group: makeGroup() }], {
      getChildren: () => [{ kind: 'repo', repo: makeRepo() }],
    })

    expect(screen.getByRole('button', { name: '展开' })).toBeTruthy()
  })

  it('同步 children provider：没有子节点时不显示展开箭头', () => {
    renderTree([{ kind: 'group', group: makeGroup() }], {
      getChildren: () => [],
    })

    expect(screen.queryByRole('button', { name: '展开' })).toBeNull()
  })

  it('同步 children provider：展开后显示子节点', async () => {
    const { user } = renderTree([{ kind: 'group', group: makeGroup() }], {
      getChildren: () => [{ kind: 'repo', repo: makeRepo({ name: 'inner-repo' }) }],
    })

    await user.click(screen.getByRole('button', { name: '展开' }))

    expect(screen.getByRole('button', { name: 'inner-repo' })).toBeTruthy()
    // The arrow flips to "collapse".
    expect(screen.getByRole('button', { name: '收起' })).toBeTruthy()
  })

  it('同步 children provider：再次点击收起子节点', async () => {
    const { user } = renderTree([{ kind: 'group', group: makeGroup() }], {
      getChildren: () => [{ kind: 'repo', repo: makeRepo({ name: 'inner-repo' }) }],
    })

    await user.click(screen.getByRole('button', { name: '展开' }))
    expect(screen.getByRole('button', { name: 'inner-repo' })).toBeTruthy()

    await user.click(screen.getByRole('button', { name: '收起' }))
    expect(screen.queryByRole('button', { name: 'inner-repo' })).toBeNull()
  })

  it('懒加载 provider：展开时加载并显示子节点', async () => {
    const getChildren = vi.fn(async () => [
      { kind: 'repo' as const, repo: makeRepo({ name: 'lazy-repo' }) },
    ])
    const { user } = renderTree([{ kind: 'group', group: makeGroup() }], {
      getChildren,
      childrenMode: 'lazy',
    })

    // The arrow is there even though we don't know the children yet.
    await user.click(screen.getByRole('button', { name: '展开' }))

    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'lazy-repo' })).toBeTruthy()
    })
    expect(getChildren).toHaveBeenCalled()
  })

  it('懒加载 provider：用户展开之前不请求子节点', () => {
    const getChildren = vi.fn(async () => [])

    renderTree([{ kind: 'group', group: makeGroup() }], {
      getChildren,
      childrenMode: 'lazy',
    })

    // A lazy provider must only be called for the rows the user opens —
    // probing it while rendering would load every group of the page.
    expect(getChildren).not.toHaveBeenCalled()
  })

  it('懒加载 provider：展开一次只请求一次', async () => {
    const getChildren = vi.fn(async () => [
      { kind: 'repo' as const, repo: makeRepo({ name: 'lazy-repo' }) },
    ])
    const { user } = renderTree([{ kind: 'group', group: makeGroup() }], {
      getChildren,
      childrenMode: 'lazy',
    })

    await user.click(screen.getByRole('button', { name: '展开' }))
    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'lazy-repo' })).toBeTruthy()
    })
    expect(getChildren).toHaveBeenCalledTimes(1)

    // Collapsing and expanding again reuses the children it already has.
    await user.click(screen.getByRole('button', { name: '收起' }))
    await user.click(screen.getByRole('button', { name: '展开' }))
    expect(getChildren).toHaveBeenCalledTimes(1)
    expect(screen.getByRole('button', { name: 'lazy-repo' })).toBeTruthy()
  })

  it('懒加载 provider：加载失败时显示错误而不是空白', async () => {
    const { user } = renderTree([{ kind: 'group', group: makeGroup() }], {
      getChildren: () => Promise.reject(new Error('not found: group missing')),
      childrenMode: 'lazy',
    })

    await user.click(screen.getByRole('button', { name: '展开' }))

    await waitFor(() => {
      expect(screen.getByText(/not found|未找到/)).toBeTruthy()
    })
  })

  it('懒加载 provider：失败后可以重试，重试成功则显示子节点', async () => {
    let fail = true
    const getChildren = vi.fn(() =>
      fail
        ? Promise.reject(new Error('boom'))
        : Promise.resolve([{ kind: 'repo' as const, repo: makeRepo({ name: 'retry-repo' }) }]),
    )
    const { user } = renderTree([{ kind: 'group', group: makeGroup() }], {
      getChildren,
      childrenMode: 'lazy',
    })

    await user.click(screen.getByRole('button', { name: '展开' }))
    await waitFor(() => {
      expect(screen.getByText(/boom/)).toBeTruthy()
    })

    // Collapse, then expand again with the provider healthy.
    fail = false
    await user.click(screen.getByRole('button', { name: '收起' }))
    await user.click(screen.getByRole('button', { name: '展开' }))

    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'retry-repo' })).toBeTruthy()
    })
  })

  it('排序：子群组在前、仓库在后，各按名称排序', () => {
    renderTree([
      { kind: 'repo', repo: makeRepo({ id: 1, name: 'zeta-repo' }) },
      { kind: 'group', group: makeGroup({ id: 2, name: 'zeta-group' }) },
      { kind: 'repo', repo: makeRepo({ id: 3, name: 'alpha-repo' }) },
      { kind: 'group', group: makeGroup({ id: 4, name: 'alpha-group' }) },
    ])

    const names = screen
      .getAllByRole('button', { name: /^(alpha|zeta)/ })
      .map((b) => b.textContent)
    expect(names).toEqual(['alpha-group', 'zeta-group', 'alpha-repo', 'zeta-repo'])
  })

  it('子节点也可以再次展开（嵌套层级）', async () => {
    const { user } = renderTree([{ kind: 'group', group: makeGroup() }], {
      childrenMode: 'lazy',
      getChildren: (g) =>
        Promise.resolve([
          {
            kind: 'group',
            group: makeGroup({ id: 2, name: 'sub', full_path: 'acme/sub', parent_id: g.id }),
          },
        ]),
    })

    await user.click(screen.getByRole('button', { name: '展开' }))
    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'sub' })).toBeTruthy()
    })

    // The nested group has its own expand arrow, so the tree is two levels
    // deep: the parent is expanded ("收起") and the child is still closed
    // ("展开").
    expect(screen.getAllByRole('button', { name: '收起' })).toHaveLength(1)
    expect(screen.getAllByRole('button', { name: '展开' })).toHaveLength(1)
  })
})

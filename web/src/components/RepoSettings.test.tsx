// Tests for RepoSettings — the repository settings page.
//
// The page has four sections (general / branches / tags / pages) selected
// by the `section` prop, and it is a thin client over the repo, branch and
// tag endpoints.
//
// What can actually break:
//  1. the general section must show the current name, description,
//     visibility, default branch and clone URLs;
//  2. saving must PATCH the repository;
//  3. a rename must follow the repository to its new URL;
//  4. branches and tags must be listed, creatable and deletable;
//  5. deleting the repository must go through the confirmation dialog;
//  6. an API failure must be shown.

import { cleanup, render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { MemoryRouter, useLocation } from 'react-router-dom'
import { afterEach, describe, expect, it, vi } from 'vitest'


vi.mock('../lib/api', () => ({
  api: {
    getRepo: vi.fn(),
    getBranches: vi.fn(),
    listTags: vi.fn(),
    listGroups: vi.fn(),
    updateRepo: vi.fn(),
    createBranch: vi.fn(),
    deleteBranch: vi.fn(),
    createTag: vi.fn(),
    deleteTag: vi.fn(),
    deleteRepo: vi.fn(),
    getPages: vi.fn(),
    putPages: vi.fn(),
    deletePages: vi.fn(),
  },
}))

import { api } from '../lib/api'
import { I18nProvider } from '../i18n'
import { RepoSettings } from './RepoSettings'
import type { Group, Repository } from '../lib/api'

const getRepo = vi.mocked(api.getRepo)
const getBranches = vi.mocked(api.getBranches)
const listTags = vi.mocked(api.listTags)
const listGroups = vi.mocked(api.listGroups)
const updateRepo = vi.mocked(api.updateRepo)
const createBranch = vi.mocked(api.createBranch)
const deleteBranch = vi.mocked(api.deleteBranch)
const createTag = vi.mocked(api.createTag)
const deleteTag = vi.mocked(api.deleteTag)
const deleteRepo = vi.mocked(api.deleteRepo)

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
    clone_url_ssh: 'ssh://git@example.com/alice/widgets.git',
    ...overrides,
  }
}

function makeGroup(overrides: Partial<Group> = {}): Group {
  return {
    id: 9,
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

/** The "move repository" row. Its label is a plain <span>, not a <label>,
 *  so the row is scoped by its container instead. */
function moveRow() {
  const row = document.querySelector('.settings-move-row')
  if (!row) throw new Error('the move row is missing')
  return within(row as HTMLElement)
}

function setup(
  section: string | null = 'general',
  onSectionChange = vi.fn(),
  onBack = vi.fn(),
) {
  const user = userEvent.setup()
  render(
    <MemoryRouter initialEntries={['/alice/widgets/settings/general']}>
      <I18nProvider>
        <RepoSettings
          owner="alice"
          name="widgets"
          section={section}
          onSectionChange={onSectionChange}
          onBack={onBack}
        />
        <LocationProbe />
      </I18nProvider>
    </MemoryRouter>,
  )
  return { user, onSectionChange, onBack }
}

describe('RepoSettings', () => {
  it('常规分区显示仓库当前的名字、描述、可见性和默认分支', async () => {
    getRepo.mockResolvedValue(makeRepo())
    getBranches.mockResolvedValue([])
    listTags.mockResolvedValue([])
    listGroups.mockResolvedValue([])

    setup('general')

    await waitFor(() => {
      expect((screen.getByLabelText('仓库名称') as HTMLInputElement).value).toBe('widgets')
    })
    expect((screen.getByLabelText('描述') as HTMLTextAreaElement).value).toBe('the widgets repo')
    expect((screen.getByLabelText('可见性') as HTMLSelectElement).value).toBe('public')
    expect(screen.getByText('main')).toBeTruthy()
  })

  it('显示克隆地址', async () => {
    getRepo.mockResolvedValue(makeRepo())
    getBranches.mockResolvedValue([])
    listTags.mockResolvedValue([])
    listGroups.mockResolvedValue([])

    setup('general')

    await waitFor(() => {
      // A relative clone URL is turned into an absolute one.
      expect(screen.getByText('http://localhost:3000/alice/widgets.git')).toBeTruthy()
    })
    expect(screen.getByText('ssh://git@example.com/alice/widgets.git')).toBeTruthy()
  })

  it('保存时把描述和可见性提交给接口', async () => {
    getRepo.mockResolvedValue(makeRepo())
    getBranches.mockResolvedValue([])
    listTags.mockResolvedValue([])
    listGroups.mockResolvedValue([])
    updateRepo.mockResolvedValue(makeRepo())

    const { user } = setup('general')

    await waitFor(() => {
      expect(screen.getByLabelText('仓库名称')).toBeTruthy()
    })

    await user.clear(screen.getByLabelText('描述'))
    await user.type(screen.getByLabelText('描述'), 'new description')
    await user.selectOptions(screen.getByLabelText('可见性'), 'private')
    await user.click(screen.getByRole('button', { name: '保存更改' }))

    await waitFor(() => {
      expect(updateRepo).toHaveBeenCalledWith('alice', 'widgets', {
        description: 'new description',
        is_private: true,
      })
    })
    expect(screen.getByText('已保存。')).toBeTruthy()
  })

  it('改名后跳到新名字的设置页', async () => {
    getRepo.mockResolvedValue(makeRepo())
    getBranches.mockResolvedValue([])
    listTags.mockResolvedValue([])
    listGroups.mockResolvedValue([])
    updateRepo.mockResolvedValue(makeRepo({ name: 'gadgets', full_name: 'alice/gadgets' }))

    const { user } = setup('general')

    await waitFor(() => {
      expect(screen.getByLabelText('仓库名称')).toBeTruthy()
    })

    await user.clear(screen.getByLabelText('仓库名称'))
    await user.type(screen.getByLabelText('仓库名称'), 'gadgets')
    await user.click(screen.getByRole('button', { name: '保存更改' }))

    await waitFor(() => {
      expect(updateRepo).toHaveBeenCalledWith('alice', 'widgets', {
        description: 'the widgets repo',
        is_private: false,
        new_name: 'gadgets',
      })
    })
    expect(screen.getByTestId('location').textContent).toBe('/alice/gadgets/settings/general')
  })

  it('保存失败时显示错误', async () => {
    getRepo.mockResolvedValue(makeRepo())
    getBranches.mockResolvedValue([])
    listTags.mockResolvedValue([])
    listGroups.mockResolvedValue([])
    updateRepo.mockRejectedValue(new Error('repository already exists'))

    const { user } = setup('general')

    await waitFor(() => {
      expect(screen.getByLabelText('仓库名称')).toBeTruthy()
    })
    await user.click(screen.getByRole('button', { name: '保存更改' }))

    await waitFor(() => {
      expect(screen.getByText(/repository already exists|仓库已存在/)).toBeTruthy()
    })
  })

  it('分区导航切换 section', async () => {
    getRepo.mockResolvedValue(makeRepo())
    getBranches.mockResolvedValue([])
    listTags.mockResolvedValue([])
    listGroups.mockResolvedValue([])

    const { user, onSectionChange } = setup('general')

    await waitFor(() => {
      expect(screen.getByRole('button', { name: '分支' })).toBeTruthy()
    })
    await user.click(screen.getByRole('button', { name: '分支' }))

    expect(onSectionChange).toHaveBeenCalledWith('branches')
  })

  it('分支分区列出分支并标记默认分支', async () => {
    getRepo.mockResolvedValue(makeRepo())
    getBranches.mockResolvedValue(['main', 'feature/login'])
    listTags.mockResolvedValue([])
    listGroups.mockResolvedValue([])

    setup('branches')

    await waitFor(() => {
      expect(screen.getByText('feature/login')).toBeTruthy()
    })
    expect(screen.getByText('main')).toBeTruthy()
    // The default branch carries the badge.
    expect(screen.getByText('默认')).toBeTruthy()
  })

  it('创建分支调用接口并刷新列表', async () => {
    getRepo.mockResolvedValue(makeRepo())
    getBranches.mockResolvedValue(['main'])
    listTags.mockResolvedValue([])
    listGroups.mockResolvedValue([])
    createBranch.mockResolvedValue({ branch: 'feature/new' })

    const { user } = setup('branches')

    await waitFor(() => {
      expect(screen.getByPlaceholderText('新分支名')).toBeTruthy()
    })

    await user.type(screen.getByPlaceholderText('新分支名'), 'feature/new')
    await user.click(screen.getByRole('button', { name: '创建分支' }))

    await waitFor(() => {
      expect(createBranch).toHaveBeenCalledWith('alice', 'widgets', 'feature/new', undefined)
    })
    await waitFor(() => {
      expect(getBranches).toHaveBeenCalledTimes(2)
    })
  })

  it('删除分支调用接口', async () => {
    getRepo.mockResolvedValue(makeRepo())
    getBranches.mockResolvedValue(['main', 'feature/login'])
    listTags.mockResolvedValue([])
    listGroups.mockResolvedValue([])
    deleteBranch.mockResolvedValue({ message: 'ok' })

    const { user } = setup('branches')

    await waitFor(() => {
      expect(screen.getByText('feature/login')).toBeTruthy()
    })
    // Both branches have a delete button; pick the one next to the branch.
    const items = screen.getAllByTitle('删除分支')
    await user.click(items[items.length - 1])

    await waitFor(() => {
      expect(deleteBranch).toHaveBeenCalledWith('alice', 'widgets', 'feature/login')
    })
  })

  it('标签分区列出标签并可以创建', async () => {
    getRepo.mockResolvedValue(makeRepo())
    getBranches.mockResolvedValue([])
    listTags.mockResolvedValue(['v1.0.0'])
    listGroups.mockResolvedValue([])
    createTag.mockResolvedValue({ tag: 'v2.0.0' })

    const { user } = setup('tags')

    await waitFor(() => {
      expect(screen.getByText('v1.0.0')).toBeTruthy()
    })

    await user.type(screen.getByPlaceholderText('新标签名'), 'v2.0.0')
    await user.click(screen.getByRole('button', { name: '创建标签' }))

    await waitFor(() => {
      expect(createTag).toHaveBeenCalledWith('alice', 'widgets', 'v2.0.0', undefined)
    })
  })

  it('删除标签调用接口', async () => {
    getRepo.mockResolvedValue(makeRepo())
    getBranches.mockResolvedValue([])
    listTags.mockResolvedValue(['v1.0.0'])
    listGroups.mockResolvedValue([])
    deleteTag.mockResolvedValue({ message: 'ok' })

    const { user } = setup('tags')

    await waitFor(() => {
      expect(screen.getByText('v1.0.0')).toBeTruthy()
    })
    await user.click(screen.getByTitle('删除标签'))

    await waitFor(() => {
      expect(deleteTag).toHaveBeenCalledWith('alice', 'widgets', 'v1.0.0')
    })
  })

  it('删除仓库需要二次确认，确认后调用接口并跳回首页', async () => {
    getRepo.mockResolvedValue(makeRepo())
    getBranches.mockResolvedValue([])
    listTags.mockResolvedValue([])
    listGroups.mockResolvedValue([])
    deleteRepo.mockResolvedValue({ message: 'ok' })

    const { user } = setup('general')

    await waitFor(() => {
      expect(screen.getByRole('button', { name: '删除此仓库' })).toBeTruthy()
    })
    await user.click(screen.getByRole('button', { name: '删除此仓库' }))

    // A confirmation dialog appears.
    const dialog = await screen.findByRole('dialog')
    expect(dialog.textContent).toContain('确定要删除这个仓库吗')
    // Nothing is deleted until the user confirms.
    expect(deleteRepo).not.toHaveBeenCalled()

    // The confirm button carries the same label as the trigger, so it is
    // clicked inside the dialog.
    await user.click(within(dialog).getByRole('button', { name: '删除此仓库' }))

    await waitFor(() => {
      expect(deleteRepo).toHaveBeenCalledWith('alice', 'widgets')
    })
    expect(screen.getByTestId('location').textContent).toBe('/')
  })

  it('取消确认对话框不会删除仓库', async () => {
    getRepo.mockResolvedValue(makeRepo())
    getBranches.mockResolvedValue([])
    listTags.mockResolvedValue([])
    listGroups.mockResolvedValue([])

    const { user } = setup('general')

    await waitFor(() => {
      expect(screen.getByRole('button', { name: '删除此仓库' })).toBeTruthy()
    })
    await user.click(screen.getByRole('button', { name: '删除此仓库' }))
    await user.click(screen.getByRole('button', { name: '取消' }))

    expect(deleteRepo).not.toHaveBeenCalled()
    expect(screen.queryByRole('dialog')).toBeNull()
  })

  it('可以移动仓库到另一个群组', async () => {
    getRepo.mockResolvedValue(makeRepo())
    getBranches.mockResolvedValue([])
    listTags.mockResolvedValue([])
    listGroups.mockResolvedValue([makeGroup()])
    updateRepo.mockResolvedValue(makeRepo())

    const { user } = setup('general')

    await waitFor(() => {
      expect(moveRow().getByRole('combobox')).toBeTruthy()
    })

    await user.selectOptions(moveRow().getByRole('combobox'), 'acme')
    await user.click(moveRow().getByRole('button', { name: '移动仓库' }))

    // A confirmation dialog names the source and the target.
    const dialog = await screen.findByRole('dialog')
    expect(dialog.textContent).toContain('alice/widgets')
    expect(dialog.textContent).toContain('acme/widgets')

    await user.click(within(dialog).getByRole('button', { name: '移动仓库' }))

    await waitFor(() => {
      expect(updateRepo).toHaveBeenCalledWith('alice', 'widgets', { new_owner: 'acme' })
    })
    expect(screen.getByTestId('location').textContent).toBe('/acme/widgets/settings')
  })

  it('当前群组不出现在移动目标里', async () => {
    getRepo.mockResolvedValue(makeRepo())
    getBranches.mockResolvedValue([])
    listTags.mockResolvedValue([])
    listGroups.mockResolvedValue([
      makeGroup({ id: 1, name: 'alice', full_path: 'alice' }),
      makeGroup({ id: 2, name: 'acme', full_path: 'acme' }),
    ])

    setup('general')

    await waitFor(() => {
      expect(moveRow().getByRole('combobox')).toBeTruthy()
    })
    const select = moveRow().getByRole('combobox') as HTMLSelectElement
    const values = Array.from(select.options).map((o) => o.value)
    expect(values).toContain('acme')
    expect(values).not.toContain('alice')
  })

  it('返回按钮触发 onBack', async () => {
    getRepo.mockResolvedValue(makeRepo())
    getBranches.mockResolvedValue([])
    listTags.mockResolvedValue([])
    listGroups.mockResolvedValue([])

    const { user, onBack } = setup('general')

    await waitFor(() => {
      expect(screen.getByRole('button', { name: '返回' })).toBeTruthy()
    })
    await user.click(screen.getByRole('button', { name: '返回' }))

    expect(onBack).toHaveBeenCalled()
  })
})

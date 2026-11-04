// Tests for PagesPanel — the Pages publishing configuration.
//
// The panel reads the current Pages config and the repository (to know
// whether the repo is private), lets the user pick a branch and a
// directory, and saves or disables the configuration.
//
// What can actually break:
//  1. the panel must show the current branch/path and the public URL;
//  2. the "enable" toggle must reveal the form;
//  3. saving must call PUT with the chosen branch and path;
//  4. disabling must call DELETE and turn Pages off;
//  5. a missing branch must be reported and block saving;
//  6. a private repo must show the auth notice;
//  7. an API failure must be shown.

import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'


vi.mock('../lib/api', () => ({
  api: {
    getPages: vi.fn(),
    getRepo: vi.fn(),
    putPages: vi.fn(),
    deletePages: vi.fn(),
  },
}))

import { api } from '../lib/api'
import { I18nProvider } from '../i18n'
import { PagesPanel } from './PagesPanel'
import type { PagesConfig, Repository } from '../lib/api'

const getPages = vi.mocked(api.getPages)
const getRepo = vi.mocked(api.getRepo)
const putPages = vi.mocked(api.putPages)
const deletePages = vi.mocked(api.deletePages)

function makeConfig(overrides: Partial<PagesConfig> = {}): PagesConfig {
  return {
    enabled: true,
    branch: 'main',
    path: '/',
    url: '/alice/widgets/',
    available: true,
    ...overrides,
  }
}

function makeRepo(overrides: Partial<Repository> = {}): Repository {
  return {
    id: 1,
    owner: 'alice',
    name: 'widgets',
    full_name: 'alice/widgets',
    description: '',
    is_private: false,
    default_branch: 'main',
    created_at: '2026-01-01T00:00:00Z',
    clone_url_https: '/alice/widgets.git',
    clone_url_ssh: null,
    ...overrides,
  }
}

function setup() {
  const user = userEvent.setup()
  render(
    <I18nProvider>
      <PagesPanel owner="alice" name="widgets" />
    </I18nProvider>,
  )
  return { user }
}

describe('PagesPanel', () => {
  it('渲染当前配置：分支、目录和站点地址', async () => {
    getPages.mockResolvedValue(makeConfig())
    getRepo.mockResolvedValue(makeRepo())

    setup()

    await waitFor(() => {
      expect(screen.getByLabelText('发布分支')).toBeTruthy()
    })
    expect((screen.getByLabelText('发布分支') as HTMLInputElement).value).toBe('main')
    expect((screen.getByLabelText('发布目录') as HTMLSelectElement).value).toBe('/')
    // The site URL is server-computed and shown as a link.
    expect(screen.getByRole('link', { name: '访问站点' })).toBeTruthy()
    expect(screen.getByText('/alice/widgets/')).toBeTruthy()
  })

  it('未启用时不显示配置表单', async () => {
    getPages.mockResolvedValue(makeConfig({ enabled: false, url: '' }))
    getRepo.mockResolvedValue(makeRepo())

    setup()

    await waitFor(() => {
      expect(screen.getByLabelText('启用 Pages')).toBeTruthy()
    })
    expect(screen.queryByLabelText('发布分支')).toBeNull()
  })

  it('勾选启用后显示配置表单', async () => {
    getPages.mockResolvedValue(makeConfig({ enabled: false, url: '' }))
    getRepo.mockResolvedValue(makeRepo())

    const { user } = setup()

    await waitFor(() => {
      expect(screen.getByLabelText('启用 Pages')).toBeTruthy()
    })
    await user.click(screen.getByLabelText('启用 Pages'))

    expect(screen.getByLabelText('发布分支')).toBeTruthy()
    expect(screen.getByLabelText('发布目录')).toBeTruthy()
  })

  it('保存时把分支和目录传给接口', async () => {
    getPages.mockResolvedValue(makeConfig())
    getRepo.mockResolvedValue(makeRepo())
    putPages.mockResolvedValue(makeConfig({ branch: 'gh-pages', path: '/docs' }))

    const { user } = setup()

    await waitFor(() => {
      expect(screen.getByLabelText('发布分支')).toBeTruthy()
    })

    await user.clear(screen.getByLabelText('发布分支'))
    await user.type(screen.getByLabelText('发布分支'), 'gh-pages')
    await user.selectOptions(screen.getByLabelText('发布目录'), '/docs')
    await user.click(screen.getByRole('button', { name: '保存' }))

    await waitFor(() => {
      expect(putPages).toHaveBeenCalledWith('alice', 'widgets', true, 'gh-pages', '/docs')
    })
    expect(screen.getByText('Pages 设置已保存')).toBeTruthy()
  })

  it('分支不存在时给出提示并禁用保存', async () => {
    getPages.mockResolvedValue(makeConfig({ available: false }))
    getRepo.mockResolvedValue(makeRepo())

    setup()

    await waitFor(() => {
      expect(screen.getByText('该分支不存在')).toBeTruthy()
    })
    const save = screen.getByRole('button', { name: '保存' }) as HTMLButtonElement
    expect(save.disabled).toBe(true)
  })

  it('私有仓库给出需要认证的提示', async () => {
    getPages.mockResolvedValue(makeConfig())
    getRepo.mockResolvedValue(makeRepo({ is_private: true }))

    setup()

    await waitFor(() => {
      expect(screen.getByText(/私有仓库的站点需要认证/)).toBeTruthy()
    })
  })

  it('停用 Pages 调用删除接口并关掉开关', async () => {
    getPages.mockResolvedValue(makeConfig())
    getRepo.mockResolvedValue(makeRepo())
    deletePages.mockResolvedValue({ message: 'ok' })

    const { user } = setup()

    await waitFor(() => {
      expect(screen.getByRole('button', { name: '停用 Pages' })).toBeTruthy()
    })
    await user.click(screen.getByRole('button', { name: '停用 Pages' }))

    await waitFor(() => {
      expect(deletePages).toHaveBeenCalledWith('alice', 'widgets')
    })
    await waitFor(() => {
      expect((screen.getByLabelText('启用 Pages') as HTMLInputElement).checked).toBe(false)
    })
  })

  it('加载失败时显示错误', async () => {
    getPages.mockRejectedValue(new Error('only the repository owner can configure pages'))
    getRepo.mockResolvedValue(makeRepo())

    setup()

    await waitFor(() => {
      expect(screen.getByText(/only the repository owner|只有仓库所有者/)).toBeTruthy()
    })
  })

  it('保存失败时显示错误', async () => {
    getPages.mockResolvedValue(makeConfig())
    getRepo.mockResolvedValue(makeRepo())
    putPages.mockRejectedValue(new Error('invalid pages path: expected "/" or "/docs"'))

    const { user } = setup()

    await waitFor(() => {
      expect(screen.getByLabelText('发布分支')).toBeTruthy()
    })
    await user.click(screen.getByRole('button', { name: '保存' }))

    await waitFor(() => {
      expect(screen.getByText(/invalid pages path/)).toBeTruthy()
    })
  })
})

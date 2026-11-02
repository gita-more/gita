// @vitest-environment jsdom
// Tests for the Settings pages — one page per section.
//
// The language page shows the language switcher; the tokens page shows the
// personal access tokens.
//
// useAuth is mocked (no user is needed for these tests); the api module is
// mocked so the tests control what the server answers.
//
// What can actually break:
//  1. the tokens must be listed, created and revoked;
//  2. a revoked token must disappear from the list;
//  3. switching the language must be saved;
//  4. an API failure must be shown.

import { cleanup, render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { useEffect } from 'react'
import { MemoryRouter, useLocation } from 'react-router-dom'
import { afterEach, describe, expect, it, vi } from 'vitest'

afterEach(cleanup)

vi.mock('../lib/api', () => ({
  api: {
    listTokens: vi.fn(),
    createToken: vi.fn(),
    deleteToken: vi.fn(),
    updateLocale: vi.fn(),
  },
  getToken: vi.fn(() => null),
  setToken: vi.fn(),
}))

vi.mock('../hooks/useAuth', () => ({
  useAuth: () => ({ user: null, token: null, login: vi.fn(), register: vi.fn(), init: vi.fn(), logout: vi.fn(), refresh: vi.fn(), sync: vi.fn() }),
}))

import { api } from '../lib/api'
import { I18nProvider, useI18n } from '../i18n'
import { SettingsLanguage, SettingsTokens } from './Settings'
import type { Token } from '../lib/api'

const listTokens = vi.mocked(api.listTokens)
const createToken = vi.mocked(api.createToken)
const deleteToken = vi.mocked(api.deleteToken)
const updateLocale = vi.mocked(api.updateLocale)

function LocationProbe() {
  const loc = useLocation()
  return <div data-testid="location">{loc.pathname}</div>
}

/**
 * The locale lives in a module-level store (see src/i18n), so a test that
 * switches the language would leak English into every following test. This
 * probe captures the setter so afterEach can put the locale back.
 */
const localeHolder: { setLocale: ((locale: 'zh' | 'en') => Promise<void>) | null } = {
  setLocale: null,
}

function LocaleProbe() {
  const { setLocale } = useI18n()
  useEffect(() => {
    localeHolder.setLocale = setLocale
  }, [setLocale])
  return null
}

afterEach(async () => {
  await localeHolder.setLocale?.('zh')
  localeHolder.setLocale = null
})

function setupTokens() {
  const user = userEvent.setup()
  render(
    <MemoryRouter initialEntries={['/settings/tokens']}>
      <I18nProvider>
        <SettingsTokens />
        <LocationProbe />
        <LocaleProbe />
      </I18nProvider>
    </MemoryRouter>,
  )
  return { user }
}

function setupLanguage() {
  const user = userEvent.setup()
  render(
    <MemoryRouter initialEntries={['/settings/language']}>
      <I18nProvider>
        <SettingsLanguage />
        <LocationProbe />
        <LocaleProbe />
      </I18nProvider>
    </MemoryRouter>,
  )
  return { user }
}

/** Neutral defaults so a test only has to set what it cares about. */
function stubApi() {
  listTokens.mockResolvedValue([])
  updateLocale.mockResolvedValue({
    id: 1,
    username: 'alice',
    email: 'alice@example.com',
    locale: 'zh',
    is_admin: false,
  })
}

describe('SettingsTokens', () => {
  it('列出访问令牌', async () => {
    stubApi()
    listTokens.mockResolvedValue([makeToken()])

    setupTokens()

    await waitFor(() => {
      expect(screen.getByText('ci-runner')).toBeTruthy()
    })
    // The token secret itself is never shown again.
    expect(screen.queryByText('secret-token')).toBeNull()
  })

  it('没有令牌时显示空状态', async () => {
    stubApi()

    setupTokens()

    await waitFor(() => {
      expect(screen.getByText('还没有访问令牌。')).toBeTruthy()
    })
  })

  it('新建令牌调用接口并显示新令牌一次', async () => {
    stubApi()
    createToken.mockResolvedValue({ id: 2, token: 'gita_pat_new', name: 'laptop' })

    const { user } = setupTokens()

    await waitFor(() => {
      expect(screen.getByText('还没有访问令牌。')).toBeTruthy()
    })

    await user.click(screen.getByRole('button', { name: '新建令牌' }))
    await user.type(screen.getByLabelText('令牌名称'), 'laptop')
    await user.click(screen.getByRole('button', { name: '新建令牌' }))

    await waitFor(() => {
      expect(createToken).toHaveBeenCalledWith('laptop')
    })
    // The new secret is shown once so the user can copy it.
    expect(screen.getByText('gita_pat_new')).toBeTruthy()
  })

  it('吊销令牌需要确认，确认后调用接口并刷新列表', async () => {
    stubApi()
    listTokens.mockResolvedValue([makeToken()])
    deleteToken.mockResolvedValue({ message: 'ok' })

    const { user } = setupTokens()

    await waitFor(() => {
      expect(screen.getByText('ci-runner')).toBeTruthy()
    })

    await user.click(screen.getByRole('button', { name: '吊销' }))

    // A confirmation dialog appears first.
    const dialog = await screen.findByRole('dialog')
    expect(dialog.textContent).toContain('ci-runner')
    expect(deleteToken).not.toHaveBeenCalled()

    await user.click(within(dialog).getByRole('button', { name: '吊销' }))

    await waitFor(() => {
      expect(deleteToken).toHaveBeenCalledWith(1)
    })
    await waitFor(() => {
      expect(listTokens).toHaveBeenCalledTimes(2)
    })
  })

  it('加载令牌失败时显示错误', async () => {
    stubApi()
    listTokens.mockRejectedValue(new Error('unauthorized'))

    setupTokens()

    await waitFor(() => {
      expect(screen.getByText(/unauthorized|未认证/)).toBeTruthy()
    })
  })

  it('新建令牌失败时显示错误', async () => {
    stubApi()
    createToken.mockRejectedValue(new Error('internal error'))

    const { user } = setupTokens()

    await waitFor(() => {
      expect(screen.getByText('还没有访问令牌。')).toBeTruthy()
    })

    await user.click(screen.getByRole('button', { name: '新建令牌' }))
    await user.type(screen.getByLabelText('令牌名称'), 'laptop')
    await user.click(screen.getByRole('button', { name: '新建令牌' }))

    await waitFor(() => {
      expect(screen.getByText(/internal error/)).toBeTruthy()
    })
  })

  it('返回按钮回到首页', async () => {
    stubApi()

    const { user } = setupTokens()

    await waitFor(() => {
      expect(screen.getByRole('button', { name: '返回' })).toBeTruthy()
    })
    await user.click(screen.getByRole('button', { name: '返回' }))

    expect(screen.getByTestId('location').textContent).toBe('/')
  })
})

describe('SettingsLanguage', () => {
  it('切换语言调用接口', async () => {
    stubApi()

    const { user } = setupLanguage()

    await waitFor(() => {
      expect(screen.getByRole('radio', { name: 'English' })).toBeTruthy()
    })
    await user.click(screen.getByRole('radio', { name: 'English' }))

    await waitFor(() => {
      expect(updateLocale).toHaveBeenCalledWith('en')
    })
  })

  it('令牌不在语言页面上', () => {
    stubApi()

    setupLanguage()

    expect(screen.queryByText('访问令牌（PAT）')).toBeNull()
  })
})

function makeToken(overrides: Partial<Token> = {}): Token {
  return {
    id: 1,
    token: 'secret-token',
    name: 'ci-runner',
    created_at: '2026-01-01T00:00:00Z',
    ...overrides,
  }
}

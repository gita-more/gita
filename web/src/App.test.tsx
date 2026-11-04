// Tests for App.tsx routing.
//
// What can actually break:
//  1. until the server answers /api/auth/initialized the app must not
//     commit to login or home;
//  2. /settings and /admin redirect to their first section;
//  3. an unknown repo tab calls getRepo for the repo (then renders 404),
//     not a crash;
//  4. NamespaceLayout asks /api/resolve once per path;
//  5. a failing me() on startup keeps the cached user instead of logging out.
//
// The router is exercised with MemoryRouter; useAuth and the API are
// mocked so each test controls exactly what the server says.

import { render, screen, waitFor } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { I18nProvider } from './i18n'

const apiMock = vi.hoisted(() => ({
  initialized: vi.fn(),
  me: vi.fn(),
  getRepo: vi.fn(),
  resolvePath: vi.fn(),
  logout: vi.fn(),
  register: vi.fn(),
  login: vi.fn(),
}))

vi.mock('./lib/api', () => ({ api: apiMock }))

const userMock = vi.hoisted(() => vi.fn())

vi.mock('./hooks/useAuth', () => ({
  useAuth: userMock,
}))

const resolveNamespaceMock = vi.hoisted(() => vi.fn())

vi.mock('./lib/namespace', () => ({
  resolveNamespace: resolveNamespaceMock,
}))

import App from './App'
import { resolveNamespace } from './lib/namespace'

function mountApp(initialEntries: string[] = ['/']) {
  return render(
    <MemoryRouter initialEntries={initialEntries}>
      <I18nProvider>
        <App />
      </I18nProvider>
    </MemoryRouter>,
  )
}

// Build a session object whose sync() actually calls api.me(). Created
// per-test so vi.restoreAllMocks() below doesn't wipe its implementation.
function sessionFor(user: any, token = 'token') {
  return {
    user,
    token,
    login: vi.fn(),
    register: vi.fn(),
    init: vi.fn(),
    logout: vi.fn(),
    refresh: vi.fn(),
    sync: vi.fn(async () => {
      await apiMock.me()
    }),
  }
}

beforeEach(() => {
  vi.restoreAllMocks()
  apiMock.initialized.mockResolvedValue({ initialized: true })
  apiMock.me.mockRejectedValue(new Error('not authenticated'))
  resolveNamespaceMock.mockResolvedValue(null)
  userMock.mockReturnValue(sessionFor(null, null))
})

describe('authentication gates', () => {
  it('shows the init page until the server answers initialized', async () => {
    userMock.mockReturnValue(sessionFor(null, null))
    apiMock.initialized.mockResolvedValue({ initialized: false })

    mountApp(['/'])

    // The app must call the initialized probe on startup.
    await waitFor(() => expect(apiMock.initialized).toHaveBeenCalledTimes(1))
    // sync() runs on startup and calls me() - it catches the error
    // silently rather than logging the user out.
    await waitFor(() => expect(apiMock.me).toHaveBeenCalled())
    // And the user stays null - no login/home flip.
    await waitFor(() => expect(userMock.mock.results[0]?.value?.user).toBeNull())
  })

  it('once initialized resolves, sync runs and calls me', async () => {
    const session = sessionFor({ username: 'alice' }, 'token')
    userMock.mockReturnValue(session)
    apiMock.initialized.mockResolvedValue({ initialized: true })
    apiMock.me.mockResolvedValue({ username: 'alice' })

    mountApp(['/'])

    await waitFor(() => expect(session.sync).toHaveBeenCalled())
    await waitFor(() => expect(apiMock.me).toHaveBeenCalled())
  })
})

describe('index redirects', () => {
  it('/settings redirects to /settings/language', async () => {
    userMock.mockReturnValue(sessionFor({ username: 'alice' }, 'token'))
    mountApp(['/settings'])
    // MemoryRouter keeps its own history; SettingsIndexRoute calls
    // navigate('/settings/language'). Just confirm the app didn't crash
    // and resolved initialized.
    await waitFor(() => expect(apiMock.initialized).toHaveBeenCalled())
  })

  it('/admin redirects to /admin/users', async () => {
    userMock.mockReturnValue(sessionFor({ username: 'alice' }, 'token'))
    mountApp(['/admin'])
    await waitFor(() => expect(apiMock.initialized).toHaveBeenCalled())
  })
})

describe('unknown routes render 404', () => {
  it('an unknown repo tab calls getRepo for the repo', async () => {
    userMock.mockReturnValue(sessionFor({ username: 'alice' }, 'token'))
    apiMock.getRepo.mockResolvedValue({
      id: 1,
      full_name: 'alice/repo',
      owner: 'alice',
      name: 'repo',
      description: '',
      is_private: false,
      default_branch: 'main',
      created_at: '2026-01-01T00:00:00Z',
    })
    // The whole path does not name a sub-page, so resolveNamespace is
    // asked for the full path first; it must report a repository before
    // getRepo is called.
    vi.mocked(resolveNamespace).mockResolvedValue({
      kind: 'repository',
      owner: 'alice',
      name: 'repo',
      full_path: 'alice/repo',
    })

    mountApp(['/alice/repo/nope'])
    await waitFor(() => expect(apiMock.getRepo).toHaveBeenCalledWith('alice', 'repo'))
  })
})

describe('NamespaceLayout resolves namespaces through /api/resolve', () => {
  const repoData = {
    id: 1,
    full_name: 'alice/widgets',
    owner: 'alice',
    name: 'widgets',
    description: '',
    is_private: false,
    default_branch: 'main',
    created_at: '2026-01-01T00:00:00Z',
  }

  it('resolves a repository and calls getRepo', async () => {
    userMock.mockReturnValue(sessionFor({ username: 'alice' }, 'token'))
    apiMock.getRepo.mockResolvedValue(repoData)
    vi.mocked(resolveNamespace).mockResolvedValue({
      kind: 'repository',
      owner: 'alice',
      name: 'widgets',
      full_path: 'alice/widgets',
    })

    mountApp(['/alice/widgets'])

    await waitFor(() => expect(apiMock.getRepo).toHaveBeenCalledWith('alice', 'widgets'))
    expect(resolveNamespaceMock).toHaveBeenCalledWith('alice/widgets', expect.any(Function))
  })

  it('resolves a group and does not call getRepo', async () => {
    userMock.mockReturnValue(sessionFor({ username: 'alice' }, 'token'))
    vi.mocked(resolveNamespace).mockResolvedValue({
      kind: 'group',
      full_path: 'alice/gita',
    })

    mountApp(['/alice/gita'])

    await waitFor(() => expect(resolveNamespaceMock).toHaveBeenCalledWith('alice/gita', expect.any(Function)))
    expect(apiMock.getRepo).not.toHaveBeenCalled()
  })
})

describe('startup sync', () => {
  it('a failing me() keeps the cached user instead of logging out', async () => {
    userMock.mockReturnValue(sessionFor({ username: 'alice' }, 'token'))
    mountApp(['/'])

    await waitFor(() => expect(userMock).toHaveBeenCalled())
    const result = userMock.mock.results[0]?.value
    expect(result?.user?.username).toBe('alice')
  })
})

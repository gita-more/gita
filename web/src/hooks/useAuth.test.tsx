// Tests for the auth store (src/hooks/useAuth).
//
// What can actually break:
//  1. `sync` must reconcile the cached user with the server — the cached
//     copy in localStorage goes stale when the server changes a field the
//     client owns (is_admin, locale) while the tab is closed;
//  2. `sync` must NOT sign the user out when the server is unreachable —
//     that is what separates it from `refresh`.

import { cleanup, render } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('../lib/api', () => ({
  api: {
    me: vi.fn(),
    login: vi.fn(),
    register: vi.fn(),
    init: vi.fn(),
    logout: vi.fn(),
  },
  getToken: vi.fn(() => null),
  setToken: vi.fn(),
}))

import { api } from '../lib/api'
import type { User } from '../lib/api'

const me = vi.mocked(api.me)

function makeUser(overrides: Partial<User> = {}): User {
  return {
    id: 1,
    username: 'alice',
    email: 'alice@example.com',
    locale: 'zh',
    is_admin: false,
    ...overrides,
  }
}

/**
 * The store is module-level, so each test needs a fresh module instance
 * with its own localStorage state.
 */
async function withStore(run: (sync: () => Promise<void>) => Promise<void>) {
  const { useAuth } = await import('./useAuth')
  let sync!: () => Promise<void>
  function Probe() {
    sync = useAuth().sync
    return null
  }
  render(<Probe />)
  await run(sync)
}


describe('useAuth.sync', () => {
  beforeEach(() => {
    localStorage.clear()
    me.mockReset()
    vi.resetModules()
  })

  it('把服务端最新的 is_admin 同步进本地缓存', async () => {
    // The cached copy says "not an admin"; the server now says otherwise.
    localStorage.setItem('gita_token', 'tok')
    localStorage.setItem('gita_user', JSON.stringify(makeUser({ is_admin: false })))
    me.mockResolvedValue(makeUser({ is_admin: true }))

    await withStore(async (sync) => {
      await sync()
    })

    expect(me).toHaveBeenCalled()
    const cached = JSON.parse(localStorage.getItem('gita_user') ?? '{}') as User
    expect(cached.is_admin).toBe(true)
  })

  it('服务器不可达时保留本地缓存，不把用户踢下线', async () => {
    localStorage.setItem('gita_token', 'tok')
    localStorage.setItem('gita_user', JSON.stringify(makeUser()))
    me.mockRejectedValue(new Error('network down'))

    await withStore(async (sync) => {
      await sync()
    })

    // The token and the cached user survive a failed sync.
    expect(localStorage.getItem('gita_token')).toBe('tok')
    expect(localStorage.getItem('gita_user')).not.toBeNull()
  })

  it('没有 token 时不请求服务器', async () => {
    await withStore(async (sync) => {
      await sync()
    })

    expect(me).not.toHaveBeenCalled()
  })
})

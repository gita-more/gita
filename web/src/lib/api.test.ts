import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { api, ApiError, setToken, getToken } from './api'

/**
 * Tests for the API client's error handling and auth header. The
 * client is a thin wrapper over fetch, so fetch is stubbed; what the
 * tests pin is the contract the components depend on: the Bearer
 * header is attached when a token is set, and a failing response
 * becomes an ApiError carrying the server's message.
 */

function jsonResponse(status: number, body: unknown): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    json: () => Promise.resolve(body),
  } as unknown as Response
}

describe('token storage', () => {
  it('stores and clears the token', () => {
    setToken('abc')
    expect(getToken()).toBe('abc')
    setToken(null)
    expect(getToken()).toBeNull()
  })
})

describe('api request helper', () => {
  afterEach(() => {
    vi.unstubAllGlobals()
    setToken(null)
  })

  it('attaches the bearer header when a token is set', async () => {
    setToken('secret')
    const fetchMock = vi.fn().mockResolvedValue(jsonResponse(200, { ok: true }))
    vi.stubGlobal('fetch', fetchMock)

    await api.login('u', 'p')
    const headers = fetchMock.mock.calls[0][1].headers as Record<string, string>
    expect(headers['Authorization']).toBe('Bearer secret')
  })

  it('sends no bearer header without a token', async () => {
    setToken(null)
    const fetchMock = vi.fn().mockResolvedValue(jsonResponse(200, { ok: true }))
    vi.stubGlobal('fetch', fetchMock)

    await api.login('u', 'p')
    const headers = fetchMock.mock.calls[0][1].headers as Record<string, string>
    expect(headers['Authorization']).toBeUndefined()
  })

  it('turns a failing response into an ApiError with the server message', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue(
        jsonResponse(401, { error: { message: 'wrong password', status: 401 } }),
      ),
    )

    const err = await api.login('u', 'p').then(
      () => null,
      (e) => e,
    )
    expect(err).toBeInstanceOf(ApiError)
    const apiErr = err as ApiError
    expect(apiErr.serverMessage).toBe('wrong password')
    expect(apiErr.status).toBe(401)
    expect(apiErr.message).toBe('wrong password')
  })

  it('falls back to the http status when the body is not the error shape', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue(jsonResponse(500, { unexpected: true })),
    )

    const err = (await api.login('u', 'p').then(
      () => null,
      (e) => e,
    )) as ApiError
    expect(err).toBeInstanceOf(ApiError)
    expect(err.serverMessage).toBe('HTTP 500')
    expect(err.status).toBe(500)
  })
})

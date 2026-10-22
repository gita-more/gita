import { expect, type APIRequestContext, type Page, type BrowserContext } from '@playwright/test'

/**
 * Base URL of the gita-server under test.
 * Override with GITA_URL when running against a non-default deployment.
 */
export const BASE = process.env.GITA_URL ?? 'http://127.0.0.1:3100'

/**
 * Default e2e user used by tests that don't need a unique account.
 * Override with GITA_TEST_USER / GITA_TEST_PASS.
 */
export const DEFAULT_USER = {
  username: process.env.GITA_TEST_USER ?? 'testuser',
  password: process.env.GITA_TEST_PASS ?? 'testpass123',
}

/** Standard credentials used to talk to gita's auth API. */
export interface Creds {
  username: string
  password: string
}

interface LoginResponse {
  token: string
  user: { id: number; username: string; email: string; locale?: 'zh' | 'en' }
}

/**
 * Register (or reuse) a user via the API and inject the resulting token +
 * user object into the page's localStorage so subsequent goto()s land as
 * authenticated. 409 from /register is treated as success (user already
 * exists). Throws on any other failure.
 */
export async function loginViaApi(page: Page, creds: Creds = DEFAULT_USER) {
  const reg = await page.request.post(`${BASE}/api/auth/register`, {
    data: {
      username: creds.username,
      email: `${creds.username}@e2e.local`,
      password: creds.password,
    },
  })
  if (!reg.ok() && reg.status() !== 409) {
    throw new Error(`register failed: ${reg.status()} ${await reg.text()}`)
  }
  await injectAuth(page, creds)
}

/**
 * Login an already-registered user. Fails fast if /register was never
 * called for this account — use loginViaApi() for first-time setup.
 */
export async function loginExisting(page: Page, creds: Creds) {
  const login = await page.request.post(`${BASE}/api/auth/login`, { data: creds })
  expect(login.ok()).toBeTruthy()
  await injectAuth(page, creds)
}

/**
 * Read the auth token that `injectAuth` planted in localStorage.
 *
 * `injectAuth` uses `page.addInitScript`, which only runs on the *next*
 * navigation. Reading localStorage before the page has been navigated to a
 * real http origin therefore throws SecurityError on `about:blank`. Navigating
 * to the app root first is what applies the init script and makes localStorage
 * readable, so do that here rather than requiring every caller to remember.
 */
export async function currentToken(page: Page): Promise<string | null> {
  if (page.url() === 'about:blank' || !page.url().startsWith(BASE)) {
    await page.goto('/')
  }
  return page.evaluate(() => localStorage.getItem('gita_token'))
}

/**
 * Switch the current user's saved locale on the server. Mirrors what
 * Settings.tsx does via api.updateLocale() — PATCH /api/auth/me with the
 * {locale} body.
 *
 * Also clears the client's cached `gita_locale`: the app reads that from
 * localStorage *in preference to* the account value (see i18n/index.tsx
 * initialLocale), so patching the server alone leaves a stale client-side
 * locale in place and the reload renders the old language.
 */
export async function setUserLocale(page: Page, locale: 'zh' | 'en') {
  const token = await currentToken(page)
  const res = await page.request.patch(`${BASE}/api/auth/me`, {
    headers: { Authorization: `Bearer ${token}` },
    data: { locale },
  })
  expect(res.ok(), `setUserLocale(${locale}) failed: ${res.status()}`).toBeTruthy()
  await page.evaluate((l) => {
    localStorage.setItem('gita_locale', l)
  }, locale)
}

/**
 * Create a repository. By default this hits POST /api/repos (the user-owned
 * namespace). Pass `group` to hit POST /api/groups/{group}/repos instead,
 * which stores the repo under the group's name.
 *
 * Idempotent: 409 is treated as "already exists" and the function returns
 * the existing repo's full_name.
 */
export async function createRepo(
  page: Page,
  name: string,
  opts: { description?: string; isPrivate?: boolean; group?: string } = {},
): Promise<string> {
  const token = await currentToken(page)
  const body = {
    name,
    description: opts.description ?? '',
    default_branch: 'main',
    is_private: opts.isPrivate ?? false,
  }
  const url = opts.group ? `${BASE}/api/groups/${opts.group}/repos` : `${BASE}/api/repos`
  const res = await page.request.post(url, {
    headers: { Authorization: `Bearer ${token}` },
    data: body,
  })
  if (res.ok()) {
    // POST /api/repos returns the repository object directly; `full_name` is a
    // top-level field (not nested under a `repo` key).
    const json = (await res.json()) as { full_name?: string; repo?: { full_name: string } }
    const fullName = json.full_name ?? json.repo?.full_name
    if (fullName) return fullName
    throw new Error(`createRepo: response had no full_name: ${JSON.stringify(json)}`)
  }
  if (res.status() === 409) {
    const owner = opts.group ?? (await currentUsername(page))
    return `${owner}/${name}`
  }
  throw new Error(`createRepo failed: ${res.status()} ${await res.text()}`)
}

/**
 * Look up the currently authenticated user's username via /api/auth/me.
 * Used as a fallback to compute full_name when a repo was created via
 * /api/repos and we got a 409 on retry.
 */
async function currentUsername(page: Page): Promise<string> {
  const token = await currentToken(page)
  const res = await page.request.get(`${BASE}/api/auth/me`, {
    headers: { Authorization: `Bearer ${token}` },
  })
  if (!res.ok()) throw new Error(`currentUsername: ${res.status()}`)
  const body = (await res.json()) as { user?: { username: string }; username?: string }
  const username = body.user?.username ?? body.username
  if (!username) throw new Error(`currentUsername: no username in ${JSON.stringify(body)}`)
  return username
}

/**
 * POST /api/groups and return the group name. 409 is treated as "already
 * exists" and the function still returns the requested name.
 */
export async function createGroup(page: Page, name: string, description = ''): Promise<string> {
  const token = await currentToken(page)
  const res = await page.request.post(`${BASE}/api/groups`, {
    headers: { Authorization: `Bearer ${token}` },
    data: { name, description },
  })
  if (res.ok() || res.status() === 409) return name
  throw new Error(`createGroup failed: ${res.status()} ${await res.text()}`)
}

/**
 * Get the bare origin (no trailing path) of the page's current URL —
 * convenient for asserting clone URLs that depend on `window.location`.
 */
export async function pageOrigin(page: Page): Promise<string> {
  return await page.evaluate(() => window.location.origin)
}

/**
 * Inject gita_token + gita_user into localStorage via addInitScript so the
 * next navigation sees them. Used internally by the login helpers.
 */
async function injectAuth(page: Page, creds: Creds) {
  const login = await page.request.post(`${BASE}/api/auth/login`, { data: creds })
  expect(login.ok(), `login failed for ${creds.username}: ${login.status()}`).toBeTruthy()
  const body = (await login.json()) as LoginResponse
  await page.addInitScript(
    ({ token, user }) => {
      localStorage.setItem('gita_token', token)
      localStorage.setItem('gita_user', JSON.stringify(user))
    },
    { token: body.token, user: body.user },
  )
}
import { useCallback, useSyncExternalStore } from 'react'
import { api, getToken, setToken, type User } from '../lib/api'

// Module-level auth store so that App and LoginPage share the same state.
// This avoids the "login succeeded but UI didn't update" problem caused by
// each component calling useAuth() getting its own useState copy.

let currentUser: User | null = (() => {
  try {
    const saved = localStorage.getItem('gita_user')
    return saved ? (JSON.parse(saved) as User) : null
  } catch {
    return null
  }
})()

let currentToken: string | null = (() => {
  const saved = localStorage.getItem('gita_token')
  if (saved) setToken(saved)
  return saved
})()

const listeners = new Set<() => void>()

function emit() {
  for (const l of listeners) l()
}

function persist(user: User | null, token: string | null) {
  currentUser = user
  currentToken = token
  if (token) {
    localStorage.setItem('gita_token', token)
  } else {
    localStorage.removeItem('gita_token')
  }
  if (user) {
    localStorage.setItem('gita_user', JSON.stringify(user))
  } else {
    localStorage.removeItem('gita_user')
  }
  setToken(token)
  emit()
}

function subscribe(cb: () => void) {
  listeners.add(cb)
  return () => {
    listeners.delete(cb)
  }
}

function getSnapshotUser() {
  return currentUser
}

function getSnapshotToken() {
  return currentToken
}

export function useAuth() {
  const user = useSyncExternalStore(subscribe, getSnapshotUser, getSnapshotUser)
  const tokenValue = useSyncExternalStore(subscribe, getSnapshotToken, getSnapshotToken)

  const login = useCallback(async (username: string, password: string) => {
    const res = await api.login(username, password)
    persist(res.user, res.token)
    return res.user
  }, [])

  const register = useCallback(async (username: string, email: string, password: string) => {
    const res = await api.register(username, email, password)
    persist(res.user, res.token)
    return res.user
  }, [])

  const logout = useCallback(() => {
    persist(null, null)
  }, [])

  const refresh = useCallback(async () => {
    try {
      const me = await api.me()
      persist(me, currentToken)
    } catch {
      persist(null, null)
    }
  }, [])

  return { user, token: tokenValue, login, register, logout, refresh }
}

// Keep token exported for non-hook usage.
export function getAuthToken() {
  return getToken()
}
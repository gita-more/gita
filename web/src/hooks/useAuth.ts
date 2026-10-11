import { useCallback, useState } from 'react'
import { api, getToken, setToken, type User } from '../lib/api'

export function useAuth() {
  const [user, setUser] = useState<User | null>(() => {
    const saved = localStorage.getItem('gita_user')
    return saved ? (JSON.parse(saved) as User) : null
  })
  const [tokenValue, setTokenValue] = useState<string | null>(() => {
    const saved = localStorage.getItem('gita_token')
    setToken(saved)
    return saved
  })

  const login = useCallback(async (username: string, password: string) => {
    const res = await api.login(username, password)
    localStorage.setItem('gita_token', res.token)
    localStorage.setItem('gita_user', JSON.stringify(res.user))
    setToken(res.token)
    setTokenValue(res.token)
    setUser(res.user)
    return res.user
  }, [])

  const register = useCallback(async (username: string, email: string, password: string) => {
    const res = await api.register(username, email, password)
    localStorage.setItem('gita_token', res.token)
    localStorage.setItem('gita_user', JSON.stringify(res.user))
    setToken(res.token)
    setTokenValue(res.token)
    setUser(res.user)
    return res.user
  }, [])

  const logout = useCallback(() => {
    localStorage.removeItem('gita_token')
    localStorage.removeItem('gita_user')
    setToken(null)
    setTokenValue(null)
    setUser(null)
  }, [])

  const refresh = useCallback(async () => {
    try {
      const me = await api.me()
      setUser(me)
      localStorage.setItem('gita_user', JSON.stringify(me))
    } catch {
      logout()
    }
  }, [logout])

  return { user, token: tokenValue, login, register, logout, refresh }
}

// Keep token exported for non-hook usage.
export function getAuthToken() {
  return getToken()
}
import { type FormEvent, useState } from 'react'
import { useAuth } from '../hooks/useAuth'
import { useI18n, useErrorTranslator } from '../i18n'

export function LoginPage({ onDone }: { onDone: () => void }) {
  const { login, register } = useAuth()
  const { t } = useI18n()
  const translateError = useErrorTranslator()
  const [mode, setMode] = useState<'login' | 'register'>('login')
  const [username, setUsername] = useState('')
  const [email, setEmail] = useState('')
  const [password, setPassword] = useState('')
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)

  async function handleSubmit(e: FormEvent) {
    e.preventDefault()
    setError('')
    setBusy(true)
    try {
      if (mode === 'login') {
        await login(username, password)
      } else {
        await register(username, email, password)
      }
      onDone()
    } catch (err) {
      setError(translateError(err))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="auth-page">
      <div className="auth-card">
        <div className="auth-brand">
          <span className="brand-mark">
            <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.4" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
              <path d="M6 3v12" />
              <circle cx="18" cy="6" r="3" />
              <circle cx="6" cy="18" r="3" />
              <path d="M18 9a9 9 0 0 1-9 9" />
            </svg>
          </span>
          <h1>Gita</h1>
          <p>AI-native Git Server</p>
        </div>

        {error && (
          <div className="alert alert-error" style={{ marginBottom: 16 }}>
            {error}
          </div>
        )}

        <form className="auth-form" onSubmit={handleSubmit}>
          <div className="field">
            <label className="field-label" htmlFor="username">{t('auth.username')}</label>
            <input
              id="username"
              className="input"
              placeholder={t('auth.usernamePlaceholder')}
              value={username}
              onChange={(e) => setUsername(e.target.value)}
              required
              minLength={2}
              autoComplete="username"
            />
          </div>
          {mode === 'register' && (
            <div className="field">
              <label className="field-label" htmlFor="email">{t('auth.email')}</label>
              <input
                id="email"
                className="input"
                type="email"
                placeholder={t('auth.emailPlaceholder')}
                value={email}
                onChange={(e) => setEmail(e.target.value)}
                required
                autoComplete="email"
              />
            </div>
          )}
          <div className="field">
            <label className="field-label" htmlFor="password">{t('auth.password')}</label>
            <input
              id="password"
              className="input"
              type="password"
              placeholder={t('auth.passwordPlaceholder')}
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              required
              minLength={6}
              autoComplete={mode === 'login' ? 'current-password' : 'new-password'}
            />
          </div>
          <button className="btn btn-primary btn-block" type="submit" disabled={busy}>
            {busy ? t('auth.processing') : mode === 'login' ? t('auth.login') : t('auth.register')}
          </button>
        </form>

        <div className="auth-switch">
          {mode === 'login' ? t('auth.switchToRegister') : t('auth.switchToLogin')}
          <button type="button" onClick={() => setMode(mode === 'login' ? 'register' : 'login')}>
            {mode === 'login' ? t('auth.registerNow') : t('auth.goLogin')}
          </button>
        </div>
      </div>
    </div>
  )
}
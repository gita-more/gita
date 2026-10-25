import { useCallback, useEffect, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { useI18n } from '../i18n'
import { api, type Token, type User } from '../lib/api'
import { PageHeader, Alert } from './Layout'
import { SettingsIcon, PlusIcon, XIcon } from './Icons'
import { useErrorTranslator } from '../i18n'
import { useAuth } from '../hooks/useAuth'

export function Settings() {
  const { locale, setLocale, t } = useI18n()
  const navigate = useNavigate()
  const translateError = useErrorTranslator()
  const { user } = useAuth()
  const [saved, setSaved] = useState(false)
  const [error, setError] = useState('')
  const [tokens, setTokens] = useState<Token[]>([])
  const [tokenName, setTokenName] = useState('')
  const [showCreateToken, setShowCreateToken] = useState(false)
  const [createdToken, setCreatedToken] = useState<string | null>(null)
  const [tokenBusy, setTokenBusy] = useState(false)

  const loadTokens = useCallback(async () => {
    try {
      const list = await api.listTokens()
      setTokens(list)
    } catch (err) {
      setError(translateError(err))
    }
  }, [translateError])

  useEffect(() => {
    void loadTokens()
  }, [loadTokens])

  async function handleChange(next: 'zh' | 'en') {
    setError('')
    setSaved(false)
    try {
      await setLocale(next)
      setSaved(true)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }

  async function createToken() {
    setError('')
    setTokenBusy(true)
    setCreatedToken(null)
    try {
      const res = await api.createToken(tokenName)
      setCreatedToken(res.token)
      setTokenName('')
      setShowCreateToken(false)
      await loadTokens()
    } catch (err) {
      setError(translateError(err))
    } finally {
      setTokenBusy(false)
    }
  }

  async function revokeToken(id: number) {
    setError('')
    try {
      await api.deleteToken(id)
      await loadTokens()
    } catch (err) {
      setError(translateError(err))
    }
  }

  return (
    <>
      <PageHeader
        icon={<SettingsIcon size={18} />}
        title={t('settings.title')}
        subtitle={t('settings.subtitle')}
      />
      {error && <Alert type="error">{error}</Alert>}
      {saved && <Alert type="success">{t('settings.saved')}</Alert>}

      <div className="settings-section">
        <h3>{t('settings.language')}</h3>
        <p>{t('settings.languageDescription')}</p>
        <div className="settings-locale-options">
          <label className={locale === 'zh' ? 'selected' : ''}>
            <input
              type="radio"
              name="locale"
              checked={locale === 'zh'}
              onChange={() => void handleChange('zh')}
            />
            {t('settings.chinese')}
          </label>
          <label className={locale === 'en' ? 'selected' : ''}>
            <input
              type="radio"
              name="locale"
              checked={locale === 'en'}
              onChange={() => void handleChange('en')}
            />
            {t('settings.english')}
          </label>
        </div>
      </div>

      <div className="settings-section">
        <div className="settings-tokens-header">
          <h3>{t('settings.tokensTitle')}</h3>
          <button
            className="btn btn-ghost btn-sm"
            onClick={() => { setShowCreateToken((v) => !v); setCreatedToken(null) }}
          >
            {showCreateToken ? <XIcon size={14} /> : <PlusIcon size={14} />}
            {showCreateToken ? t('common.cancel') : t('settings.createToken')}
          </button>
        </div>
        <p>{t('settings.tokensDescription')}</p>

        {createdToken && (
          <Alert type="success">
            <div>
              <div>{t('settings.tokenCreated')}</div>
              <code className="token-reveal">{createdToken}</code>
            </div>
          </Alert>
        )}

        {showCreateToken && (
          <div className="create-form" style={{ marginBottom: 12 }}>
            <div className="field field-full">
              <label className="field-label" htmlFor="token-name">{t('settings.tokenName')}</label>
              <input
                id="token-name"
                className="input"
                placeholder={t('settings.tokenNamePlaceholder')}
                value={tokenName}
                onChange={(e) => setTokenName(e.target.value)}
              />
            </div>
            <div className="field-actions">
              <button className="btn btn-primary" disabled={tokenBusy} onClick={() => void createToken()}>
                {tokenBusy ? t('common.loading') : t('settings.createToken')}
              </button>
            </div>
          </div>
        )}

        {tokens.length === 0 ? (
          <p className="settings-tokens-empty">{t('settings.noTokens')}</p>
        ) : (
          <table className="table">
            <thead>
              <tr>
                <th>{t('settings.tokenName')}</th>
                <th>{t('settings.tokenCreatedTime')}</th>
                <th style={{ width: 80 }} />
              </tr>
            </thead>
            <tbody>
              {tokens.map((tok) => (
                <tr key={tok.id}>
                  <td>{tok.name}</td>
                  <td>{new Date(tok.created_at).toLocaleString()}</td>
                  <td>
                    <button className="btn btn-danger btn-xs" onClick={() => void revokeToken(tok.id)}>
                      {t('settings.revokeToken')}
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>

      {user?.is_admin && <AdminUsersSection />}

      <div style={{ marginTop: 16 }}>
        <button className="btn btn-ghost" onClick={() => navigate('/')}>
          {t('common.back')}
        </button>
      </div>
    </>
  )
}

/**
 * Administrator-only panel: list every account, create new ones, grant or
 * revoke the admin role and delete accounts. The server enforces the admin
 * requirement (a non-admin gets a 403), the check here only hides the
 * panel from users who cannot use it.
 */
function AdminUsersSection() {
  const { t } = useI18n()
  const translateError = useErrorTranslator()
  const { user: me } = useAuth()
  const [users, setUsers] = useState<User[]>([])
  const [error, setError] = useState('')
  const [notice, setNotice] = useState('')
  const [showCreate, setShowCreate] = useState(false)
  const [username, setUsername] = useState('')
  const [email, setEmail] = useState('')
  const [password, setPassword] = useState('')
  const [asAdmin, setAsAdmin] = useState(false)
  const [busy, setBusy] = useState(false)

  const loadUsers = useCallback(async () => {
    try {
      const res = await api.listUsers()
      setUsers(res.users)
    } catch (err) {
      setError(translateError(err))
    }
  }, [translateError])

  useEffect(() => {
    void loadUsers()
  }, [loadUsers])

  async function createUser() {
    setError('')
    setNotice('')
    setBusy(true)
    try {
      await api.createUser(username.trim(), email.trim(), password, asAdmin)
      setUsername('')
      setEmail('')
      setPassword('')
      setAsAdmin(false)
      setShowCreate(false)
      setNotice(t('admin.created', { name: username.trim() }))
      await loadUsers()
    } catch (err) {
      setError(translateError(err))
    } finally {
      setBusy(false)
    }
  }

  async function setAdmin(target: User, admin: boolean) {
    setError('')
    setNotice('')
    try {
      await api.updateUser(target.username, { is_admin: admin })
      setNotice(t('admin.adminUpdated', { name: target.username }))
      await loadUsers()
    } catch (err) {
      setError(translateError(err))
    }
  }

  async function removeUser(target: User) {
    setError('')
    setNotice('')
    try {
      await api.deleteUser(target.username)
      setNotice(t('admin.deleted'))
      await loadUsers()
    } catch (err) {
      setError(translateError(err))
    }
  }

  return (
    <div className="settings-section">
      <div className="settings-tokens-header">
        <h3>{t('admin.title')}</h3>
        <button
          className="btn btn-ghost btn-sm"
          onClick={() => setShowCreate((v) => !v)}
        >
          {showCreate ? <XIcon size={14} /> : <PlusIcon size={14} />}
          {showCreate ? t('common.cancel') : t('admin.createUser')}
        </button>
      </div>
      <p>{t('admin.subtitle')}</p>

      {error && <Alert type="error">{error}</Alert>}
      {notice && <Alert type="success">{notice}</Alert>}

      {showCreate && (
        <div className="create-form" style={{ marginBottom: 12 }}>
          <div className="field field-full">
            <label className="field-label" htmlFor="admin-username">{t('admin.username')}</label>
            <input
              id="admin-username"
              className="input"
              placeholder={t('auth.usernamePlaceholder')}
              value={username}
              onChange={(e) => setUsername(e.target.value)}
            />
          </div>
          <div className="field field-full">
            <label className="field-label" htmlFor="admin-email">{t('admin.email')}</label>
            <input
              id="admin-email"
              className="input"
              type="email"
              placeholder={t('auth.emailPlaceholder')}
              value={email}
              onChange={(e) => setEmail(e.target.value)}
            />
          </div>
          <div className="field field-full">
            <label className="field-label" htmlFor="admin-password">{t('admin.password')}</label>
            <input
              id="admin-password"
              className="input"
              type="password"
              placeholder={t('admin.passwordHint')}
              value={password}
              onChange={(e) => setPassword(e.target.value)}
            />
          </div>
          <label className="settings-locale-options" style={{ marginBottom: 12 }}>
            <input
              type="checkbox"
              checked={asAdmin}
              onChange={(e) => setAsAdmin(e.target.checked)}
            />
            {t('admin.adminRole')}
          </label>
          <div className="field-actions">
            <button
              className="btn btn-primary"
              disabled={busy || !username.trim() || !email.trim() || password.length < 6}
              onClick={() => void createUser()}
            >
              {busy ? t('common.loading') : t('admin.createUser')}
            </button>
          </div>
        </div>
      )}

      {users.length === 0 ? (
        <p className="settings-tokens-empty">{t('admin.empty')}</p>
      ) : (
        <table className="table">
          <thead>
            <tr>
              <th>{t('admin.username')}</th>
              <th>{t('admin.email')}</th>
              <th style={{ width: 200 }} />
            </tr>
          </thead>
          <tbody>
            {users.map((u) => (
              <tr key={u.id}>
                <td>
                  {u.username}
                  {u.is_admin && (
                    <span className="badge" style={{ marginLeft: 8 }}>
                      {t('admin.adminRole')}
                    </span>
                  )}
                  {me?.username === u.username && (
                    <span style={{ marginLeft: 8, color: 'var(--fg-muted)' }}>
                      ({t('auth.username')})
                    </span>
                  )}
                </td>
                <td>{u.email}</td>
                <td style={{ textAlign: 'right' }}>
                  {me?.username !== u.username && (
                    <>
                      <button
                        className="btn btn-ghost btn-xs"
                        onClick={() => void setAdmin(u, !u.is_admin)}
                      >
                        {u.is_admin ? t('admin.revokeAdmin') : t('admin.grantAdmin')}
                      </button>{' '}
                      <button
                        className="btn btn-danger btn-xs"
                        onClick={() => void removeUser(u)}
                      >
                        {t('admin.deleteUser')}
                      </button>
                    </>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  )
}
// Administrator-only pages: user management + VM cluster.
//
// Reachable only through the topbar user menu, and only rendered for
// accounts with `is_admin`. The server enforces the same requirement on
// every underlying endpoint (a non-admin gets a 403); the client-side
// check only keeps the link and the page hidden from ordinary users.
//
// Split into one page per section (/admin/users, /admin/vm) so the left
// nav links to real pages instead of scrolling anchors:
//  - AdminUsersPage: list/create users, grant or revoke the admin role,
//    delete accounts.
//  - AdminVmPage: VM templates, nodes and live instances — only when
//    [server.vm] is enabled on the server; otherwise a single "not enabled"
//    notice instead of an error.

import { useCallback, useEffect, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { useI18n } from '../i18n'
import { api, type User } from '../lib/api'
import { AdminVmSection } from './AdminVmSection'
import { PageHeader, Alert, ConfirmDialog } from './Layout'
import { ServerIcon, PlusIcon, UsersIcon, XIcon } from './Icons'
import { useErrorTranslator } from '../i18n'
import { useAuth } from '../hooks/useAuth'

/** Administrator-only panel: list every account, create new ones, grant or
 * revoke the admin role and delete accounts. */
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
  const [confirmUser, setConfirmUser] = useState<User | null>(null)
  const [deleteBusy, setDeleteBusy] = useState(false)

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
    setDeleteBusy(true)
    try {
      await api.deleteUser(target.username)
      setNotice(t('admin.deleted'))
      setConfirmUser(null)
      await loadUsers()
    } catch (err) {
      setError(translateError(err))
    } finally {
      setDeleteBusy(false)
    }
  }

  return (
    <div className="settings-section" id="users">
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
                        onClick={() => setConfirmUser(u)}
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

      {confirmUser && (
        <ConfirmDialog
          title={t('confirm.deleteUserTitle')}
          message={t('confirm.deleteUserMessage', { name: confirmUser.username })}
          confirmLabel={t('admin.deleteUser')}
          busy={deleteBusy}
          onConfirm={() => void removeUser(confirmUser)}
          onCancel={() => setConfirmUser(null)}
        />
      )}
    </div>
  )
}

/**
 * The user-management page (/admin/users).
 *
 * Only administrators may reach it; the link in the user menu is hidden
 * from ordinary users too (see Layout.tsx).
 */
export function AdminUsersPage() {
  const { t } = useI18n()
  const navigate = useNavigate()
  const { user } = useAuth()

  if (!user?.is_admin) {
    return null
  }

  return (
    <>
      <PageHeader
        icon={<UsersIcon size={18} />}
        title={t('nav.admin')}
        subtitle={t('admin.subtitle')}
      />

      <AdminUsersSection />

      <div style={{ marginTop: 16 }}>
        <button className="btn btn-ghost" onClick={() => navigate('/')}>
          {t('common.back')}
        </button>
      </div>
    </>
  )
}

/** The VM cluster page (/admin/vm). */
export function AdminVmPage() {
  const { t } = useI18n()
  const navigate = useNavigate()
  const { user } = useAuth()

  if (!user?.is_admin) {
    return null
  }

  return (
    <>
      <PageHeader
        icon={<ServerIcon size={18} />}
        title={t('nav.admin')}
        subtitle={t('vm.subtitle')}
      />

      <AdminVmSection />

      <div style={{ marginTop: 16 }}>
        <button className="btn btn-ghost" onClick={() => navigate('/')}>
          {t('common.back')}
        </button>
      </div>
    </>
  )
}

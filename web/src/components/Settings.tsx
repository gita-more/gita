import { useCallback, useEffect, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { useI18n } from '../i18n'
import { api, type Token } from '../lib/api'
import { PageHeader, Alert, ConfirmDialog } from './Layout'
import { GlobeIcon, PlusIcon, CodeIcon, XIcon } from './Icons'
import { useErrorTranslator } from '../i18n'

/**
 * Settings is split into one page per section (/settings/language,
 * /settings/tokens) so the left nav links to real pages instead of
 * scrolling anchors. Each page renders exactly one section.
 */

/** The interface-language section (/settings/language). */
export function SettingsLanguage() {
  const { locale, setLocale, t } = useI18n()
  const navigate = useNavigate()
  const [saved, setSaved] = useState(false)
  const [error, setError] = useState('')

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

  return (
    <>
      <PageHeader
        icon={<GlobeIcon size={18} />}
        title={t('common.settings')}
        subtitle={t('settings.languageDescription')}
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

      <div style={{ marginTop: 16 }}>
        <button className="btn btn-ghost" onClick={() => navigate('/')}>
          {t('common.back')}
        </button>
      </div>
    </>
  )
}

/** The personal access tokens section (/settings/tokens). */
export function SettingsTokens() {
  const { t } = useI18n()
  const navigate = useNavigate()
  const translateError = useErrorTranslator()
  const [error, setError] = useState('')
  const [tokens, setTokens] = useState<Token[]>([])
  const [tokenName, setTokenName] = useState('')
  const [showCreateToken, setShowCreateToken] = useState(false)
  const [createdToken, setCreatedToken] = useState<string | null>(null)
  const [tokenBusy, setTokenBusy] = useState(false)
  // Token pending revocation — shown in the confirmation dialog.
  const [confirmToken, setConfirmToken] = useState<Token | null>(null)
  const [revokeBusy, setRevokeBusy] = useState(false)

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

  async function revokeToken(tok: Token) {
    setError('')
    setRevokeBusy(true)
    try {
      await api.deleteToken(tok.id)
      await loadTokens()
      setConfirmToken(null)
    } catch (err) {
      setError(translateError(err))
    } finally {
      setRevokeBusy(false)
    }
  }

  return (
    <>
      <PageHeader
        icon={<CodeIcon size={18} />}
        title={t('common.settings')}
        subtitle={t('settings.tokensDescription')}
      />
      {error && <Alert type="error">{error}</Alert>}

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
                    <button className="btn btn-danger btn-xs" onClick={() => setConfirmToken(tok)}>
                      {t('settings.revokeToken')}
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>

      <div style={{ marginTop: 16 }}>
        <button className="btn btn-ghost" onClick={() => navigate('/')}>
          {t('common.back')}
        </button>
      </div>

      {confirmToken && (
        <ConfirmDialog
          title={t('confirm.revokeTokenTitle')}
          message={t('confirm.revokeTokenMessage', { name: confirmToken.name })}
          confirmLabel={t('settings.revokeToken')}
          busy={revokeBusy}
          onConfirm={() => void revokeToken(confirmToken)}
          onCancel={() => setConfirmToken(null)}
        />
      )}
    </>
  )
}

import { useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { useI18n } from '../i18n'
import { PageHeader, Alert } from './Layout'
import { SettingsIcon } from './Icons'

export function Settings() {
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
        <div style={{ marginTop: 16 }}>
          <button className="btn btn-ghost" onClick={() => navigate('/')}>
            {t('common.back')}
          </button>
        </div>
      </div>
    </>
  )
}
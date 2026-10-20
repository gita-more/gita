import { useCallback, useEffect, useState } from 'react'
import { Alert, EmptyState } from './Layout'
import { GlobeIcon } from './Icons'
import { useI18n } from '../i18n'
import { api, type PagesConfig } from '../lib/api'

/**
 * Pages publishing configuration for a repository. Mirrors GitHub's
 * "Deploy from a branch": pick a branch and a directory ("/" or "/docs"),
 * and the tree there is served at `/{owner}/{repo}/`.
 *
 * The public URL is server-computed (`config.url`) so this panel never
 * re-derives the path scheme. Repo visibility is fetched here so the panel
 * can show the private-repo auth notice without the parent having to load
 * repo metadata.
 */
export function PagesPanel({
  owner,
  name,
}: {
  owner: string
  name: string
}) {
  const { t } = useI18n()
  const [config, setConfig] = useState<PagesConfig | null>(null)
  const [enabled, setEnabled] = useState(false)
  const [branch, setBranch] = useState('')
  const [path, setPath] = useState('/')
  const [error, setError] = useState('')
  const [saved, setSaved] = useState(false)
  const [busy, setBusy] = useState(false)
  const [available, setAvailable] = useState(true)
  const [isPrivate, setIsPrivate] = useState(false)

  const load = useCallback(async () => {
    try {
      const [res, repo] = await Promise.all([
        api.getPages(owner, name),
        api.getRepo(owner, name),
      ])
      setConfig(res)
      setEnabled(res.enabled)
      setBranch(res.branch)
      setPath(res.path)
      setAvailable(res.available)
      setIsPrivate(repo.is_private)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }, [owner, name])

  useEffect(() => {
    void load()
  }, [load])

  const save = useCallback(async () => {
    setBusy(true)
    setError('')
    setSaved(false)
    try {
      const res = await api.putPages(owner, name, enabled, branch, path)
      setConfig(res)
      setAvailable(res.available)
      setSaved(true)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }, [owner, name, enabled, branch, path])

  const disable = useCallback(async () => {
    setBusy(true)
    setError('')
    setSaved(false)
    try {
      await api.deletePages(owner, name)
      setEnabled(false)
      setSaved(true)
      setConfig(null)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }, [owner, name])

  const siteUrl = config?.url

  return (
    <>
      {error && <Alert type="error">{error}</Alert>}
      {saved && <Alert type="success">{t('pages.saved')}</Alert>}
      <div className="pages-panel">
        <label className="pages-toggle">
          <input
            type="checkbox"
            checked={enabled}
            onChange={(e) => setEnabled(e.target.checked)}
          />
          <span>{t('pages.enable')}</span>
        </label>

        {enabled && (
          <>
            <div className="create-form">
              <div className="field field-full">
                <label className="field-label" htmlFor="pages-branch">
                  {t('pages.branch')}
                </label>
                <input
                  id="pages-branch"
                  className="input"
                  placeholder={t('pages.branchPlaceholder')}
                  value={branch}
                  onChange={(e) => setBranch(e.target.value)}
                />
              </div>
              <div className="field field-full">
                <label className="field-label" htmlFor="pages-path">
                  {t('pages.path')}
                </label>
                <select
                  id="pages-path"
                  className="select"
                  value={path}
                  onChange={(e) => setPath(e.target.value)}
                >
                  <option value="/">{t('pages.pathRoot')}</option>
                  <option value="/docs">/docs</option>
                </select>
              </div>
              <div className="field-actions">
                <button
                  className="btn btn-primary"
                  disabled={busy || !available}
                  onClick={() => void save()}
                >
                  {busy ? t('common.loading') : t('pages.save')}
                </button>
                <button className="btn btn-ghost btn-sm" onClick={() => void disable()}>
                  {t('pages.disable')}
                </button>
              </div>
            </div>

            {!available && <Alert type="warning">{t('pages.branchMissing')}</Alert>}

            {isPrivate && <Alert type="info">{t('pages.privateNotice')}</Alert>}

            <p className="pages-hint">{t('pages.cspNotice')}</p>

            {siteUrl && (
              <p className="pages-visit">
                <GlobeIcon size={14} />
                <a href={siteUrl} target="_blank" rel="noreferrer">
                  {t('pages.visitSite')}
                </a>
                <code>{siteUrl}</code>
              </p>
            )}
          </>
        )}
      </div>
    </>
  )
}

/** Placeholder used when a repo has no Pages entry; kept for parity with the
 *  global EmptyState pattern so the feature is discoverable. */
export function PagesEmptyState({ onEnable }: { onEnable: () => void }) {
  const { t } = useI18n()
  return (
    <EmptyState
      icon={<GlobeIcon size={22} />}
      title={t('pages.title')}
      description={t('pages.subtitle')}
      action={
        <button className="btn btn-primary" onClick={onEnable}>
          {t('pages.enable')}
        </button>
      }
    />
  )
}
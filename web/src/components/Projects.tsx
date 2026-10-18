import { type FormEvent, useCallback, useEffect, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { api, type Repository } from '../lib/api'
import {
  PageHeader,
  Alert,
  EmptyState,
  StatusLabel,
} from './Layout'
import {
  BookIcon,
  FolderIcon,
  GitBranchIcon,
  PlusIcon,
  XIcon,
} from './Icons'
import { useI18n, useErrorTranslator } from '../i18n'

function repoStatus(repo: Repository): 'open' | 'merged' | 'closed' {
  return repo.is_private ? 'merged' : 'open'
}

export function Projects({
  onOpenRepo,
}: {
  onOpenRepo?: (owner: string, name: string) => void
}) {
  const { t } = useI18n()
  const translateError = useErrorTranslator()
  const navigate = useNavigate()
  const openRepo = (owner: string, name: string) =>
    onOpenRepo ? onOpenRepo(owner, name) : navigate(`/${owner}/${name}`)
  const [repos, setRepos] = useState<Repository[]>([])
  const [showCreate, setShowCreate] = useState(false)
  const [name, setName] = useState('')
  const [description, setDescription] = useState('')
  const [branch, setBranch] = useState('main')
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)

  const load = useCallback(async () => {
    try {
      const list = await api.listRepos()
      setRepos(list)
    } catch (err) {
      setError(translateError(err))
    }
  }, [translateError])

  useEffect(() => {
    void load()
  }, [load])

  async function handleCreate(e: FormEvent) {
    e.preventDefault()
    setError('')
    setBusy(true)
    try {
      const repo = await api.createRepo(name, description, branch, false)
      setShowCreate(false)
      setName('')
      setDescription('')
      setBranch('main')
      openRepo(repo.owner, repo.name)
    } catch (err) {
      setError(translateError(err))
    } finally {
      setBusy(false)
    }
  }

  return (
    <>
      <PageHeader
        icon={<FolderIcon size={18} />}
        title={t('projects.title')}
        subtitle={t('projects.subtitle', { n: repos.length })}
        actions={
          <button
            className="btn btn-primary"
            onClick={() => setShowCreate((v) => !v)}
            style={{ display: 'inline-flex', alignItems: 'center', gap: 6 }}
          >
            {showCreate ? <XIcon size={14} /> : <PlusIcon size={14} />}
            {showCreate ? t('common.cancel') : t('projects.createRepo')}
          </button>
        }
      />

      {showCreate && (
        <form className="create-form" onSubmit={handleCreate}>
          <div className="field">
            <label className="field-label" htmlFor="repo-name">{t('projects.repoName')}</label>
            <input
              id="repo-name"
              className="input"
              placeholder={t('projects.repoNamePlaceholder')}
              value={name}
              onChange={(e) => setName(e.target.value)}
              required
              pattern="[A-Za-z0-9\-_.]+"
            />
          </div>
          <div className="field">
            <label className="field-label" htmlFor="repo-branch">{t('projects.defaultBranch')}</label>
            <input
              id="repo-branch"
              className="input"
              placeholder="main"
              value={branch}
              onChange={(e) => setBranch(e.target.value)}
            />
          </div>
          <div className="field field-full">
            <label className="field-label" htmlFor="repo-desc">{t('projects.description')}</label>
            <input
              id="repo-desc"
              className="input"
              placeholder={t('projects.descriptionPlaceholder')}
              value={description}
              onChange={(e) => setDescription(e.target.value)}
            />
          </div>
          <div className="field-actions">
            <button className="btn btn-ghost" type="button" onClick={() => setShowCreate(false)}>
              {t('common.cancel')}
            </button>
            <button className="btn btn-primary" type="submit" disabled={busy}>
              {busy ? t('common.loading') : t('projects.createRepoTitle')}
            </button>
          </div>
        </form>
      )}

      {error && <Alert type="error">{error}</Alert>}

      {repos.length === 0 ? (
        <EmptyState
          icon={<FolderIcon size={22} />}
          title={t('projects.emptyTitle')}
          description={t('projects.emptyDescription')}
          action={
            <button className="btn btn-primary" onClick={() => setShowCreate(true)}>
              <PlusIcon size={14} />
              {t('projects.createRepo')}
            </button>
          }
        />
      ) : (
        <div className="repo-grid">
          {repos.map((repo) => (
            <button key={repo.id} className="repo-card" onClick={() => openRepo(repo.owner, repo.name)}>
              <div className="repo-card-top">
                <span className="repo-card-icon">
                  <BookIcon size={18} />
                </span>
                <StatusLabel status={repoStatus(repo)} />
              </div>
              <div>
                <div className="repo-card-name">{repo.full_name}</div>
                <div className="repo-card-desc">{repo.description || t('common.noDescription')}</div>
              </div>
              <div className="repo-card-meta">
                <span className="meta-item">
                  <GitBranchIcon size={14} />
                  {repo.default_branch}
                </span>
                <span className="meta-item">
                  {repo.is_private ? t('projects.private') : t('projects.public')}
                </span>
              </div>
            </button>
          ))}
        </div>
      )}
    </>
  )
}
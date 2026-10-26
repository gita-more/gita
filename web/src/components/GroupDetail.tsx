import { useCallback, useEffect, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { api, type Group, type Repository } from '../lib/api'
import {
  PageHeader,
  Alert,
  EmptyState,
  StatusLabel,
} from './Layout'
import { BookIcon, FolderIcon, PlusIcon, GitBranchIcon } from './Icons'
import { useI18n, useErrorTranslator } from '../i18n'

function repoStatus(repo: Repository): 'open' | 'merged' | 'closed' {
  return repo.is_private ? 'merged' : 'open'
}

export function GroupDetail({
  group,
  onOpenRepo,
}: {
  group: string
  onOpenRepo: (owner: string, name: string) => void
}) {
  const { t } = useI18n()
  const translateError = useErrorTranslator()
  const navigate = useNavigate()
  const [groupInfo, setGroupInfo] = useState<Group | null>(null)
  const [repos, setRepos] = useState<Repository[]>([])
  const [subgroups, setSubgroups] = useState<Group[]>([])
  const [error, setError] = useState('')
  const [showCreateRepo, setShowCreateRepo] = useState(false)
  const [showCreateSubgroup, setShowCreateSubgroup] = useState(false)
  const [repoName, setRepoName] = useState('')
  const [repoDescription, setRepoDescription] = useState('')
  const [repoBranch, setRepoBranch] = useState('main')
  const [subgroupName, setSubgroupName] = useState('')
  const [subgroupDescription, setSubgroupDescription] = useState('')
  const [repoError, setRepoError] = useState('')
  const [subgroupError, setSubgroupError] = useState('')
  const [repoBusy, setRepoBusy] = useState(false)
  const [subgroupBusy, setSubgroupBusy] = useState(false)

  const load = useCallback(async () => {
    try {
      const res = await api.getGroup(group)
      setGroupInfo(res.group)
      setRepos(res.repos)
      setSubgroups(res.subgroups || [])
    } catch (err) {
      setError(translateError(err))
    }
  }, [group, translateError])

  useEffect(() => {
    void load()
  }, [load])

  async function handleCreateRepo(e: React.FormEvent) {
    e.preventDefault()
    setRepoError('')
    setRepoBusy(true)
    try {
      const repo = await api.createGroupRepo(
        groupInfo!.full_path,
        repoName,
        repoDescription,
        false,
        repoBranch
      )
      setShowCreateRepo(false)
      setRepoName('')
      setRepoDescription('')
      setRepoBranch('main')
      onOpenRepo(repo.owner, repo.name)
    } catch (err) {
      setRepoError(translateError(err))
    } finally {
      setRepoBusy(false)
    }
  }

  async function handleCreateSubgroup(e: React.FormEvent) {
    e.preventDefault()
    setSubgroupError('')
    setSubgroupBusy(true)
    try {
      await api.createSubgroup(
        groupInfo!.full_path,
        subgroupName,
        subgroupDescription
      )
      setShowCreateSubgroup(false)
      setSubgroupName('')
      setSubgroupDescription('')
      // Reload to show new subgroup
      void load()
    } catch (err) {
      setSubgroupError(translateError(err))
    } finally {
      setSubgroupBusy(false)
    }
  }

  return (
    <>
      <PageHeader
        icon={<FolderIcon size={18} />}
        title={groupInfo?.full_path ?? group}
        subtitle={groupInfo?.description || t('groups.detailSubtitle')}
        actions={
          <div style={{ display: 'flex', gap: 8 }}>
            <button
              className="btn btn-primary"
              onClick={() => setShowCreateRepo(!showCreateRepo)}
              style={{ display: 'inline-flex', alignItems: 'center', gap: 6 }}
            >
              <PlusIcon size={14} />
              {t('groups.createProject')}
            </button>
            <button
              className="btn btn-primary"
              onClick={() => setShowCreateSubgroup(!showCreateSubgroup)}
              style={{ display: 'inline-flex', alignItems: 'center', gap: 6 }}
            >
              <PlusIcon size={14} />
              {t('groups.createSubgroup')}
            </button>
          </div>
        }
      />

      {error && <Alert type="error">{error}</Alert>}

      {showCreateRepo && (
        <form className="create-form" onSubmit={handleCreateRepo} style={{ marginBottom: 24 }}>
          <div className="field">
            <label className="field-label" htmlFor="repo-name">{t('projects.repoName')}</label>
            <input
              id="repo-name"
              className="input"
              placeholder={t('projects.repoNamePlaceholder')}
              value={repoName}
              onChange={(e) => setRepoName(e.target.value)}
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
              value={repoBranch}
              onChange={(e) => setRepoBranch(e.target.value)}
            />
          </div>
          <div className="field field-full">
            <label className="field-label" htmlFor="repo-desc">{t('projects.description')}</label>
            <input
              id="repo-desc"
              className="input"
              placeholder={t('projects.descriptionPlaceholder')}
              value={repoDescription}
              onChange={(e) => setRepoDescription(e.target.value)}
            />
          </div>
          {repoError && <Alert type="error">{repoError}</Alert>}
          <div className="field-actions">
            <button className="btn btn-ghost" type="button" onClick={() => setShowCreateRepo(false)}>
              {t('common.cancel')}
            </button>
            <button className="btn btn-primary" type="submit" disabled={repoBusy}>
              {repoBusy ? t('common.loading') : t('projects.createRepoTitle')}
            </button>
          </div>
        </form>
      )}

      {showCreateSubgroup && (
        <form className="create-form" onSubmit={handleCreateSubgroup} style={{ marginBottom: 24 }}>
          <div className="field">
            <label className="field-label" htmlFor="subgroup-name">{t('groups.subgroupName')}</label>
            <input
              id="subgroup-name"
              className="input"
              placeholder={t('groups.subgroupNamePlaceholder')}
              value={subgroupName}
              onChange={(e) => setSubgroupName(e.target.value)}
              required
              pattern="[A-Za-z0-9\-_.]+"
            />
          </div>
          <div className="field field-full">
            <label className="field-label" htmlFor="subgroup-desc">{t('groups.subgroupDescription')}</label>
            <input
              id="subgroup-desc"
              className="input"
              placeholder={t('groups.subgroupDescriptionPlaceholder')}
              value={subgroupDescription}
              onChange={(e) => setSubgroupDescription(e.target.value)}
            />
          </div>
          {subgroupError && <Alert type="error">{subgroupError}</Alert>}
          <div className="field-actions">
            <button className="btn btn-ghost" type="button" onClick={() => setShowCreateSubgroup(false)}>
              {t('common.cancel')}
            </button>
            <button className="btn btn-primary" type="submit" disabled={subgroupBusy}>
              {subgroupBusy ? t('common.loading') : t('groups.createSubgroupTitle')}
            </button>
          </div>
        </form>
      )}

      {subgroups.length > 0 && (
        <div style={{ marginBottom: 24 }}>
          <h3 style={{ fontSize: 14, fontWeight: 600, marginBottom: 12, color: '#8b949e' }}>
            {t('groups.subgroups')}
          </h3>
          <div className="repo-grid">
            {subgroups.map((sg) => (
              <button key={sg.id} className="repo-card" onClick={() => navigate(`/groups/${sg.full_path}`)}>
                <div className="repo-card-top">
                  <span className="repo-card-icon">
                    <FolderIcon size={18} />
                  </span>
                </div>
                <div className="repo-card-body">
                  <div className="repo-card-name">{sg.name}</div>
                  <div className="repo-card-desc">{sg.description || t('common.noDescription')}</div>
                </div>
              </button>
            ))}
          </div>
        </div>
      )}

      {repos.length === 0 && subgroups.length === 0 ? (
        <EmptyState
          icon={<FolderIcon size={22} />}
          title={t('groups.emptyReposTitle')}
          description={t('groups.emptyReposDescription')}
        />
      ) : (
        <div className="repo-grid">
          {repos.map((repo) => (
            <button
              key={repo.id}
              className="repo-card"
              onClick={() => onOpenRepo(repo.owner, repo.name)}
            >
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
                <span className="meta-item">{repo.is_private ? t('projects.private') : t('projects.public')}</span>
              </div>
            </button>
          ))}
        </div>
      )}
    </>
  )
}
import { useCallback, useEffect, useState } from 'react'
import { api, type Group, type Repository } from '../lib/api'
import {
  AppLayout,
  PageHeader,
  Alert,
  EmptyState,
  StatusLabel,
} from './Layout'
import { BookIcon, FolderIcon, UsersIcon } from './Icons'
import { useI18n, useErrorTranslator } from '../i18n'

function repoStatus(repo: Repository): 'open' | 'merged' | 'closed' {
  return repo.is_private ? 'merged' : 'open'
}

export function GroupDetail({
  group,
  onBack,
  onOpenRepo,
}: {
  group: string
  onBack: () => void
  onOpenRepo: (owner: string, name: string) => void
}) {
  const { t } = useI18n()
  const translateError = useErrorTranslator()
  const [groupInfo, setGroupInfo] = useState<Group | null>(null)
  const [repos, setRepos] = useState<Repository[]>([])
  const [error, setError] = useState('')

  const load = useCallback(async () => {
    try {
      const res = await api.getGroup(group)
      setGroupInfo(res.group)
      setRepos(res.repos)
    } catch (err) {
      setError(translateError(err))
    }
  }, [group, translateError])

  useEffect(() => {
    void load()
  }, [load])

  return (
    <AppLayout
      variant="repo"
      active="home"
      onNavigate={(target) => {
        if (target === 'home') onBack()
      }}
      repo={{ owner: group, name: '' }}
    >
      <PageHeader
        icon={<UsersIcon size={18} />}
        title={groupInfo?.name ?? group}
        subtitle={groupInfo?.description || t('groups.detailSubtitle')}
      />

      {error && <Alert type="error">{error}</Alert>}

      {repos.length === 0 ? (
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
                <span className="meta-item">{repo.default_branch}</span>
                <span className="meta-item">{repo.is_private ? t('projects.private') : t('projects.public')}</span>
              </div>
            </button>
          ))}
        </div>
      )}
    </AppLayout>
  )
}
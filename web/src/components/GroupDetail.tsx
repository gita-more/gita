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
  const [groupInfo, setGroupInfo] = useState<Group | null>(null)
  const [repos, setRepos] = useState<Repository[]>([])
  const [error, setError] = useState('')

  const load = useCallback(async () => {
    try {
      const res = await api.getGroup(group)
      setGroupInfo(res.group)
      setRepos(res.repos)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }, [group])

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
        subtitle={groupInfo?.description || '组的详情页'}
      />

      {error && <Alert type="error">{error}</Alert>}

      {repos.length === 0 ? (
        <EmptyState
          icon={<FolderIcon size={22} />}
          title="组里还没有项目"
          description="在组下创建项目后会显示在这里。"
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
                <div className="repo-card-desc">{repo.description || '暂无描述'}</div>
              </div>
              <div className="repo-card-meta">
                <span className="meta-item">{repo.default_branch}</span>
                <span className="meta-item">{repo.is_private ? '私有' : '公开'}</span>
              </div>
            </button>
          ))}
        </div>
      )}
    </AppLayout>
  )
}
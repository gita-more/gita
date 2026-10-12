import { type FormEvent, useCallback, useEffect, useState } from 'react'
import { api, type Repository } from '../lib/api'
import {
  AppLayout,
  PageHeader,
  Alert,
  EmptyState,
  StatusLabel,
  type NavTarget,
} from './Layout'
import {
  BookIcon,
  FolderIcon,
  GitBranchIcon,
  PlusIcon,
  XIcon,
} from './Icons'

function repoStatus(repo: Repository): 'open' | 'merged' | 'closed' {
  return repo.is_private ? 'merged' : 'open'
}

export function RepoList({ onOpen }: { onOpen: (owner: string, name: string) => void }) {
  const [repos, setRepos] = useState<Repository[]>([])
  const [showCreate, setShowCreate] = useState(false)
  const [name, setName] = useState('')
  const [description, setDescription] = useState('')
  const [branch, setBranch] = useState('main')
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const [active, setActive] = useState<NavTarget>('home')

  const load = useCallback(async () => {
    try {
      const list = await api.listRepos()
      setRepos(list)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }, [])

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
      onOpen(repo.owner, repo.name)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  const handleNavigate = (_target: NavTarget) => {
    // On the repo list page, all sidebar items lead back to the repository
    // overview (the only view available at this level).
    setActive('home')
  }

  return (
    <AppLayout active={active} onNavigate={handleNavigate}>
      <PageHeader
        icon={<FolderIcon size={18} />}
        title="仓库"
        subtitle={`共 ${repos.length} 个仓库`}
        actions={
          <button
            className="btn btn-primary"
            onClick={() => setShowCreate((v) => !v)}
            style={{ display: 'inline-flex', alignItems: 'center', gap: 6 }}
          >
            {showCreate ? <XIcon size={14} /> : <PlusIcon size={14} />}
            {showCreate ? '取消' : '新建仓库'}
          </button>
        }
      />

      {showCreate && (
        <form className="create-form" onSubmit={handleCreate}>
          <div className="field">
            <label className="field-label" htmlFor="repo-name">仓库名</label>
            <input
              id="repo-name"
              className="input"
              placeholder="如 my-project"
              value={name}
              onChange={(e) => setName(e.target.value)}
              required
              pattern="[A-Za-z0-9\-_.]+"
            />
          </div>
          <div className="field">
            <label className="field-label" htmlFor="repo-branch">默认分支</label>
            <input
              id="repo-branch"
              className="input"
              placeholder="main"
              value={branch}
              onChange={(e) => setBranch(e.target.value)}
            />
          </div>
          <div className="field field-full">
            <label className="field-label" htmlFor="repo-desc">描述（可选）</label>
            <input
              id="repo-desc"
              className="input"
              placeholder="仓库描述…"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
            />
          </div>
          <div className="field-actions">
            <button className="btn btn-ghost" type="button" onClick={() => setShowCreate(false)}>
              取消
            </button>
            <button className="btn btn-primary" type="submit" disabled={busy}>
              {busy ? '创建中…' : '创建仓库'}
            </button>
          </div>
        </form>
      )}

      {error && <Alert type="error">{error}</Alert>}

      {repos.length === 0 ? (
        <EmptyState
          icon={<FolderIcon size={22} />}
          title="还没有仓库"
          description="创建一个仓库，然后通过 git clone / push 开始使用 Gita 的 AI 原生工作流。"
          action={
            <button className="btn btn-primary" onClick={() => setShowCreate(true)}>
              <PlusIcon size={14} />
              新建仓库
            </button>
          }
        />
      ) : (
        <div className="repo-grid">
          {repos.map((repo) => (
            <button key={repo.id} className="repo-card" onClick={() => onOpen(repo.owner, repo.name)}>
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
                <span className="meta-item">
                  <GitBranchIcon size={14} />
                  {repo.default_branch}
                </span>
                <span className="meta-item">
                  {repo.is_private ? '私有' : '公开'}
                </span>
              </div>
            </button>
          ))}
        </div>
      )}
    </AppLayout>
  )
}
import { type FormEvent, useCallback, useEffect, useState } from 'react'
import { api, type Repository } from '../lib/api'
import { useAuth } from '../hooks/useAuth'

export function RepoList({ onOpen }: { onOpen: (owner: string, name: string) => void }) {
  const { user, logout } = useAuth()
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
      onOpen(repo.owner, repo.name)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="page">
      <header className="topbar">
        <div className="topbar-left">
          <span className="logo">Gita</span>
        </div>
        <div className="topbar-right">
          {user && <span className="username">@{user.username}</span>}
          <button className="btn-ghost" onClick={() => { void load() }}>刷新</button>
          <button className="btn-ghost" onClick={logout}>退出</button>
        </div>
      </header>

      <main className="repo-list">
        <div className="repo-list-header">
          <h2>仓库</h2>
          <button className="btn-primary" onClick={() => setShowCreate(!showCreate)}>
            {showCreate ? '取消' : '+ 新建仓库'}
          </button>
        </div>

        {showCreate && (
          <form className="create-repo-form" onSubmit={handleCreate}>
            <input
              placeholder="仓库名 (如 my-project)"
              value={name}
              onChange={(e) => setName(e.target.value)}
              required
              pattern="[A-Za-z0-9\-_.]+"
            />
            <input
              placeholder="描述 (可选)"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
            />
            <input
              placeholder="默认分支"
              value={branch}
              onChange={(e) => setBranch(e.target.value)}
            />
            <button className="btn-primary" type="submit" disabled={busy}>创建</button>
          </form>
        )}

        {error && <div className="error-banner">{error}</div>}

        {repos.length === 0 ? (
          <div className="empty">还没有仓库，点击右上角「+ 新建仓库」开始。</div>
        ) : (
          <div className="repo-cards">
            {repos.map((repo) => (
              <button
                key={repo.id}
                className="repo-card"
                onClick={() => onOpen(repo.owner, repo.name)}
              >
                <div className="repo-card-name">{repo.full_name}</div>
                <div className="repo-card-desc">{repo.description || '暂无描述'}</div>
                <div className="repo-card-meta">
                  <span>{repo.default_branch}</span>
                  <span>{repo.is_private ? '私有' : '公开'}</span>
                </div>
              </button>
            ))}
          </div>
        )}
      </main>
    </div>
  )
}
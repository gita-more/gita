import { type FormEvent, useCallback, useEffect, useState } from 'react'
import { api, type Group } from '../lib/api'
import {
  AppLayout,
  PageHeader,
  Alert,
  EmptyState,
  type GlobalNavTarget,
} from './Layout'
import { PlusIcon, UsersIcon, XIcon } from './Icons'

export function Groups({
  onOpenGroup,
  onNavigate,
  activeTab,
}: {
  onOpenGroup: (group: string) => void
  onNavigate: (target: GlobalNavTarget) => void
  activeTab: GlobalNavTarget
}) {
  const [groups, setGroups] = useState<Group[]>([])
  const [showCreate, setShowCreate] = useState(false)
  const [name, setName] = useState('')
  const [description, setDescription] = useState('')
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)

  const load = useCallback(async () => {
    try {
      const list = await api.listGroups()
      setGroups(list)
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
      const g = await api.createGroup(name, description)
      setShowCreate(false)
      setName('')
      setDescription('')
      onOpenGroup(g.name)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  return (
    <AppLayout
      variant="global"
      globalActive={activeTab}
      onGlobalNavigate={onNavigate}
    >
      <PageHeader
        icon={<UsersIcon size={18} />}
        title="Groups"
        subtitle={`共 ${groups.length} 个组`}
        actions={
          <button
            className="btn btn-primary"
            onClick={() => setShowCreate((v) => !v)}
            style={{ display: 'inline-flex', alignItems: 'center', gap: 6 }}
          >
            {showCreate ? <XIcon size={14} /> : <PlusIcon size={14} />}
            {showCreate ? '取消' : '新建 Group'}
          </button>
        }
      />

      {showCreate && (
        <form className="create-form" onSubmit={handleCreate}>
          <div className="field">
            <label className="field-label" htmlFor="group-name">组名</label>
            <input
              id="group-name"
              className="input"
              placeholder="如 acme"
              value={name}
              onChange={(e) => setName(e.target.value)}
              required
              pattern="[A-Za-z0-9\-_.]+"
            />
          </div>
          <div className="field field-full">
            <label className="field-label" htmlFor="group-desc">描述（可选）</label>
            <input
              id="group-desc"
              className="input"
              placeholder="组描述…"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
            />
          </div>
          <div className="field-actions">
            <button className="btn btn-ghost" type="button" onClick={() => setShowCreate(false)}>
              取消
            </button>
            <button className="btn btn-primary" type="submit" disabled={busy}>
              {busy ? '创建中…' : '创建组'}
            </button>
          </div>
        </form>
      )}

      {error && <Alert type="error">{error}</Alert>}

      {groups.length === 0 ? (
        <EmptyState
          icon={<UsersIcon size={22} />}
          title="还没有组"
          description="创建一个组，把多个仓库归在同一 namespace 下。"
          action={
            <button className="btn btn-primary" onClick={() => setShowCreate(true)}>
              <PlusIcon size={14} />
              新建组
            </button>
          }
        />
      ) : (
        <div className="repo-grid">
          {groups.map((g) => (
            <button key={g.id} className="repo-card" onClick={() => onOpenGroup(g.name)}>
              <div className="repo-card-top">
                <span className="repo-card-icon">
                  <UsersIcon size={18} />
                </span>
              </div>
              <div>
                <div className="repo-card-name">{g.name}</div>
                <div className="repo-card-desc">{g.description || '暂无描述'}</div>
              </div>
            </button>
          ))}
        </div>
      )}
    </AppLayout>
  )
}
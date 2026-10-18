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
import { useI18n, useErrorTranslator } from '../i18n'

export function Groups({
  onOpenGroup,
  onNavigate,
  activeTab,
}: {
  onOpenGroup: (group: string) => void
  onNavigate: (target: GlobalNavTarget) => void
  activeTab: GlobalNavTarget
}) {
  const { t } = useI18n()
  const translateError = useErrorTranslator()
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
      const g = await api.createGroup(name, description)
      setShowCreate(false)
      setName('')
      setDescription('')
      onOpenGroup(g.name)
    } catch (err) {
      setError(translateError(err))
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
        title={t('groups.title')}
        subtitle={t('groups.subtitle', { n: groups.length })}
        actions={
          <button
            className="btn btn-primary"
            onClick={() => setShowCreate((v) => !v)}
            style={{ display: 'inline-flex', alignItems: 'center', gap: 6 }}
          >
            {showCreate ? <XIcon size={14} /> : <PlusIcon size={14} />}
            {showCreate ? t('common.cancel') : t('groups.createGroup')}
          </button>
        }
      />

      {showCreate && (
        <form className="create-form" onSubmit={handleCreate}>
          <div className="field">
            <label className="field-label" htmlFor="group-name">{t('groups.groupName')}</label>
            <input
              id="group-name"
              className="input"
              placeholder={t('groups.groupNamePlaceholder')}
              value={name}
              onChange={(e) => setName(e.target.value)}
              required
              pattern="[A-Za-z0-9\-_.]+"
            />
          </div>
          <div className="field field-full">
            <label className="field-label" htmlFor="group-desc">{t('groups.groupDescription')}</label>
            <input
              id="group-desc"
              className="input"
              placeholder={t('groups.groupDescriptionPlaceholder')}
              value={description}
              onChange={(e) => setDescription(e.target.value)}
            />
          </div>
          <div className="field-actions">
            <button className="btn btn-ghost" type="button" onClick={() => setShowCreate(false)}>
              {t('common.cancel')}
            </button>
            <button className="btn btn-primary" type="submit" disabled={busy}>
              {busy ? t('common.loading') : t('groups.createGroupTitle')}
            </button>
          </div>
        </form>
      )}

      {error && <Alert type="error">{error}</Alert>}

      {groups.length === 0 ? (
        <EmptyState
          icon={<UsersIcon size={22} />}
          title={t('groups.emptyTitle')}
          description={t('groups.emptyDescription')}
          action={
            <button className="btn btn-primary" onClick={() => setShowCreate(true)}>
              <PlusIcon size={14} />
              {t('groups.createGroupTitle')}
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
                <div className="repo-card-desc">{g.description || t('common.noDescription')}</div>
              </div>
            </button>
          ))}
        </div>
      )}
    </AppLayout>
  )
}
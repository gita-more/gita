import { useCallback, useEffect, useState } from 'react'
import { api, type GlobalIssueItem } from '../lib/api'
import {
  AppLayout,
  PageHeader,
  Alert,
  EmptyState,
  StatusLabel,
  type GlobalNavTarget,
} from './Layout'
import { CircleDotIcon, FolderIcon } from './Icons'
import { useI18n, useErrorTranslator } from '../i18n'

export function Issues({
  onOpenRepo,
  onNavigate,
  activeTab,
}: {
  onOpenRepo: (owner: string, name: string) => void
  onNavigate: (target: GlobalNavTarget) => void
  activeTab: GlobalNavTarget
}) {
  const { t } = useI18n()
  const translateError = useErrorTranslator()
  const [issues, setIssues] = useState<GlobalIssueItem[]>([])
  const [error, setError] = useState('')

  const load = useCallback(async () => {
    try {
      const res = await api.listGlobalIssues('all', 50)
      setIssues(res.issues)
    } catch (err) {
      setError(translateError(err))
    }
  }, [translateError])

  useEffect(() => {
    void load()
  }, [load])

  return (
    <AppLayout
      variant="global"
      globalActive={activeTab}
      onGlobalNavigate={onNavigate}
    >
      <PageHeader
        icon={<FolderIcon size={18} />}
        title={t('issues.title')}
        subtitle={t('issues.subtitle', { n: issues.length })}
      />

      {error && <Alert type="error">{error}</Alert>}

      {issues.length === 0 ? (
        <EmptyState
          icon={<FolderIcon size={22} />}
          title={t('issues.emptyTitle')}
          description={t('issues.emptyDescription')}
        />
      ) : (
        <div className="row-list">
          {issues.map((it) => (
            <button
              key={it.id}
              className="row-item"
              onClick={() => onOpenRepo(it.repo_owner, it.repo_name)}
            >
              <span
                className={`row-icon ${
                  it.status === 'closed' ? 'closed' : 'open'
                }`}
              >
                <CircleDotIcon size={16} />
              </span>
              <div className="row-main">
                <div className="row-title">{it.title}</div>
                <div className="row-meta">
                  <span>
                    {it.full_name} #{it.number}
                  </span>
                  <span>{t('issues.by')} {it.author}</span>
                </div>
              </div>
              <StatusLabel status={it.status} />
            </button>
          ))}
        </div>
      )}
    </AppLayout>
  )
}
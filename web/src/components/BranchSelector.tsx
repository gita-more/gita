import { useEffect, useMemo, useRef, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { useI18n } from '../i18n'
import { CheckIcon, ChevronDownIcon, GitBranchIcon, TagIcon, XIcon } from './Icons'

type RefTab = 'branches' | 'tags'

/**
 * GitHub-style "Switch branches/tags" dropdown: search box, branch/tag tabs
 * and a check-marked list with a "default" badge on the default branch.
 */
export function BranchSelector({
  owner,
  name,
  rev,
  branches,
  tags,
  defaultBranch,
  onSelect,
}: {
  owner: string
  name: string
  /** Currently selected ref (branch name or tag). */
  rev: string
  branches: string[]
  tags: string[]
  defaultBranch: string
  onSelect: (ref: string) => void
}) {
  const { t } = useI18n()
  const navigate = useNavigate()
  const [open, setOpen] = useState(false)
  const [tab, setTab] = useState<RefTab>('branches')
  const [query, setQuery] = useState('')
  const rootRef = useRef<HTMLDivElement>(null)

  // Close on Escape / click outside, matching the usual popover contract.
  useEffect(() => {
    if (!open) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setOpen(false)
    }
    const onClick = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) setOpen(false)
    }
    document.addEventListener('keydown', onKey)
    document.addEventListener('mousedown', onClick)
    return () => {
      document.removeEventListener('keydown', onKey)
      document.removeEventListener('mousedown', onClick)
    }
  }, [open])

  function toggle(next: boolean) {
    setOpen(next)
    // Fresh state on every open, like GitHub reopening its overlay.
    if (next) {
      setTab('branches')
      setQuery('')
    }
  }

  const items = tab === 'branches' ? branches : tags
  const visible = useMemo(() => {
    const q = query.trim().toLowerCase()
    return q === '' ? items : items.filter((name) => name.toLowerCase().includes(q))
  }, [items, query])

  return (
    <div className="branch-selector" ref={rootRef}>
      <button
        type="button"
        className="btn btn-ghost btn-sm branch-selector-trigger"
        onClick={() => toggle(!open)}
        aria-expanded={open}
        aria-haspopup="dialog"
        title={rev}
      >
        <GitBranchIcon size={14} />
        <span className="branch-selector-name">{rev}</span>
        <ChevronDownIcon size={12} />
      </button>

      {open && (
        <div className="ref-selector-panel" role="dialog" aria-label={t('repo.ref.switchTitle')}>
          <div className="ref-selector-header">
            <span className="ref-selector-title">{t('repo.ref.switchTitle')}</span>
            <button
              type="button"
              className="btn btn-ghost btn-sm ref-selector-close"
              onClick={() => toggle(false)}
              aria-label={t('common.cancel')}
            >
              <XIcon size={14} />
            </button>
          </div>

          <div className="ref-selector-search">
            <input
              type="text"
              placeholder={t('repo.ref.findRef')}
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              aria-label={t('repo.ref.findRef')}
              autoFocus
            />
          </div>

          <div className="ref-selector-tabs" role="tablist" aria-label={t('repo.ref.switchTitle')}>
            <button
              type="button"
              role="tab"
              aria-selected={tab === 'branches'}
              className={`ref-selector-tab${tab === 'branches' ? ' active' : ''}`}
              onClick={() => setTab('branches')}
            >
              <GitBranchIcon size={13} />
              {t('repo.ref.branches')}
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={tab === 'tags'}
              className={`ref-selector-tab${tab === 'tags' ? ' active' : ''}`}
              onClick={() => setTab('tags')}
            >
              <TagIcon size={13} />
              {t('repo.ref.tags')}
            </button>
          </div>

          <ul className="ref-selector-list" role="listbox" aria-label={t('repo.ref.switchTitle')}>
            {visible.map((refName) => {
              const selected = refName === rev
              return (
                <li key={refName}>
                  <button
                    type="button"
                    role="option"
                    aria-selected={selected}
                    className={`ref-selector-item${selected ? ' selected' : ''}`}
                    onClick={() => {
                      onSelect(refName)
                      toggle(false)
                    }}
                  >
                    <span className="ref-selector-check">
                      {selected && <CheckIcon size={14} />}
                    </span>
                    <span className="ref-selector-item-name" title={refName}>
                      {refName}
                    </span>
                    {tab === 'branches' && refName === defaultBranch && (
                      <span className="ref-selector-badge">{t('repo.ref.default')}</span>
                    )}
                  </button>
                </li>
              )
            })}
            {visible.length === 0 && (
              <li className="ref-selector-empty">{t('repo.ref.noMatches')}</li>
            )}
          </ul>

          <div className="ref-selector-footer">
            <button
              type="button"
              className="ref-selector-view-all"
              onClick={() => {
                toggle(false)
                // "View all branches/tags" opens the repository settings.
                navigate(
                  `/${owner}/${name}/settings/${tab === 'branches' ? 'branches' : 'tags'}`,
                )
              }}
            >
              {tab === 'branches' ? t('repo.ref.viewAllBranches') : t('repo.ref.viewAllTags')}
            </button>
          </div>
        </div>
      )}
    </div>
  )
}

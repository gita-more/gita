import { useState, type ReactNode, useRef, useEffect } from 'react'
import { Link, useLocation, useNavigate, useSearchParams } from 'react-router-dom'
import {
  AlertTriangleIcon,
  BookIcon,
  CircleCheckIcon,
  CircleDotIcon,
  GitCommitIcon,
  GitPullRequestIcon,
  GlobeIcon,
  HomeIcon,
  PersonIcon,
  RefreshIcon,
  RocketIcon,
  SettingsIcon,
  ShieldAlertIcon,
  UsersIcon,
  XIcon,
} from './Icons'
import { useAuth } from '../hooks/useAuth'
import { useI18n } from '../i18n'
import { api, type RepoSearchHit, type CodeSearchHit } from '../lib/api'

// Tiny inline magnifier icon used inside the search box.
function SearchBoxIcon({ size = 14 }: { size?: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 16 16"
      fill="currentColor"
      aria-hidden="true"
    >
      <path d="M10.68 11.74a6 6 0 0 1-7.922-8.982 6 6 0 0 1 8.982 7.922l3.04 3.04a.749.749 0 0 1-.326 1.275.749.749 0 0 1-.734-.215ZM11.5 7a4.499 4.499 0 1 0-8.997 0A4.499 4.499 0 0 0 11.5 7Z" />
    </svg>
  )
}

export type NavTarget =
  | 'home'
  | 'files'
  | 'commits'
  | 'prs'
  | 'issues'
  | 'pipelines'
  | 'settings'

/** Top-level navigation targets shown on the global (home) sidebar. */
export type GlobalNavTarget = 'projects' | 'groups' | 'issues'

const GLOBAL_NAV: { key: GlobalNavTarget; labelKey: string; icon: typeof HomeIcon }[] = [
  { key: 'projects', labelKey: 'nav.projects', icon: BookIcon },
  { key: 'groups', labelKey: 'nav.groups', icon: UsersIcon },
  { key: 'issues', labelKey: 'nav.issuesGlobal', icon: CircleDotIcon },
]

const SIDEBAR_NAV: { key: NavTarget; labelKey: string; icon: typeof HomeIcon; countKey?: 'prs' | 'issues' }[] = [
  { key: 'files', labelKey: 'nav.files', icon: BookIcon },
  { key: 'commits', labelKey: 'nav.commits', icon: GitCommitIcon },
  { key: 'prs', labelKey: 'nav.prs', icon: GitPullRequestIcon, countKey: 'prs' },
  { key: 'issues', labelKey: 'nav.issues', icon: CircleDotIcon, countKey: 'issues' },
  { key: 'pipelines', labelKey: 'nav.pipelines', icon: RocketIcon },
  { key: 'settings', labelKey: 'nav.pages', icon: GlobeIcon },
]

export function Brand({ compact = false }: { compact?: boolean }) {
  return (
    <Link className="brand" to="/">
      <span className="brand-mark">
        <svg width={compact ? 20 : 18} height={compact ? 20 : 18} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.4" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
          <path d="M6 3v12" />
          <circle cx="18" cy="6" r="3" />
          <circle cx="6" cy="18" r="3" />
          <path d="M18 9a9 9 0 0 1-9 9" />
        </svg>
      </span>
      <span>Gita</span>
    </Link>
  )
}

function Avatar({ username }: { username: string }) {
  const [failed, setFailed] = useState(false)
  if (failed) {
    return <span className="user-avatar">{username.slice(0, 1).toUpperCase()}</span>
  }
  return (
    <img
      className="user-avatar"
      src={`/api/users/${username}/avatar`}
      alt={username.slice(0, 1).toUpperCase()}
      onError={() => setFailed(true)}
    />
  )
}

function UserMenu({ user, onClose }: { user: { username: string; email: string } | null; onClose: () => void }) {
  const { t } = useI18n()
  const navigate = useNavigate()
  const { logout } = useAuth()
  const handleLogout = () => {
    logout()
    onClose()
    navigate('/login', { replace: true })
  }

  return (
    <div className="user-menu" role="menu" aria-label={t('common.userMenu')}>
      <div className="user-menu-header">
        <Avatar username={user?.username ?? 'U'} />
        <div className="user-menu-info">
          <span className="user-menu-name">{user?.username ?? t('common.notLoggedIn')}</span>
          <span className="user-menu-email">{user?.email ?? ''}</span>
        </div>
      </div>
      <hr className="user-menu-divider" />
      <ul className="user-menu-list">
        <li>
          <Link className="user-menu-item" to="/settings" onClick={onClose}>
            <SettingsIcon size={16} />
            <span>{t('common.settings')}</span>
          </Link>
        </li>
        <li>
          <button className="user-menu-item" onClick={handleLogout}>
            <PersonIcon size={16} />
            <span>{t('common.logout')}</span>
          </button>
        </li>
      </ul>
    </div>
  )
}

export function Topbar({
  repo,
  actions,
  onToggleSidebar,
  sidebarOpen,
}: {
  repo?: { owner: string; name: string }
  actions?: ReactNode
  onToggleSidebar?: () => void
  sidebarOpen?: boolean
}) {
  const { user, logout } = useAuth()
  const { t } = useI18n()
  const [showUserMenu, setShowUserMenu] = useState(false)
  const userMenuRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    function handleLogout() {
      logout()
    }
    window.addEventListener('gita:logout', handleLogout)
    return () => window.removeEventListener('gita:logout', handleLogout)
  }, [logout])

  useEffect(() => {
    function handleClickOutside(event: MouseEvent) {
      if (userMenuRef.current && !userMenuRef.current.contains(event.target as Node)) {
        setShowUserMenu(false)
      }
    }
    if (showUserMenu) {
      document.addEventListener('mousedown', handleClickOutside)
    }
    return () => document.removeEventListener('mousedown', handleClickOutside)
  }, [showUserMenu])

  return (
    <header className="topbar">
      <div className="topbar-left">
        {onToggleSidebar && (
          <button
            className="sidebar-toggle"
            onClick={onToggleSidebar}
            title={t('common.toggleMenu')}
            aria-label={t('common.toggleMenu')}
            aria-expanded={sidebarOpen}
          >
            {sidebarOpen ? <XIcon size={16} /> : <MenuIcon size={16} />}
          </button>
        )}
        <Brand />
      </div>
      {repo ? (
        <nav className="breadcrumbs" aria-label="Breadcrumb">
          <ol className="breadcrumb-list">
            <li className="breadcrumb-item">
              <Link to="/">
                <HomeIcon size={14} />
                <span>{t('nav.home')}</span>
              </Link>
            </li>
            <li className="breadcrumb-sep" aria-hidden="true">/</li>
            <li className="breadcrumb-item">
              <a href="#" onClick={(e) => e.preventDefault()}>{repo.owner}</a>
            </li>
            <li className="breadcrumb-sep" aria-hidden="true">/</li>
            <li className="breadcrumb-item current" aria-current="page">
              {repo.name}
            </li>
          </ol>
        </nav>
      ) : null}
      <div className="topbar-spacer" />
      <SearchBox />
      <div className="topbar-actions">
        {actions}
        <div className="user-menu-container" ref={userMenuRef}>
          <button
            className="topbar-user-btn"
            onClick={(e) => { e.stopPropagation(); setShowUserMenu(!showUserMenu); }}
            title={t('common.userMenu')}
            aria-label={t('common.userMenu')}
            aria-expanded={showUserMenu}
            aria-haspopup="true"
          >
            <Avatar username={user?.username ?? 'U'} />
          </button>
          {showUserMenu && <UserMenu user={user} onClose={() => setShowUserMenu(false)} />}
        </div>
      </div>
    </header>
  )
}

export type SidebarVariant = 'global' | 'repo'

export function Sidebar({
  variant,
  active,
  globalActive,
  onGlobalNavigate,
  counts,
}: {
  variant: SidebarVariant
  active?: NavTarget
  globalActive?: GlobalNavTarget
  onGlobalNavigate?: (target: GlobalNavTarget) => void
  counts?: Partial<Record<'prs' | 'issues', number>>
}) {
  const { t } = useI18n()
  const location = useLocation()
  const navigate = useNavigate()
  const [searchParams] = useSearchParams()

  // For the global sidebar, prefer the URL pathname over the deprecated
  // globalActive/onGlobalNavigate props (kept for backwards compatibility).
  const activeKey = (() => {
    if (variant !== 'global') return null
    const p = location.pathname
    if (p === '/' ) return 'projects'
    if (p.startsWith('/groups')) return 'groups'
    if (p.startsWith('/issues')) return 'issues'
    return globalActive ?? null
  })()

  return (
    <aside className="sidebar">
      <nav className="sidebar-nav" aria-label={t('common.mainNav')}>
        {variant === 'global' &&
          GLOBAL_NAV.map((item) => {
            const Icon = item.icon
            const isActive = activeKey === item.key
            const handleClick = item.key === 'projects'
              ? () => navigate('/')
              : () => {
                  onGlobalNavigate?.(item.key)
                  navigate(item.key === 'groups' ? '/groups' : '/issues')
                }
            return (
              <button
                key={item.key}
                className={`sidebar-link${isActive ? ' active' : ''}`}
                onClick={handleClick}
                title={t(item.labelKey)}
              >
                <Icon size={16} />
                <span>{t(item.labelKey)}</span>
              </button>
            )
          })}
        {variant === 'repo' &&
          SIDEBAR_NAV.map((item) => {
            const Icon = item.icon
            // Use ?tab= as the repo source-of-truth (RepoDetail reads it via
            // useSearchParams). Sidebar reads the current value to highlight.
            const currentTab = searchParams.get('tab') || 'files'
            const isActive = active === item.key
              ? true
              : (item.key === 'files' && currentTab === 'files') ||
                (item.key === 'commits' && currentTab === 'commits') ||
                (item.key === 'prs' && currentTab === 'prs') ||
                (item.key === 'issues' && currentTab === 'issues') ||
                (item.key === 'pipelines' && currentTab === 'pipelines')
            return (
              <button
                key={item.key}
                className={`sidebar-link${isActive ? ' active' : ''}`}
                onClick={() => {
                  // Repo tabs are owned by the URL (?tab=...); use the
                  // absolute pathname to be robust against relative-search
                  // resolution differences and replace so switching tabs
                  // doesn't pollute history.
                  navigate(`${location.pathname}?tab=${item.key}`, { replace: true })
                }}
                title={t(item.labelKey)}
              >
                <Icon size={16} />
                <span>{t(item.labelKey)}</span>
                {item.countKey && counts?.[item.countKey] != null && counts[item.countKey]! > 0 && (
                  <span className="count">{counts[item.countKey]}</span>
                )}
              </button>
            )
          })}
      </nav>
    </aside>
  )
}

export function AppLayout({
  variant = 'repo',
  active,
  globalActive,
  onGlobalNavigate,
  repo,
  counts,
  children,
}: {
  variant?: SidebarVariant
  active?: NavTarget
  globalActive?: GlobalNavTarget
  onGlobalNavigate?: (target: GlobalNavTarget) => void
  repo?: { owner: string; name: string }
  counts?: Partial<Record<'prs' | 'issues', number>>
  children: ReactNode
}) {
  const [sidebarOpen, setSidebarOpen] = useState(false)
  const [collapsed, setCollapsed] = useState(false)

  const isMobile = () => window.matchMedia('(max-width: 860px)').matches

  const handleToggleSidebar = () => {
    if (isMobile()) {
      setSidebarOpen((v) => !v)
    } else {
      setCollapsed((v) => !v)
    }
  }

  const handleGlobalNavigate = (target: GlobalNavTarget) => {
    setSidebarOpen(false)
    onGlobalNavigate?.(target)
  }

  return (
    <>
      <Topbar
        repo={repo}
        onToggleSidebar={handleToggleSidebar}
        sidebarOpen={sidebarOpen}
      />
      <div className={`app${collapsed ? ' sidebar-collapsed' : ''}`}>
        <div className={`sidebar-overlay${sidebarOpen ? ' open' : ''}`} onClick={() => setSidebarOpen(false)} />
        <div className={`sidebar-shell${sidebarOpen ? ' open' : ''}`}>
          <Sidebar
            variant={variant}
            active={active}
            globalActive={globalActive}
            onGlobalNavigate={handleGlobalNavigate}
            counts={counts}
          />
        </div>
        <main className="main">
          <div className="content">{children}</div>
        </main>
      </div>
    </>
  )
}

function MenuIcon({ size = 16 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <line x1="3" y1="6" x2="21" y2="6" />
      <line x1="3" y1="12" x2="21" y2="12" />
      <line x1="3" y1="18" x2="21" y2="18" />
    </svg>
  )
}

export function PageHeader({
  title,
  subtitle,
  actions,
  icon,
}: {
  title: ReactNode
  subtitle?: ReactNode
  actions?: ReactNode
  icon?: ReactNode
}) {
  return (
    <div className="page-header">
      <div>
        <h2 className="page-title">
          {icon}
          {title}
        </h2>
        {subtitle && <p className="page-subtitle">{subtitle}</p>}
      </div>
      {actions && <div className="page-actions">{actions}</div>}
    </div>
  )
}

export function Alert({
  type,
  children,
}: {
  type: 'error' | 'info' | 'warning' | 'success'
  children: ReactNode
}) {
  const Icon =
    type === 'error' ? ShieldAlertIcon : type === 'warning' ? AlertTriangleIcon : type === 'success' ? CircleCheckIcon : RefreshIcon
  return (
    <div className={`alert alert-${type}`}>
      <Icon size={14} />
      <div>{children}</div>
    </div>
  )
}

export function EmptyState({
  icon,
  title,
  description,
  action,
}: {
  icon?: ReactNode
  title: string
  description?: ReactNode
  action?: ReactNode
}) {
  return (
    <div className="empty-state">
      {icon && <div className="empty-icon">{icon}</div>}
      <h3>{title}</h3>
      {description && <p>{description}</p>}
      {action}
    </div>
  )
}

export function StatusLabel({ status }: { status: string }) {
  const { t } = useI18n()
  let cls = 'status'
  let Icon = GitPullRequestIcon
  // status is one of 'open' / 'merged' / 'closed' / 'resolved'. We use the
  // raw status as the i18n key under status.*; if a future status slips in
  // here untranslated, fall back to the raw string so the UI never goes blank.
  const labelKey = `status.${status}` as const
  const translated = t(labelKey)
  const label = translated === labelKey ? status : translated
  if (status === 'open') {
    cls += ' status-open'
    Icon = CircleDotIcon
  } else if (status === 'merged') {
    cls += ' status-merged'
    Icon = GitPullRequestIcon
  } else if (status === 'closed') {
    cls += ' status-closed'
    Icon = CircleCheckIcon
  } else if (status === 'resolved') {
    cls += ' status-open'
    Icon = CircleCheckIcon
  }
  return (
    <span className={cls}>
      <Icon size={12} />
      {label}
    </span>
  )
}
// ---------------------------------------------------------------------------
// Global search box
// ---------------------------------------------------------------------------

function SearchBox() {
  const { t } = useI18n()
  const [open, setOpen] = useState(false)
  const [q, setQ] = useState('')
  const [repos, setRepos] = useState<RepoSearchHit[]>([])
  const [code, setCode] = useState<CodeSearchHit[]>([])
  const [busy, setBusy] = useState(false)
  const inputRef = useRef<HTMLInputElement>(null)
  const navigate = useNavigate()

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === '/' && document.activeElement?.tagName !== 'INPUT' && document.activeElement?.tagName !== 'TEXTAREA') {
        e.preventDefault()
        setOpen(true)
        setTimeout(() => inputRef.current?.focus(), 0)
      } else if (e.key === 'Escape' && open) {
        setOpen(false)
        setQ('')
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [open])

  async function runSearch(value: string) {
    setQ(value)
    if (!value.trim()) {
      setRepos([])
      setCode([])
      return
    }
    setBusy(true)
    try {
      const res = await api.search(value, 'all')
      setRepos(res.repos ?? [])
      setCode(res.code ?? [])
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="search-wrapper">
      <div className="search-box" role="search" onClick={() => inputRef.current?.focus()}>
        <SearchBoxIcon size={14} />
        <input
          ref={inputRef}
          className="search-input"
          placeholder={t('common.search')}
          value={q}
          onChange={(e) => void runSearch(e.target.value)}
          onFocus={() => setOpen(true)}
        />
        <kbd>/</kbd>
        {open && q && (
          <button className="search-clear" onClick={() => { setQ(''); setRepos([]); setCode([]) }} aria-label="clear">
            <XIcon size={12} />
          </button>
        )}
      </div>
      {open && q.trim() && (
        <div className="search-results">
          {busy && <div className="search-hint">{t('common.loading')}</div>}
          {!busy && repos.length === 0 && code.length === 0 && (
            <div className="search-hint">no results</div>
          )}
          {repos.length > 0 && (
            <div className="search-section">
              <div className="search-section-title">repositories</div>
              {repos.slice(0, 5).map((r) => (
                <button
                  key={r.id}
                  className="search-row"
                  onClick={() => { setOpen(false); setQ(''); navigate(`/${r.owner}/${r.name}`) }}
                >
                  <BookIcon size={14} />
                  <span className="search-row-title">{r.full_name}</span>
                  <span className="search-row-sub">{r.description || ''}</span>
                </button>
              ))}
            </div>
          )}
          {code.length > 0 && (
            <div className="search-section">
              <div className="search-section-title">code</div>
              {code.slice(0, 8).map((c, i) => (
                <button
                  key={`${c.owner}-${c.repo}-${c.path}-${c.line}-${i}`}
                  className="search-row search-row-code"
                  onClick={() => { setOpen(false); setQ(''); navigate(`/${c.owner}/${c.repo}`) }}
                >
                  <FileCodeIconPlaceholder />
                  <div className="search-code-body">
                    <div className="search-row-title">{c.owner}/{c.repo}/{c.path}:{c.line}</div>
                    <code className="search-row-line">{c.content}</code>
                  </div>
                </button>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  )
}

function FileCodeIconPlaceholder() {
  return <span className="search-row-icon">📄</span>
}

/**
 * A minimal confirmation dialog for destructive actions (deleting a user,
 * revoking a token, …). Renders as a modal overlay; click on the backdrop
 * or the cancel button to dismiss. `busy` disables the confirm button while
 * the destructive request is in flight.
 */
export function ConfirmDialog({
  title,
  message,
  confirmLabel,
  busy = false,
  onConfirm,
  onCancel,
}: {
  title: string
  message: string
  confirmLabel: string
  busy?: boolean
  onConfirm: () => void
  onCancel: () => void
}) {
  const { t } = useI18n()
  return (
    <div className="modal-overlay" onClick={onCancel}>
      <div
        className="modal-card"
        role="dialog"
        aria-modal="true"
        aria-label={title}
        onClick={(e) => e.stopPropagation()}
      >
        <h3 className="modal-title">{title}</h3>
        <p className="modal-message">{message}</p>
        <div className="modal-actions">
          <button className="btn btn-ghost" onClick={onCancel}>
            {t('common.cancel')}
          </button>
          <button className="btn btn-danger" disabled={busy} onClick={onConfirm}>
            {confirmLabel}
          </button>
        </div>
      </div>
    </div>
  )
}

import { useState, type ReactNode, useRef, useEffect } from 'react'
import {
  AlertTriangleIcon,
  BookIcon,
  CircleCheckIcon,
  CircleDotIcon,
  GitCommitIcon,
  GitPullRequestIcon,
  HomeIcon,
  PersonIcon,
  RefreshIcon,
  SettingsIcon,
  ShieldAlertIcon,
  UsersIcon,
  XIcon,
} from './Icons'
import { useAuth } from '../hooks/useAuth'
import { useI18n } from '../i18n'

export type NavTarget =
  | 'home'
  | 'files'
  | 'commits'
  | 'prs'
  | 'issues'

/** Top-level navigation targets shown on the global (home) sidebar. */
export type GlobalNavTarget = 'projects' | 'groups' | 'issues'

const GLOBAL_NAV: { key: GlobalNavTarget; labelKey: string; icon: typeof HomeIcon }[] = [
  { key: 'projects', labelKey: 'nav.projects', icon: BookIcon },
  { key: 'groups', labelKey: 'nav.groups', icon: UsersIcon },
  { key: 'issues', labelKey: 'nav.issuesGlobal', icon: CircleDotIcon },
]

const SIDEBAR_NAV: { key: NavTarget; labelKey: string; icon: typeof HomeIcon; countKey?: 'prs' | 'issues' }[] = [
  { key: 'home', labelKey: 'nav.home', icon: HomeIcon },
  { key: 'files', labelKey: 'nav.files', icon: BookIcon },
  { key: 'commits', labelKey: 'nav.commits', icon: GitCommitIcon },
  { key: 'prs', labelKey: 'nav.prs', icon: GitPullRequestIcon, countKey: 'prs' },
  { key: 'issues', labelKey: 'nav.issues', icon: CircleDotIcon, countKey: 'issues' },
]

export function Brand({ compact = false }: { compact?: boolean }) {
  return (
    <a className="brand" href="#" onClick={(e) => { e.preventDefault(); onNavigateHome() }}>
      <span className="brand-mark">
        <svg width={compact ? 20 : 18} height={compact ? 20 : 18} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.4" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
          <path d="M6 3v12" />
          <circle cx="18" cy="6" r="3" />
          <circle cx="6" cy="18" r="3" />
          <path d="M18 9a9 9 0 0 1-9 9" />
        </svg>
      </span>
      <span>Gita</span>
    </a>
  )
}

function Avatar({ username }: { username: string }) {
  return <span className="user-avatar">{username.slice(0, 1).toUpperCase()}</span>
}

function UserMenu({ user, onClose }: { user: { username: string; email: string } | null; onClose: () => void }) {
  const { t } = useI18n()
  const handleLogout = () => {
    onClose()
    // 这里使用 setTimeout 让菜单先关闭，再执行登出
    setTimeout(() => {
      const event = new CustomEvent('gita:logout')
      window.dispatchEvent(event)
    }, 0)
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
          <button className="user-menu-item" onClick={() => { onClose(); onOpenSettings() }}>
            <SettingsIcon size={16} />
            <span>{t('common.settings')}</span>
          </button>
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

function SearchIcon({ size = 16 }: { size?: number }) {
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
              <a href="#" onClick={(e) => { e.preventDefault(); onNavigateHome() }}>
                <HomeIcon size={14} />
                <span>{t('nav.home')}</span>
              </a>
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
      {/* Search placeholder */}
      <div className="search-box" role="search">
        <SearchIcon size={14} />
        <span>{t('common.search')}</span>
        <kbd>/</kbd>
      </div>
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

// Helper used by Topbar breadcrumbs — navigates back to repo list.
// Implemented via a module-level callback set by AppLayout.
let _onNavigateHome: (() => void) | null = null
export function setNavigateHome(fn: (() => void) | null) {
  _onNavigateHome = fn
}
function onNavigateHome() {
  if (_onNavigateHome) _onNavigateHome()
}

// Helper used by the user menu "Settings" item — opens the settings page.
// Implemented via a module-level callback registered by App.
let _onOpenSettings: (() => void) | null = null
export function setOpenSettings(fn: (() => void) | null) {
  _onOpenSettings = fn
}
function onOpenSettings() {
  if (_onOpenSettings) _onOpenSettings()
}

export type SidebarVariant = 'global' | 'repo'

export function Sidebar({
  variant,
  active,
  onNavigate,
  globalActive,
  onGlobalNavigate,
  counts,
}: {
  variant: SidebarVariant
  active?: NavTarget
  onNavigate?: (target: NavTarget) => void
  globalActive?: GlobalNavTarget
  onGlobalNavigate?: (target: GlobalNavTarget) => void
  counts?: Partial<Record<'prs' | 'issues', number>>
}) {
  const { t } = useI18n()
  return (
    <aside className="sidebar">
      <nav className="sidebar-nav" aria-label={t('common.mainNav')}>
        {variant === 'global' &&
          GLOBAL_NAV.map((item) => {
            const Icon = item.icon
            const isActive = globalActive === item.key
            return (
              <button
                key={item.key}
                className={`sidebar-link${isActive ? ' active' : ''}`}
                onClick={() => onGlobalNavigate?.(item.key)}
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
            const isActive = active === item.key
            return (
              <button
                key={item.key}
                className={`sidebar-link${isActive ? ' active' : ''}`}
                onClick={() => onNavigate?.(item.key)}
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
  onNavigate,
  globalActive,
  onGlobalNavigate,
  repo,
  counts,
  children,
}: {
  variant?: SidebarVariant
  active?: NavTarget
  onNavigate?: (target: NavTarget) => void
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

  const handleNavigate = (target: NavTarget) => {
    setSidebarOpen(false)
    onNavigate?.(target)
  }

  const handleGlobalNavigate = (target: GlobalNavTarget) => {
    setSidebarOpen(false)
    onGlobalNavigate?.(target)
  }

  // 注册"返回全局首页"回调，供顶栏面包屑使用
  setNavigateHome(() => onGlobalNavigate?.('projects'))

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
            onNavigate={handleNavigate}
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
  let cls = 'status'
  let Icon = GitPullRequestIcon
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
      {status}
    </span>
  )
}
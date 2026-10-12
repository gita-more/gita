import { useState, type ReactNode } from 'react'
import {
  BookIcon,
  ChevronRightIcon,
  CircleCheckIcon,
  CircleDotIcon,
  GitCommitIcon,
  GitPullRequestIcon,
  HomeIcon,
  LogOutIcon,
  RefreshIcon,
  SearchIcon,
  SparklesIcon,
  ShieldAlertIcon,
  XIcon,
} from './Icons'
import { useAuth } from '../hooks/useAuth'

export type NavTarget =
  | 'home'
  | 'files'
  | 'commits'
  | 'prs'
  | 'issues'
  | 'reviews'
  | 'search'

const SIDEBAR_NAV: { key: NavTarget; label: string; icon: typeof HomeIcon; countKey?: 'prs' | 'issues' }[] = [
  { key: 'home', label: '仓库', icon: HomeIcon },
  { key: 'files', label: '代码', icon: BookIcon },
  { key: 'commits', label: '提交', icon: GitCommitIcon },
  { key: 'prs', label: '合并请求', icon: GitPullRequestIcon, countKey: 'prs' },
  { key: 'issues', label: 'Issue', icon: CircleDotIcon },
  { key: 'reviews', label: 'AI 评审', icon: SparklesIcon },
  { key: 'search', label: '代码搜索', icon: SearchIcon },
]

export function Brand() {
  return (
    <a className="brand" href="#" onClick={(e) => e.preventDefault()}>
      <span className="brand-mark">
        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.4" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
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

export function Topbar({
  repo,
  actions,
}: {
  repo?: { owner: string; name: string }
  actions?: ReactNode
}) {
  const { user, logout } = useAuth()

  return (
    <header className="topbar">
      <Brand />
      {repo && (
        <>
          <span className="topbar-divider" />
          <div className="topbar-repo">
            <span>{repo.owner}</span>
            <ChevronRightIcon size={14} />
            <a href="#" onClick={(e) => e.preventDefault()}>{repo.name}</a>
          </div>
        </>
      )}
      <div className="topbar-spacer" />
      <div className="topbar-actions">
        {actions}
        {user && (
          <span className="user-chip">
            <Avatar username={user.username} />
            {user.username}
          </span>
        )}
        <button
          className="btn btn-ghost btn-sm"
          title="退出登录"
          onClick={logout}
          style={{ display: 'inline-flex', alignItems: 'center', gap: 4 }}
        >
          <LogOutIcon size={14} />
        </button>
      </div>
    </header>
  )
}

export function Sidebar({
  active,
  onNavigate,
  counts,
}: {
  active: NavTarget
  onNavigate: (target: NavTarget) => void
  counts?: Partial<Record<'prs' | 'issues', number>>
}) {
  return (
    <aside className="sidebar">
      <div className="sidebar-group">
        <div className="sidebar-group-label">仓库</div>
        {SIDEBAR_NAV.map((item) => {
          const Icon = item.icon
          const isActive = active === item.key
          return (
            <button
              key={item.key}
              className={`sidebar-link${isActive ? ' active' : ''}`}
              onClick={() => onNavigate(item.key)}
              title={item.label}
            >
              <Icon size={16} />
              <span>{item.label}</span>
              {item.countKey && counts?.[item.countKey] != null && counts[item.countKey]! > 0 && (
                <span className="count">{counts[item.countKey]}</span>
              )}
            </button>
          )
        })}
      </div>
      <div className="sidebar-footer">
        <span className="version">v0.0.1</span>
        <a href="/api/docs" target="_blank" rel="noreferrer" title="OpenAPI 文档">
          API
        </a>
      </div>
    </aside>
  )
}

export function AppLayout({
  active,
  onNavigate,
  repo,
  counts,
  children,
}: {
  active: NavTarget
  onNavigate: (target: NavTarget) => void
  repo?: { owner: string; name: string }
  counts?: Partial<Record<'prs' | 'issues', number>>
  children: ReactNode
}) {
  const [sidebarOpen, setSidebarOpen] = useState(false)
  const { logout } = useAuth()

  const handleNavigate = (target: NavTarget) => {
    setSidebarOpen(false)
    onNavigate(target)
  }

  return (
    <>
      <Topbar
        repo={repo}
        actions={
          <button
            className="sidebar-toggle"
            onClick={() => setSidebarOpen((v) => !v)}
            title="切换菜单"
          >
            {sidebarOpen ? <XIcon size={14} /> : <MenuIcon size={14} />}
          </button>
        }
      />
      <div className="app">
        <div className={`sidebar-overlay${sidebarOpen ? ' open' : ''}`} onClick={() => setSidebarOpen(false)} />
        <div className={`sidebar${sidebarOpen ? ' open' : ''}`}>
          <div className="sidebar-group">
            <div className="sidebar-group-label">仓库</div>
            {SIDEBAR_NAV.map((item) => {
              const Icon = item.icon
              const isActive = active === item.key
              return (
                <button
                  key={item.key}
                  className={`sidebar-link${isActive ? ' active' : ''}`}
                  onClick={() => handleNavigate(item.key)}
                >
                  <Icon size={16} />
                  <span>{item.label}</span>
                  {item.countKey && counts?.[item.countKey] != null && counts[item.countKey]! > 0 && (
                    <span className="count">{counts[item.countKey]}</span>
                  )}
                </button>
              )
            })}
          </div>
          <div className="sidebar-footer">
            <span className="version">v0.0.1</span>
            <a href="/api/docs" target="_blank" rel="noreferrer">API</a>
            <button
              onClick={() => {
                setSidebarOpen(false)
                logout()
              }}
              className="sidebar-link"
              style={{ borderLeft: 'none', padding: '4px 0', width: 'auto' }}
            >
              <LogOutIcon size={14} />
              <span>退出</span>
            </button>
          </div>
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
    type === 'error' ? ShieldAlertIcon : type === 'warning' ? SparklesIcon : type === 'success' ? CircleCheckIcon : RefreshIcon
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
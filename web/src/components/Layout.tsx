import { useState, type ReactNode } from 'react'
import {
  BookIcon,
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
  { key: 'issues', label: 'Issue', icon: CircleDotIcon, countKey: 'issues' },
  { key: 'reviews', label: 'AI 评审', icon: SparklesIcon },
  { key: 'search', label: '代码搜索', icon: SearchIcon },
]

export function Brand({ compact = false }: { compact?: boolean }) {
  return (
    <a className="brand" href="#" onClick={(e) => e.preventDefault()}>
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
  return (
    <header className="topbar">
      {onToggleSidebar && (
        <button
          className="sidebar-toggle"
          onClick={onToggleSidebar}
          title="切换菜单"
          aria-label="切换菜单"
          aria-expanded={sidebarOpen}
        >
          {sidebarOpen ? <XIcon size={16} /> : <MenuIcon size={16} />}
        </button>
      )}
      {repo ? (
        <nav className="breadcrumbs" aria-label="Breadcrumb">
          <ol className="breadcrumb-list">
            <li className="breadcrumb-item">
              <a href="#" onClick={(e) => { e.preventDefault(); onNavigateHome() }}>
                <HomeIcon size={14} />
                <span>仓库</span>
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
      <div className="topbar-actions">
        {actions}
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

export function Sidebar({
  active,
  onNavigate,
  counts,
  onToggleCollapse,
  collapsed,
}: {
  active: NavTarget
  onNavigate: (target: NavTarget) => void
  counts?: Partial<Record<'prs' | 'issues', number>>
  onToggleCollapse?: () => void
  collapsed?: boolean
}) {
  const { user, logout } = useAuth()

  return (
    <aside className="sidebar">
      {/* GitLab Super Sidebar 顶部用户区：Logo + 折叠图标 + 新建按钮 */}
      <div className="sidebar-user-bar">
        <Brand compact />
        <button
          className="btn btn-ghost btn-sm sidebar-toggle-btn"
          onClick={onToggleCollapse}
          title={collapsed ? '展开侧边栏' : '折叠侧边栏'}
          aria-label={collapsed ? '展开侧边栏' : '折叠侧边栏'}
        >
          <PanelLeftIcon size={15} />
        </button>
        <button
          className="btn btn-ghost btn-sm sidebar-new-btn"
          title="新建仓库"
          aria-label="新建仓库"
          onClick={() => onNavigate('home')}
        >
          <PlusIcon size={15} />
        </button>
      </div>

      <nav className="sidebar-nav" aria-label="主导航">
        <div className="sidebar-group-label">导航</div>
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
      </nav>

      {/* 底部用户区：头像 + 用户名 + 退出 + ^ */}
      <div className="sidebar-user-bottom">
        <button className="sidebar-user-bottom-btn" onClick={logout} title="退出登录">
          <Avatar username={user?.username ?? 'U'} />
          <span className="sidebar-user-name">{user?.username ?? '未登录'}</span>
          <LogOutIcon size={13} className="sidebar-user-bottom-logout" />
        </button>
        <button className="sidebar-user-bottom-caret" title="帮助" aria-label="帮助">
          <ChevronUpIcon size={14} />
        </button>
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
    onNavigate(target)
  }

  // 注册"返回仓库列表"回调，供顶栏面包屑使用
  setNavigateHome(() => onNavigate('home'))

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
            active={active}
            onNavigate={handleNavigate}
            counts={counts}
            collapsed={collapsed}
            onToggleCollapse={handleToggleSidebar}
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

function PlusIcon({ size = 16 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <line x1="12" y1="5" x2="12" y2="19" />
      <line x1="5" y1="12" x2="19" y2="12" />
    </svg>
  )
}

function PanelLeftIcon({ size = 16 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <rect x="3" y="3" width="18" height="18" rx="2" />
      <line x1="9" y1="3" x2="9" y2="21" />
    </svg>
  )
}

function ChevronUpIcon({ size = 16 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <polyline points="18 15 12 9 6 15" />
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
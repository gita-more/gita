import { useEffect, type ReactNode, useState } from 'react'
import {
  Link,
  Navigate,
  Outlet,
  Route,
  Routes,
  useLocation,
  useNavigate,
  useParams,
} from 'react-router-dom'
import { LoginPage } from './components/LoginPage'
import { InitPage } from './components/InitPage'
import { Projects } from './components/Projects'
import { Groups } from './components/Groups'
import { Issues } from './components/Issues'
import { GroupDetail } from './components/GroupDetail'
import { RepoDetail } from './components/RepoDetail'
import { Settings } from './components/Settings'
import { useAuth } from './hooks/useAuth'
import { useI18n } from './i18n'
import { api } from './lib/api'
import { AppLayout, EmptyState } from './components/Layout'
import { FolderIcon } from './components/Icons'

/** Page-level outlet wrapper. Adds topbar + sidebar; child route renders inside. */
function Root({ children }: { children?: ReactNode } = {}) {
  const location = useLocation()
  const navigate = useNavigate()
  // Routes that show the repository sidebar:
  //   /:owner/:repo           → personal repo (owner = username)
  //   /:group/:repo           → group repo (owner = group full_path)
  //   /:owner                 → user profile or group detail
  // All other top-level routes (/, /groups, /issues, /settings) use the global sidebar.
  const repoMatch = location.pathname.match(/^\/([^/]+)\/([^/]+)$/)
  let variant: 'global' | 'repo' = 'global'
  let repoForBreadcrumb: { owner: string; name: string } | undefined

  if (repoMatch) {
    variant = 'repo'
    repoForBreadcrumb = { owner: repoMatch[1], name: repoMatch[2] }
  }

  return (
    <AppLayout
      variant={variant}
      repo={repoForBreadcrumb}
      onGlobalNavigate={(target) => {
        if (target === 'projects') navigate('/')
        else if (target === 'groups') navigate('/groups')
        else if (target === 'issues') navigate('/issues')
      }}
    >
      {children ?? <Outlet />}
    </AppLayout>
  )
}

/** Wrap routes that require an authenticated user. */
function Protected({ children }: { children: React.ReactNode }) {
  const { user } = useAuth()
  const location = useLocation()
  if (!user) {
    return <Navigate to="/login" state={{ from: location.pathname }} replace />
  }
  return <>{children}</>
}

/** Adapter: group detail by URL path (supports nested groups like groupa/subgroupa) */
function GroupDetailContainer() {
  const { owner } = useParams()
  const navigate = useNavigate()
  return (
    <GroupDetail
      group={owner ?? ''}
      onOpenRepo={(_owner, repoName) => navigate(`/${owner}/${repoName}`)}
    />
  )
}

/** Adapter: smart repo detail that detects if owner is group or user */
function SmartRepoDetailContainer() {
  const params = useParams()
  const navigate = useNavigate()
  const owner = params.owner ?? ''
  const repo = params.repo ?? ''
  const [repoKind, setRepoKind] = useState<'group' | 'personal' | 'loading'>('loading')

  useEffect(() => {
    async function detectType() {
      try {
        await api.getGroup(owner)
        setRepoKind('group')
      } catch {
        setRepoKind('personal')
      }
    }
    if (owner) detectType()
  }, [owner])

  if (repoKind === 'loading') {
    return (
      <Root>
        <div className="auth-page" style={{ display: 'flex', alignItems: 'center', justifyContent: 'center', minHeight: '200px' }}>
          Loading...
        </div>
      </Root>
    )
  }

  return (
    <RepoDetail
      owner={owner}
      name={repo}
      repoKind={repoKind}
      groupName={repoKind === 'group' ? owner : undefined}
      onBack={() => navigate(`/${owner}`)}
    />
  )
}

/** Settings page — no props; navigate from inside the page via Link/useNavigate. */
function SettingsRoute() {
  return <Settings />
}

/** Login route wrapper — stores the original destination so we can return there. */
function LoginRoute() {
  const navigate = useNavigate()
  const location = useLocation()
  return (
    <LoginPage
      onDone={() => {
        const from = (location.state as { from?: string } | null)?.from
        navigate(from && from !== '/login' ? from : '/')
      }}
    />
  )
}

/** 404 page; user can't reach it for known routes. */
function NotFound() {
  const { t } = useI18n()
  return (
    <Root>
      <EmptyState
        icon={<FolderIcon size={22} />}
        title="404"
        description={t('error.notFound')}
      />
    </Root>
  )
}

export default function App() {
  const { user } = useAuth()
  const navigate = useNavigate()
  const { t } = useI18n()
  const [initialized, setInitialized] = useState<boolean | null>(null)

  useEffect(() => {
    api.initialized().then(({ initialized }) => setInitialized(initialized)).catch(() => setInitialized(false))
  }, [])

  useEffect(() => {
    // No-op: the Protected wrapper handles it. Hook reserved for future use.
  }, [])

  if (initialized === null) {
    return (
      <div className="auth-page" style={{ display: 'flex', alignItems: 'center', justifyContent: 'center', minHeight: '100vh', background: '#0d1117' }}>
        {t('init.processing')}
      </div>
    )
  }

  if (!initialized) {
    return (
      <InitPage
        onDone={() => {
          setInitialized(true)
          navigate('/', { replace: true })
        }}
      />
    )
  }

  if (user && window.location.pathname === '/login') {
    return <Navigate to="/" replace />
  }

  return (
    <Routes>
      <Route path="/login" element={<LoginRoute />} />
      <Route path="/init" element={<InitPage onDone={() => navigate('/')} />} />

      <Route element={<Protected><Root /></Protected>}>
        <Route path="/" element={<Projects />} />
        <Route path="/groups" element={<Groups />} />
        <Route path="/issues" element={<Issues />} />
        <Route path="/settings" element={<SettingsRoute />} />
        <Route path="/:owner/:repo" element={<SmartRepoDetailContainer />} />
        <Route path="/:owner" element={<GroupDetailContainer />} />
        <Route path="*" element={<NotFound />} />
      </Route>
    </Routes>
  )
}

export { Link }
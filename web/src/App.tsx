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
  //   /groups/:name           → group detail
  //   /groups/:name/:repo     → group repo
  //   /:owner/:repo           → personal repo
  // All other top-level routes (/, /groups, /issues, /settings) use the global sidebar.
  const m = location.pathname.match(/^\/(?:groups\/([^/]+)(?:\/([^/]+))?|(?:([^/]+)\/([^/]+)))$/)
  let variant: 'global' | 'repo' = 'global'
  let repoForBreadcrumb: { owner: string; name: string } | undefined

  if (m) {
    if (m[1]) {
      // /groups/:name or /groups/:name/:repo
      variant = 'repo'
      repoForBreadcrumb = { owner: m[1], name: m[2] ?? '' }
    } else if (m[3] && m[4]) {
      // /:owner/:repo
      variant = 'repo'
      repoForBreadcrumb = { owner: m[3], name: m[4] }
    }
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

/** Adapter: group detail by URL :name */
function GroupDetailContainer() {
  const { name = '' } = useParams()
  const navigate = useNavigate()
  return (
    <GroupDetail
      group={name}
      onOpenRepo={(_owner, repoName) => navigate(`/groups/${name}/${repoName}`)}
    />
  )
}

/** Adapter: repo detail (personal or group) by URL params. The underlying
 *  RepoDetail manages its own internal tab state, but the ?tab= search param
 *  has been retired — sidebar tabs inside a repo still work via the legacy
 *  handleNavigate callback (see Notes). */
function RepoDetailContainer() {
  const params = useParams()
  const navigate = useNavigate()
  // When the URL matches /groups/:name/:repo (params.name is the group,
  // params.repo is the repo), forward group metadata so EmptyRepoWelcome can
  // build the correct clone URL.
  const isGroup = typeof params.name === 'string' && typeof params.repo === 'string' && params.owner === undefined
  return (
    <RepoDetail
      owner={params.owner ?? params.name ?? ''}
      name={params.repo ?? ''}
      repoKind={isGroup ? 'group' : 'personal'}
      groupName={isGroup ? params.name : undefined}
      onBack={() => {
        if (params.name) navigate(`/groups/${params.name}`)
        else navigate('/')
      }}
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
        // After successful login, send users back to wherever they came
        // from (or to the projects list as the default landing).
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

  // Probe whether any account exists yet, so we can show the init page on
  // first-ever use instead of the login page.
  useEffect(() => {
    api.initialized().then(({ initialized }) => setInitialized(initialized)).catch(() => setInitialized(false))
  }, [])

  // Stay backwards-compatible: when the user logs out, redirect to /login.
  useEffect(() => {
    // No-op: the Protected wrapper handles it. Hook reserved for future use.
  }, [])

  // Still probing whether the system has been initialized.
  if (initialized === null) {
    return (
      <div className="auth-page" style={{ display: 'flex', alignItems: 'center', justifyContent: 'center', minHeight: '100vh', background: '#0d1117' }}>
        {t('init.processing')}
      </div>
    )
  }

  // No account exists yet: create the first administrator.
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

  // If we have a user and the URL is /login, redirect to / (so the auth gate
  // doesn't trap a logged-in user on the login page).
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
        <Route path="/groups/:name" element={<GroupDetailContainer />} />
        <Route
          path="/groups/:name/:repo"
          element={<RepoDetailContainer />}
        />
        <Route path="/:owner/:repo" element={<RepoDetailContainer />} />
        <Route path="/settings" element={<SettingsRoute />} />
        <Route path="*" element={<NotFound />} />
      </Route>
    </Routes>
  )
}

// Re-export so it can be imported if needed.
export { Link }
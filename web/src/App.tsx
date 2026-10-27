import { createContext, useContext, useEffect, type ReactNode, useState } from 'react'
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
import { useErrorTranslator, useI18n } from './i18n'
import { api, type ResolvedPath } from './lib/api'
import { AppLayout, EmptyState } from './components/Layout'
import { FolderIcon } from './components/Icons'

/**
 * Lets page content override the sidebar variant for the *current* pathname.
 *
 * The default variant is URL-shaped (two segments = repo page), but a
 * two-segment URL can also be a subgroup path (/gita/tests) whose content is
 * a group detail page. The content knows which it is after its API checks,
 * so it reports back here; the override is keyed by pathname so it cannot
 * leak into a navigation to a different page.
 */
const SidebarVariantContext = createContext<(o: { path: string; variant: 'global' | 'repo' }) => void>(
  () => {},
)

/** Page-level outlet wrapper. Adds topbar + sidebar; child route renders inside. */
function Root({ children }: { children?: ReactNode } = {}) {
  const location = useLocation()
  const navigate = useNavigate()
  // Routes that show the repository sidebar:
  //   /:owner/:repo           → personal repo (owner = username)
  //   /:group/:repo           → group repo (owner = group full_path)
  //   /:owner                 → user profile or group detail
  // All other top-level routes (/, /groups, /issues, /settings) use the global sidebar.
  const segments = location.pathname.split('/').filter(Boolean)
  // Two or more segments default to the repo sidebar; the page content can
  // override that (see SidebarVariantContext) once it knows what the URL
  // addresses. Static top-level pages never show the repo sidebar.
  const isContentPath =
    segments.length >= 2 && !['groups', 'issues', 'settings'].includes(segments[0])

  const [sidebarOverride, setSidebarOverride] = useState<{ path: string; variant: 'global' | 'repo' } | null>(null)
  // Only honour an override made for the URL we are currently on.
  const variant =
    sidebarOverride && sidebarOverride.path === location.pathname
      ? sidebarOverride.variant
      : isContentPath
        ? 'repo'
        : 'global'
  // Hierarchical breadcrumb for any content path (repo or group alike).
  const crumbs = isContentPath ? segments : undefined

  return (
    <SidebarVariantContext.Provider value={setSidebarOverride}>
      <AppLayout
        variant={variant}
        crumbs={crumbs}
        onGlobalNavigate={(target) => {
          if (target === 'projects') navigate('/')
          else if (target === 'groups') navigate('/groups')
          else if (target === 'issues') navigate('/issues')
        }}
      >
        {children ?? <Outlet />}
      </AppLayout>
    </SidebarVariantContext.Provider>
  )
}

/** Declaratively pin the sidebar variant for the page rendering this component. */
function SetSidebarVariant({ variant }: { variant: 'global' | 'repo' }) {
  const setOverride = useContext(SidebarVariantContext)
  const { pathname } = useLocation()
  useEffect(() => {
    setOverride({ path: pathname, variant })
  }, [setOverride, pathname, variant])
  return null
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

/**
 * One catch-all content page for every namespace path.
 *
 * A clean URL is ambiguous by shape — /gita/tests may be a personal repo
 * under a user, a repo in a group, or a subgroup path — so the server
 * resolves it once (GET /api/resolve, repository first, then group) and
 * this component renders whatever the answer says. The sidebar variant
 * follows the resolved kind, not the URL shape.
 */
function PathPage() {
  const { '*': rawPath = '' } = useParams()
  const path = rawPath.replace(/\/+$/, '')
  const navigate = useNavigate()
  const translateError = useErrorTranslator()
  const [resolved, setResolved] = useState<ResolvedPath | null>(null)
  const [failed, setFailed] = useState('')

  useEffect(() => {
    let cancelled = false
    setResolved(null)
    setFailed('')
    api
      .resolvePath(path)
      .then((r) => {
        if (!cancelled) setResolved(r)
      })
      .catch((err) => {
        if (!cancelled) setFailed(translateError(err))
      })
    return () => {
      cancelled = true
    }
  }, [path, translateError])

  if (failed) {
    return <NotFound />
  }

  if (!resolved) {
    // Root (the route layout) is already mounted above; do not nest another.
    return (
      <div
        className="auth-page"
        style={{ display: 'flex', alignItems: 'center', justifyContent: 'center', minHeight: '200px' }}
      >
        Loading...
      </div>
    )
  }

  if (resolved.kind === 'group') {
    return (
      <>
        <SetSidebarVariant variant="global" />
        <GroupDetail
          group={resolved.full_path}
          onOpenRepo={(owner, name) => navigate(`/${owner}/${name}`)}
        />
      </>
    )
  }

  return (
    <>
      <SetSidebarVariant variant="repo" />
      <RepoDetail
        owner={resolved.owner}
        name={resolved.name}
        repoKind={resolved.owner_kind === 'group' ? 'group' : 'personal'}
        groupName={resolved.owner_kind === 'group' ? resolved.owner : undefined}
        onBack={() => navigate(resolved.owner_kind === 'group' ? `/${resolved.owner}` : '/')}
      />
    </>
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
        {/* One catch-all for every namespace path (personal repo, group
            repo, nested-group repo, group detail): the server resolves the
            URL's ambiguity once via /api/resolve. Unknown paths render the
            404 page from inside PathPage. Static routes above outrank the
            splat in React Router's ranking. */}
        <Route path="/*" element={<PathPage />} />
      </Route>
    </Routes>
  )
}

export { Link }
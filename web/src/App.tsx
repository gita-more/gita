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
import { RepoSettings } from './components/RepoSettings'
import { Settings } from './components/Settings'
import { useAuth } from './hooks/useAuth'
import { useErrorTranslator, useI18n } from './i18n'
import { api, type Repository } from './lib/api'
import { AppLayout, EmptyState } from './components/Layout'
import { FolderIcon } from './components/Icons'

/** Valid repo tab path segments (/owner/repo/:tab). */
export const REPO_TABS = ['files', 'commits', 'prs', 'issues', 'pipelines', 'pages'] as const
export type RepoTab = (typeof REPO_TABS)[number]

/** Valid settings path sections (/owner/repo/settings/:section). */
const SETTINGS_SECTIONS = ['general', 'branches', 'tags', 'pages'] as const

// ---------------------------------------------------------------------------
// RepoLayout: chrome for all repo-scoped routes (/owner/repo/*).
// Resolves the namespace once, renders AppLayout, children render in Outlet.
// ---------------------------------------------------------------------------

function RepoLayout() {
  const { owner, repo } = useParams<{ owner: string; repo: string }>()
  const translateError = useErrorTranslator()
  const [resolved, setResolved] = useState(false)
  const [failed, setFailed] = useState('')

  useEffect(() => {
    if (!owner || !repo) return
    let cancelled = false
    setResolved(false)
    setFailed('')

    api
      .resolvePath(`${owner}/${repo}`)
      .then(() => {
        if (!cancelled) setResolved(true)
      })
      .catch((err) => {
        if (!cancelled) setFailed(translateError(err))
      })
    return () => {
      cancelled = true
    }
  }, [owner, repo, translateError])

  if (failed) {
    return <NotFound />
  }

  if (!resolved) {
    return <Loading />
  }

  return (
    <AppLayout variant="repo" crumbs={[owner!, repo!]}>
      <Outlet />
    </AppLayout>
  )
}

function Loading() {
  return (
    <div
      className="auth-page"
      style={{ display: 'flex', alignItems: 'center', justifyContent: 'center', minHeight: '200px' }}
    >
      Loading...
    </div>
  )
}

// ---------------------------------------------------------------------------
// RepoDetailPage: one of the repo tabs (files/commits/prs/issues/pipelines).
// The index route passes tab = 'files'. Unknown tabs render 404.
// ---------------------------------------------------------------------------

function RepoDetailPage() {
  const { owner, repo, tab = 'files' } = useParams<{ owner: string; repo: string; tab: string }>()
  const navigate = useNavigate()
  const translateError = useErrorTranslator()
  const [repoData, setRepoData] = useState<Repository | null>(null)
  const [failed, setFailed] = useState('')

  useEffect(() => {
    if (!owner || !repo) return
    let cancelled = false
    setRepoData(null)
    setFailed('')
    api
      .getRepo(owner, repo)
      .then((r) => {
        if (!cancelled) setRepoData(r)
      })
      .catch((err) => {
        if (!cancelled) setFailed(translateError(err))
      })
    return () => {
      cancelled = true
    }
  }, [owner, repo, translateError])

  if (!(REPO_TABS as readonly string[]).includes(tab)) {
    return <NotFound />
  }

  if (failed) {
    return <NotFound />
  }

  if (!repoData) {
    return <Loading />
  }

  return (
    <RepoDetail
      owner={owner!}
      name={repo!}
      tab={tab as RepoTab}
      repoData={repoData}
      onBack={() => navigate('/')}
    />
  )
}

// ---------------------------------------------------------------------------
// RepoSettingsPage: the settings page (/owner/repo/settings[/section]).
// Unknown sections render 404.
// ---------------------------------------------------------------------------

function RepoSettingsPage() {
  const { owner, repo, section } = useParams<{
    owner: string
    repo: string
    section: string
  }>()
  const navigate = useNavigate()
  const translateError = useErrorTranslator()
  const [repoData, setRepoData] = useState<Repository | null>(null)
  const [failed, setFailed] = useState('')

  useEffect(() => {
    if (!owner || !repo) return
    let cancelled = false
    setRepoData(null)
    setFailed('')
    api
      .getRepo(owner, repo)
      .then((r) => {
        if (!cancelled) setRepoData(r)
      })
      .catch((err) => {
        if (!cancelled) setFailed(translateError(err))
      })
    return () => {
      cancelled = true
    }
  }, [owner, repo, translateError])

  // /settings normalizes to /settings/general so the URL always reflects
  // the active section. Must come after all hooks: React requires a stable
  // hook count between renders of the same component instance.
  if (!section) {
    return <Navigate to={`/${owner}/${repo}/settings/general`} replace />
  }

  if (!(SETTINGS_SECTIONS as readonly string[]).includes(section)) {
    return <NotFound />
  }

  if (failed) {
    return <NotFound />
  }

  if (!repoData) {
    return <Loading />
  }

  return (
    <RepoSettings
      owner={owner!}
      name={repo!}
      section={section}
      onSectionChange={(next) => navigate(`/${owner}/${repo}/settings/${next}`)}
      onBack={() => navigate(`/${owner}/${repo}`)}
    />
  )
}

/** Page-level layout route for global pages only (/, /groups, /issues,
 * /settings, and single-segment group paths like /gita). Repo routes
 * (/owner/repo/*) skip this layout — RepoLayout provides its own AppLayout,
 * so wrapping them here would render the chrome twice.
 */
function Root({ children }: { children?: ReactNode } = {}) {
  const location = useLocation()
  const navigate = useNavigate()
  const segments = location.pathname.split('/').filter(Boolean)
  // Global pages have at most one path segment; anything deeper is a repo
  // (or repo sub-page) owned by RepoLayout.
  const isGlobalPage = segments.length <= 1

  if (!isGlobalPage) {
    // Repo route: the nested RepoLayout renders the chrome. Pass through.
    return <>{children ?? <Outlet />}</>
  }

  return (
    <AppLayout
      variant="global"
      crumbs={segments.length > 0 ? segments : undefined}
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

        {/* Group detail: /:group (single segment) */}
        <Route path="/:group" element={<GroupDetail />} />

        {/* Repository routes: /:owner/:repo/*. RepoLayout renders the
            chrome (resolve + AppLayout); children render in its Outlet. */}
        <Route path="/:owner/:repo" element={<RepoLayout />}>
          {/* Repo detail tabs as path segments; the index route is "files". */}
          <Route index element={<RepoDetailPage />} />
          <Route path=":tab" element={<RepoDetailPage />} />
          {/* Repo settings: /owner/repo/settings[/section] */}
          <Route path="settings" element={<RepoSettingsPage />} />
          <Route path="settings/:section" element={<RepoSettingsPage />} />
        </Route>
      </Route>
    </Routes>
  )
}

export { Link }
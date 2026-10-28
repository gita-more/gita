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
  useSearchParams,
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
import { api, type ResolvedPath, type Repository } from './lib/api'
import { AppLayout, EmptyState } from './components/Layout'
import { FolderIcon } from './components/Icons'

// ---------------------------------------------------------------------------
// RepoLayout: the shell for all repo-scoped routes (/owner/repo/*).
// It resolves the repo once, provides breadcrumbs, and renders the outlet
// (RepoDetail for tabs, RepoSettings for settings).
// ---------------------------------------------------------------------------

function RepoLayout() {
  const { owner, repo } = useParams<{ owner: string; repo: string }>()
  const navigate = useNavigate()
  const translateError = useErrorTranslator()
  const [resolved, setResolved] = useState<ResolvedPath | null>(null)
  const [failed, setFailed] = useState('')
  const { search } = useLocation()

  useEffect(() => {
    if (!owner || !repo) return
    let cancelled = false
    setResolved(null)
    setFailed('')

    api
      .resolvePath(`${owner}/${repo}`)
      .then((r) => {
        if (!cancelled) setResolved(r)
      })
      .catch((err) => {
        if (!cancelled) setFailed(translateError(err))
      })
    return () => {
      cancelled = true
    }
  }, [owner, repo, translateError])

  // Any repo-tab query means the user wants the repo detail page, not settings.
  // Redirect to the repo root with that tab.
  const tab = new URLSearchParams(search).get('tab')
  const REPO_TABS = ['files', 'commits', 'prs', 'issues', 'pipelines', 'pages']
  if (tab && REPO_TABS.includes(tab)) {
    navigate(`/${owner}/${repo}?tab=${tab}`, { replace: true })
    return null
  }

  if (failed) {
    return <NotFound />
  }

  if (!resolved) {
    return (
      <div
        className="auth-page"
        style={{ display: 'flex', alignItems: 'center', justifyContent: 'center', minHeight: '200px' }}
      >
        Loading...
      </div>
    )
  }

  // Build breadcrumbs: ["", owner, repo]
  const crumbs = ['', owner!, repo!]

  return (
    <AppLayout variant="repo" crumbs={crumbs}>
      <Outlet />
    </AppLayout>
  )
}

// ---------------------------------------------------------------------------
// RepoContentLayout: wraps the repo detail tabs (files, commits, etc.).
// ---------------------------------------------------------------------------

function RepoContentLayout() {
  const { owner, repo } = useParams<{ owner: string; repo: string }>()
  const translateError = useErrorTranslator()
  const [repoData, setRepoData] = useState<Repository | null>(null)
  const [failed, setFailed] = useState('')

  useEffect(() => {
    if (!owner || !repo) return
    let cancelled = false
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

  if (failed) {
    return <NotFound />
  }

  if (!repoData) {
    return (
      <div
        className="auth-page"
        style={{ display: 'flex', alignItems: 'center', justifyContent: 'center', minHeight: '200px' }}
      >
        Loading...
      </div>
    )
  }

  return (
    <>
      <RepoDetail
        owner={owner!}
        name={repo!}
        repoData={repoData}
        onBack={() => window.history.back()}
      />
    </>
  )
}

// ---------------------------------------------------------------------------
// RepoSettingsLayout: wraps the repo settings page.
// ---------------------------------------------------------------------------

function RepoSettingsLayout() {
  const { owner, repo } = useParams<{ owner: string; repo: string }>()
  const [searchParams, setSearchParams] = useSearchParams()
  const translateError = useErrorTranslator()
  const [repoData, setRepoData] = useState<Repository | null>(null)
  const [failed, setFailed] = useState('')

  useEffect(() => {
    if (!owner || !repo) return
    let cancelled = false
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

  if (failed) {
    return <NotFound />
  }

  if (!repoData) {
    return (
      <div
        className="auth-page"
        style={{ display: 'flex', alignItems: 'center', justifyContent: 'center', minHeight: '200px' }}
      >
        Loading...
      </div>
    )
  }

  const section = searchParams.get('section')

  return (
    <>
      <RepoSettings
        owner={owner!}
        name={repo!}
        section={section}
        onSectionChange={(next) => setSearchParams({ section: next }, { replace: true })}
        onBack={() => window.history.back()}
      />
    </>
  )
}

/** Page-level outlet wrapper. Adds topbar + sidebar; child route renders inside. */
function Root({ children }: { children?: ReactNode } = {}) {
  const location = useLocation()
  const navigate = useNavigate()
  const segments = location.pathname.split('/').filter(Boolean)
  const isContentPath =
    segments.length >= 2 && !['groups', 'issues', 'settings'].includes(segments[0])
  const variant = isContentPath ? 'repo' : 'global'
  const crumbs = isContentPath ? segments : undefined

  return (
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

        {/* Group detail: /:group */}
        <Route path="/:group" element={<GroupDetail />} />

        {/* Repository routes: /:owner/:repo/* */}
        <Route path="/:owner/:repo" element={<RepoLayout />}>
          {/* Repo detail tabs: /:owner/:repo?tab=... */}
          <Route index element={<RepoContentLayout />} />
          <Route path=":tab(files|commits|prs|issues|pipelines|pages)" element={<RepoContentLayout />} />
          {/* Repo settings: /:owner/:repo/settings[?section=...] */}
          <Route path="settings" element={<RepoSettingsLayout />} />
        </Route>

        {/* Group repos: /:group/:repo/* */}
        <Route path="/:group/:repo" element={<RepoLayout />}>
          <Route index element={<RepoContentLayout />} />
          <Route path=":tab(files|commits|prs|issues|pipelines|pages)" element={<RepoContentLayout />} />
          <Route path="settings" element={<RepoSettingsLayout />} />
        </Route>
      </Route>
    </Routes>
  )
}

export { Link }
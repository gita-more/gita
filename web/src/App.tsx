import { useEffect, type ReactNode, useState } from 'react'
import {
  Link,
  Navigate,
  Outlet,
  Route,
  Routes,
  useLocation,
  useNavigate,
} from 'react-router-dom'
import { LoginPage } from './components/LoginPage'
import { InitPage } from './components/InitPage'
import { Projects } from './components/Projects'
import { Groups } from './components/Groups'
import { Issues } from './components/Issues'
import { GroupDetail } from './components/GroupDetail'
import { RepoDetail } from './components/RepoDetail'
import { RepoSettings } from './components/RepoSettings'
import { SettingsLanguage, SettingsTokens } from './components/Settings'
import { AdminUsersPage, AdminVmPage } from './components/Admin'
import { useAuth } from './hooks/useAuth'
import { useErrorTranslator, useI18n } from './i18n'
import { api, type Repository } from './lib/api'
import {
  resolveNamespace,
  type NamespaceTarget,
} from './lib/namespace'
import { AppLayout, EmptyState, type Crumb, type SidebarSection } from './components/Layout'
import {
  SETTINGS_SECTIONS as SETTINGS_PAGE_SECTIONS,
  ADMIN_SECTIONS as ADMIN_PAGE_SECTIONS,
} from './components/Layout'
import {
  CircleDotIcon,
  FolderIcon,
  SettingsIcon,
  ShieldIcon,
} from './components/Icons'

/** Valid repo tab path segments (/owner/repo/:tab). */
export const REPO_TABS = ['files', 'commits', 'prs', 'issues', 'pipelines', 'pages'] as const
export type RepoTab = (typeof REPO_TABS)[number]

/** Valid settings path sections (/owner/repo/settings/:section). */
const SETTINGS_SECTIONS = ['general', 'branches', 'tags', 'pages'] as const

// ---------------------------------------------------------------------------
// NamespaceLayout: chrome for every path that addresses a namespace.
//
// A clean URL is ambiguous by shape: `/gita` is a group, `/gita/tests` could
// be the repository `tests` in the namespace `gita` or the nested group with
// that full path. The server owns the tie-break (see /api/resolve: a
// repository wins over a group), so the SPA asks it once and renders
// whatever it says. The URL itself stays clean — no `/groups` prefix, no
// `/repos` prefix — while the topbar shows the hierarchy.
// ---------------------------------------------------------------------------

function NamespaceLayout() {
  const location = useLocation()
  const translateError = useErrorTranslator()
  const [target, setTarget] = useState<NamespaceTarget | null>(null)
  const [failed, setFailed] = useState(false)

  // The full path under the root — "gita/tests", "owner/repo", …
  const path = location.pathname.split('/').filter(Boolean).join('/')

  useEffect(() => {
    if (!path) return
    let cancelled = false
    setTarget(null)
    setFailed(false)

    resolveNamespace(path, (p) => api.resolvePath(p))
      .then((t) => {
        if (!cancelled) setTarget(t)
      })
      .catch((err) => {
        if (!cancelled) {
          setFailed(true)
          translateError(err)
        }
      })
    return () => {
      cancelled = true
    }
  }, [path, translateError])

  if (failed || path === '') {
    return <NotFound />
  }

  if (!target) {
    return <Loading />
  }

  // A group: render the group page with the group chrome.
  if (target.kind === 'group') {
    return (
      <AppLayout
        variant="global"
        crumbs={[
          { label: target.fullPath, path: `/${target.fullPath}`, icon: FolderIcon },
        ]}
      >
        <GroupDetail groupPath={target.fullPath} />
      </AppLayout>
    )
  }

  // A repository: the repo chrome, with the tab or settings page resolved
  // from the trailing path.
  const { owner, name, sub } = target
  const crumbs: Crumb[] = [
    { label: owner, path: `/${owner}` },
    { label: name, path: `/${owner}/${name}` },
  ]
  if (sub?.type === 'settings') {
    return (
      <AppLayout variant="repo" crumbs={crumbs}>
        <RepoSettingsPage
          owner={owner}
          name={name}
          section={sub.section ?? 'general'}
        />
      </AppLayout>
    )
  }
  return (
    <AppLayout variant="repo" crumbs={crumbs}>
      <RepoDetailPage
        owner={owner}
        name={name}
        tab={sub?.type === 'tab' ? sub.tab : 'files'}
      />
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

function RepoDetailPage({ owner, name, tab }: { owner: string; name: string; tab: string }) {
  const navigate = useNavigate()
  const translateError = useErrorTranslator()
  const [repoData, setRepoData] = useState<Repository | null>(null)
  const [failed, setFailed] = useState('')

  useEffect(() => {
    if (!owner || !name) return
    let cancelled = false
    setRepoData(null)
    setFailed('')
    api
      .getRepo(owner, name)
      .then((r) => {
        if (!cancelled) setRepoData(r)
      })
      .catch((err) => {
        if (!cancelled) setFailed(translateError(err))
      })
    return () => {
      cancelled = true
    }
  }, [owner, name, translateError])

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
      owner={owner}
      name={name}
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

function RepoSettingsPage({ owner, name, section }: { owner: string; name: string; section: string }) {
  const navigate = useNavigate()
  const translateError = useErrorTranslator()
  const [repoData, setRepoData] = useState<Repository | null>(null)
  const [failed, setFailed] = useState('')

  useEffect(() => {
    if (!owner || !name) return
    let cancelled = false
    setRepoData(null)
    setFailed('')
    api
      .getRepo(owner, name)
      .then((r) => {
        if (!cancelled) setRepoData(r)
      })
      .catch((err) => {
        if (!cancelled) setFailed(translateError(err))
      })
    return () => {
      cancelled = true
    }
  }, [owner, name, translateError])

  // /settings normalizes to /settings/general so the URL always reflects
  // the active section. Must come after all hooks: React requires a stable
  // hook count between renders of the same component instance.
  if (!section) {
    return <Navigate to={`/${owner}/${name}/settings/general`} replace />
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
      owner={owner}
      name={name}
      section={section}
      onSectionChange={(next) => navigate(`/${owner}/${name}/settings/${next}`)}
      onBack={() => navigate(`/${owner}/${name}`)}
    />
  )
}

/**
 * The label and icon a global page shows in the breadcrumb.
 *
 * The known top-level routes get their localized name ("Settings", not
 * "settings") and their own icon; anything else is a group path (e.g.
 * /gita) and is shown verbatim, because it is a name, not a route.
 */
const GLOBAL_CRUMB_LABELS: Record<string, string> = {
  groups: 'nav.groups',
  issues: 'nav.issuesGlobal',
  settings: 'common.settings',
  admin: 'nav.admin',
}

/** The icon each top-level route shows in the breadcrumb. */
const GLOBAL_CRUMB_ICONS: Record<string, typeof FolderIcon> = {
  groups: FolderIcon,
  issues: CircleDotIcon,
  settings: SettingsIcon,
  admin: ShieldIcon,
}

/** Page-local sidebar sections keyed by the top-level route segment. */
const PAGE_SECTIONS: Record<string, SidebarSection[]> = {
  settings: SETTINGS_PAGE_SECTIONS,
  admin: ADMIN_PAGE_SECTIONS,
}

function sidebarSections(firstSegment: string | undefined): SidebarSection[] | undefined {
  return firstSegment ? PAGE_SECTIONS[firstSegment] : undefined
}

/**
 * The breadcrumb of a global page.
 *
 * A page with sub-sections (Settings, Administration) shows two levels:
 * the section itself, then the section's own item. Only the first level
 * carries an icon — it identifies the page; deeper levels are the sections
 * inside it.
 */
export function globalCrumbs(segments: string[], t: (key: string) => string): Crumb[] {
  const section = PAGE_SECTIONS[segments[0]]
  const label = (seg: string) =>
    GLOBAL_CRUMB_LABELS[seg] ? t(GLOBAL_CRUMB_LABELS[seg]) : seg

  if (section && segments.length > 1) {
    const group = section[0]
    const item = group.items.find((i) => i.key === segments[1])
    const crumbs: Crumb[] = [
      {
        label: t(group.groupLabelKey),
        path: `/${segments[0]}/${group.items[0].key}`,
        icon: group.icon,
      },
    ]
    if (item) {
      crumbs.push({
        label: t(item.labelKey),
        path: `/${segments[0]}/${item.key}`,
      })
    }
    return crumbs
  }

  return segments.map((seg, i) => ({
    label: label(seg),
    path: `/${segments.slice(0, i + 1).join('/')}`,
    icon: i === 0 ? GLOBAL_CRUMB_ICONS[seg] : undefined,
  }))
}

/** The top-level routes that belong to the global chrome rather than to a
 * repository. Everything under them (e.g. /groups/gita) is a global page. */
const GLOBAL_PREFIXES = new Set(['groups', 'issues', 'settings', 'admin'])

/** Page-level layout route for global pages only (/projects, /groups/*,
 * /issues, /settings/*, /admin/*). Namespace paths (/gita, /gita/tests,
 * /owner/repo, …) skip this layout — NamespaceLayout resolves them via
 * /api/resolve and renders its own chrome.
 */
function Root({ children }: { children?: ReactNode } = {}) {
  const location = useLocation()
  const { t } = useI18n()
  const segments = location.pathname.split('/').filter(Boolean)
  // Global pages are the top-level lists and the sections that live in
  // their own sub-tree (/settings/language, /admin/users). Anything else
  // — including a single group segment like /gita — is a namespace path,
  // owned by NamespaceLayout.
  const isGlobalPage =
    segments.length === 0 || GLOBAL_PREFIXES.has(segments[0])

  if (!isGlobalPage) {
    // Namespace route: NamespaceLayout renders the chrome. Pass through.
    return <>{children ?? <Outlet />}</>
  }

  return (
    <AppLayout
      variant="global"
      crumbs={segments.length > 0 ? globalCrumbs(segments, t) : undefined}
      sections={sidebarSections(segments[0])}
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

/** /settings redirects to the first settings section. */
function SettingsIndexRoute() {
  return <Navigate to="/settings/language" replace />
}

/** The interface-language settings page. */
function SettingsLanguageRoute() {
  return <SettingsLanguage />
}

/** The personal access tokens settings page. */
function SettingsTokensRoute() {
  return <SettingsTokens />
}

/** /admin redirects to the first administration section. */
function AdminIndexRoute() {
  return <Navigate to="/admin/users" replace />
}

/** The user administration page. */
function AdminUsersRoute() {
  return <AdminUsersPage />
}

/** The VM cluster administration page. */
function AdminVmRoute() {
  return <AdminVmPage />
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
        data-testid="not-found"
        icon={<FolderIcon size={22} />}
        title="404"
        description={t('error.notFound')}
      />
    </Root>
  )
}

export default function App() {
  const { user, sync } = useAuth()
  const navigate = useNavigate()
  const { t } = useI18n()
  const [initialized, setInitialized] = useState<boolean | null>(null)

  useEffect(() => {
    api.initialized().then(({ initialized }) => setInitialized(initialized)).catch(() => setInitialized(false))
  }, [])

  // Reconcile the cached user with the server on startup: fields the server
  // owns (is_admin, locale) can change while the tab is closed, and the
  // cached copy in localStorage would otherwise stay stale — e.g. an admin
  // whose role was granted would not see the admin entry until they signed
  // in again.
  useEffect(() => {
    void sync()
  }, [sync])

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
        {/* Top-level lists are clean URLs, no functional prefix. */}
        <Route path="/" element={<Projects />} />
        <Route path="/groups" element={<Groups />} />
        <Route path="/issues" element={<Issues />} />

        {/* Settings: one page per section. /settings redirects to the
            first one. */}
        <Route path="/settings" element={<SettingsIndexRoute />} />
        <Route path="/settings/language" element={<SettingsLanguageRoute />} />
        <Route path="/settings/tokens" element={<SettingsTokensRoute />} />

        {/* Administration: one page per section. */}
        <Route path="/admin" element={<AdminIndexRoute />} />
        <Route path="/admin/users" element={<AdminUsersRoute />} />
        <Route path="/admin/vm" element={<AdminVmRoute />} />

        {/* Everything else addresses a namespace — a group or a repo,
            possibly nested (/gita/tests, /owner/repo). The shape alone is
            ambiguous, so NamespaceLayout asks /api/resolve once and renders
            whatever the server says. The URL stays clean, with no /groups
            or /repos prefix. */}
        <Route path="/*" element={<NamespaceLayout />} />
      </Route>
    </Routes>
  )
}

export { Link }

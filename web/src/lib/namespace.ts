// Namespace path resolution — the router's single source of truth for
// "what does this clean URL address?".
//
// A clean URL is ambiguous by shape: `/gita/tests` can be the repository
// `tests` in the namespace `gita`, or the nested group whose full path is
// `gita/tests`. The server owns the tie-break (/api/resolve: a repository
// wins over a group), and `resolveNamespace` is the thin client over it.
//
// The URL itself stays clean — no `/groups` or `/repos` prefix. The topbar
// shows the hierarchy; the URL is just the address.

import type { ResolvedPath } from './api'

/** Repo tabs that may trail a namespace path (/owner/repo/<tab>). */
export const REPO_TABS = ['files', 'commits', 'prs', 'issues', 'pipelines', 'pages'] as const

/** Repo settings sections (/owner/repo/settings/<section>). */
export const REPO_SETTINGS_SECTIONS = ['general', 'branches', 'tags', 'pages'] as const

/** A repo sub-page named by the trailing path segments. */
export type RepoSubPage =
  | { type: 'tab'; tab: string }
  | { type: 'settings'; section: string | null }

/** What a clean URL addresses. */
export type NamespaceTarget =
  | { kind: 'group'; fullPath: string }
  | { kind: 'repo'; owner: string; name: string; sub: RepoSubPage | null }

/**
 * The repo sub-page a trailing path names, if any.
 *
 * Only trailing segments that explicitly name a repo sub-page count: the
 * last segment being a repo tab, or `settings` (+ optional section). This
 * keeps `/gita/tests` from being read as group `gita` plus a bogus "tests"
 * tab.
 */
export function subPageOf(segments: string[]): RepoSubPage | null {
  const n = segments.length
  const last = segments[n - 1]
  if (n >= 2 && last === 'settings') {
    // /owner/repo/settings
    return { type: 'settings', section: null }
  }
  if (n >= 3 && segments[n - 2] === 'settings' && (REPO_SETTINGS_SECTIONS as readonly string[]).includes(last)) {
    // /owner/repo/settings/general
    return { type: 'settings', section: last }
  }
  if (n >= 3 && (REPO_TABS as readonly string[]).includes(last)) {
    // /owner/repo/commits
    return { type: 'tab', tab: last }
  }
  return null
}

/**
 * Resolve a clean URL to the entity it addresses.
 *
 * The full path is asked first — the server applies its documented rule
 * (repository wins over a group). If it misses, the path may still carry a
 * repo sub-page (`/owner/repo/commits`); the parent namespace is asked
 * then, and only a repository answer is accepted for it — a group has no
 * sub-pages.
 *
 * Returns null when nothing addresses the path.
 */
export async function resolveNamespace(
  path: string,
  resolve: (p: string) => Promise<ResolvedPath>,
): Promise<NamespaceTarget | null> {
  const segments = path.split('/').filter(Boolean)
  if (segments.length === 0) return null

  // The whole path as a namespace. The server's repo-first rule decides
  // `/gita/tests` here.
  try {
    const r = await resolve(path)
    if (r.kind === 'group') return { kind: 'group', fullPath: r.full_path }
    return { kind: 'repo', owner: r.owner, name: r.name, sub: null }
  } catch {
    // fall through to the sub-page interpretation
  }

  // The trailing segments may name a repo sub-page.
  const sub = subPageOf(segments)
  if (!sub) return null
  // How many trailing segments the sub-page occupies: a tab is one,
  // settings alone is one, settings + section is two.
  const subSegments =
    sub.type === 'settings' ? (sub.section === null ? 1 : 2) : 1
  const namespace = segments.slice(0, segments.length - subSegments).join('/')
  if (!namespace) return null
  try {
    const r = await resolve(namespace)
    // Only a repository has sub-pages; a group hit here means the URL
    // addresses something that does not exist.
    if (r.kind !== 'repo') return null
    return { kind: 'repo', owner: r.owner, name: r.name, sub }
  } catch {
    return null
  }
}

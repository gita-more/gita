import { Fragment, useState } from 'react'
import type { Group, Repository } from '../lib/api'
import { useErrorTranslator, useI18n } from '../i18n'
import {
  BookIcon,
  ChevronDownIcon,
  ChevronRightIcon,
  FolderIcon,
  GitBranchIcon,
} from './Icons'

/** One entry in a GitLab-style group tree: either a subgroup or a repository. */
export type GroupTreeChild =
  | { kind: 'group'; group: Group }
  | { kind: 'repo'; repo: Repository }

/**
 * Children provider for tree nodes. Returning an array means the children are
 * already known (static tree — the expand arrow only shows when the array is
 * non-empty); returning a Promise means they are lazy-loaded on first expand
 * (the arrow shows on every group row, like GitLab's group overview page).
 */
export type GroupTreeChildrenFn = (
  group: Group,
) => GroupTreeChild[] | Promise<GroupTreeChild[]>

/**
 * How the children of a group row are obtained.
 *
 * - `static`: the provider answers synchronously, so the tree already knows
 *   every child and the expand arrow only shows for groups that have some.
 * - `lazy`: the provider is only called when the row is expanded.
 *
 * The mode has to be declared rather than sniffed: telling the two apart
 * would mean calling the provider during render, which for a lazy provider
 * fires its request for every group row of the page instead of only for the
 * rows the user actually opens.
 */
export type GroupTreeChildrenMode = 'static' | 'lazy'

function isPromise<T>(value: T | Promise<T>): value is Promise<T> {
  return typeof (value as Promise<T> | undefined)?.then === 'function'
}

function byName(a: { name: string }, b: { name: string }): number {
  return a.name.localeCompare(b.name, undefined, { sensitivity: 'base' })
}

/** GitLab ordering: subgroups first, then projects, each alphabetical. */
function sortTreeChildren(children: GroupTreeChild[]): GroupTreeChild[] {
  const groups = children
    .filter((c): c is Extract<GroupTreeChild, { kind: 'group' }> => c.kind === 'group')
    .sort((a, b) => byName(a.group, b.group))
  const repos = children
    .filter((c): c is Extract<GroupTreeChild, { kind: 'repo' }> => c.kind === 'repo')
    .sort((a, b) => byName(a.repo, b.repo))
  return [...groups, ...repos]
}

export function GroupTree({
  nodes,
  getChildren,
  childrenMode = 'static',
  onOpenGroup,
  onOpenRepo,
}: {
  /** Top-level nodes of the tree. */
  nodes: GroupTreeChild[]
  /** How to obtain the children of a group row. */
  getChildren?: GroupTreeChildrenFn
  /** Whether the provider answers synchronously or lazily. */
  childrenMode?: GroupTreeChildrenMode
  onOpenGroup?: (fullPath: string) => void
  onOpenRepo?: (owner: string, name: string) => void
}) {
  return (
    <div className="group-tree">
      {sortTreeChildren(nodes).map((node) => (
        <GroupTreeNode
          key={node.kind === 'group' ? `g-${node.group.id}` : `r-${node.repo.id}`}
          node={node}
          getChildren={getChildren}
          childrenMode={childrenMode}
          onOpenGroup={onOpenGroup}
          onOpenRepo={onOpenRepo}
        />
      ))}
    </div>
  )
}

function GroupTreeNode({
  node,
  getChildren,
  childrenMode,
  onOpenGroup,
  onOpenRepo,
}: {
  node: GroupTreeChild
  getChildren?: GroupTreeChildrenFn
  childrenMode: GroupTreeChildrenMode
  onOpenGroup?: (fullPath: string) => void
  onOpenRepo?: (owner: string, name: string) => void
}) {
  const { t } = useI18n()
  const translateError = useErrorTranslator()
  const [open, setOpen] = useState(false)
  const [children, setChildren] = useState<GroupTreeChild[] | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState('')

  // A static provider answers synchronously, so the tree already knows every
  // child and the expand arrow only shows for groups that have some. A lazy
  // provider is not called at all until the row is expanded.
  let known: GroupTreeChild[] | null = null
  if (node.kind === 'group' && getChildren && childrenMode === 'static') {
    const result = getChildren(node.group)
    if (!isPromise(result)) known = sortTreeChildren(result)
  }
  const canExpand =
    node.kind === 'group' && getChildren !== undefined && (known === null || known.length > 0)

  async function toggle() {
    if (node.kind !== 'group' || !getChildren) return
    if (open) {
      setOpen(false)
      return
    }
    // Open the panel *before* the request: the loading note and the error
    // note both live inside the opened panel, so opening only after the
    // request settled would hide the spinner and swallow the error message
    // (a failed lazy load would look like the click did nothing).
    setOpen(true)
    if (children === null && known === null) {
      setLoading(true)
      setError('')
      try {
        setChildren(sortTreeChildren(await getChildren(node.group)))
      } catch (err) {
        setError(translateError(err))
      } finally {
        setLoading(false)
      }
    }
  }

  const isGroup = node.kind === 'group'
  const name = isGroup ? node.group.name : node.repo.name
  const description = isGroup ? node.group.description : node.repo.description

  return (
    <Fragment>
      <div className="group-tree-row">
        {canExpand ? (
          <button
            type="button"
            className="group-tree-toggle"
            aria-expanded={open}
            aria-label={open ? t('groups.collapse') : t('groups.expand')}
            onClick={() => void toggle()}
          >
            {open ? <ChevronDownIcon size={14} /> : <ChevronRightIcon size={14} />}
          </button>
        ) : (
          <span className="group-tree-toggle group-tree-toggle-empty" aria-hidden="true" />
        )}
        <span className="group-tree-icon">
          {isGroup ? <FolderIcon size={16} /> : <BookIcon size={16} />}
        </span>
        <button
          type="button"
          className="group-tree-link"
          onClick={() => {
            if (isGroup) onOpenGroup?.(node.group.full_path)
            else onOpenRepo?.(node.repo.owner, node.repo.name)
          }}
        >
          {name}
        </button>
        <span className="group-tree-desc">
          {description || t('common.noDescription')}
        </span>
        {!isGroup && (
          <span className="group-tree-meta">
            <span className="group-tree-meta-item">
              <GitBranchIcon size={14} />
              {node.repo.default_branch}
            </span>
            <span className="group-tree-meta-item">
              {node.repo.is_private ? t('projects.private') : t('projects.public')}
            </span>
          </span>
        )}
      </div>
      {open && isGroup && (
        <div className="group-tree-children">
          {error ? (
            <div className="group-tree-note group-tree-note-error">{error}</div>
          ) : loading ? (
            <div className="group-tree-note">{t('common.loading')}</div>
          ) : (() => {
            const kids = known ?? children
            if (kids == null || kids.length === 0) {
              return <div className="group-tree-note">{t('groups.noChildren')}</div>
            }
            return kids.map((child) => (
              <GroupTreeNode
                key={child.kind === 'group' ? `g-${child.group.id}` : `r-${child.repo.id}`}
                node={child}
                getChildren={getChildren}
                childrenMode={childrenMode}
                onOpenGroup={onOpenGroup}
                onOpenRepo={onOpenRepo}
              />
            ))
          })()}
        </div>
      )}
    </Fragment>
  )
}

import { useCallback, useEffect, useState } from 'react'
import { useSearchParams } from 'react-router-dom'
import {
  api,
  type Commit,
  type FileContent,
  type PullRequest,
  type Issue,
  type TreeEntry,
  type Pipeline,
  type Comment,
} from '../lib/api'
import { PageHeader, Alert, EmptyState, StatusLabel } from './Layout'
import {
  BookIcon,
  FolderIcon,
  FileIcon,
  FileCodeIcon,
  GitCommitIcon,
  GitPullRequestIcon,
  CircleDotIcon,
  ChevronRightIcon,
  PlusIcon,
  XIcon,
  CheckIcon,
  MessageSquareIcon,
} from './Icons'
import { useI18n, useErrorTranslator } from '../i18n'
import { EmptyRepoWelcome } from './EmptyRepoWelcome'
import { PipelinesPanel } from './PipelinesPanel'

type Tab = 'files' | 'commits' | 'prs' | 'issues' | 'pipelines'
const VALID_TABS: Tab[] = ['files', 'commits', 'prs', 'issues', 'pipelines']

export function RepoDetail({
  owner,
  name,
  onBack,
  repoKind,
  groupName,
}: {
  owner: string
  name: string
  onBack: () => void
  repoKind?: 'personal' | 'group'
  groupName?: string
}) {
  const { t } = useI18n()
  const translateError = useErrorTranslator()
  // Tab is owned by the URL: ?tab=files|commits|prs|issues. Default "files".
  // The Sidebar writes ?tab=<key> with replace=true; RepoDetail reads it back.
  const [searchParams] = useSearchParams()
  const tabFromUrl = searchParams.get('tab')
  const tab: Tab = (VALID_TABS as string[]).includes(tabFromUrl ?? '')
    ? (tabFromUrl as Tab)
    : 'files'
  // When the tab changes, ensure the file preview is cleared if we land on files.
  // (Mounted on tab transition rather than via a setter; URL is the source of truth.)
  void tab

  const [path, setPath] = useState('')
  const [ref, setRef] = useState('main')
  const [branches, setBranches] = useState<string[]>([])
  const [tree, setTree] = useState<TreeEntry[]>([])
  const [loaded, setLoaded] = useState(false)
  const [file, setFile] = useState<FileContent | null>(null)
  const [commits, setCommits] = useState<Commit[]>([])
  const [prs, setPrs] = useState<PullRequest[]>([])
  const [issues, setIssues] = useState<Issue[]>([])
  const [pipelines, setPipelines] = useState<Pipeline[]>([])
  const [error, setError] = useState('')

  const loadBranches = useCallback(async () => {
    try {
      const bs = await api.getBranches(owner, name)
      setBranches(bs)
      if (bs.length > 0 && !bs.includes(ref)) setRef(bs[0])
    } catch (err) {
      setError(translateError(err))
    }
  }, [owner, name, ref, translateError])

  const loadTree = useCallback(async () => {
    try {
      setFile(null)
      setLoaded(false)
      const res = await api.getTree(owner, name, path, ref)
      setTree(res.entries)
      setLoaded(true)
    } catch (err) {
      setError(translateError(err))
    }
  }, [owner, name, path, ref, translateError])

  const loadCommits = useCallback(async () => {
    try {
      const res = await api.getCommits(owner, name, ref, 50)
      setCommits(res.commits)
    } catch (err) {
      setError(translateError(err))
    }
  }, [owner, name, ref, translateError])

  const loadPrs = useCallback(async () => {
    try {
      const res = await api.listPrs(owner, name, 'all')
      setPrs(res.prs)
    } catch (err) {
      setError(translateError(err))
    }
  }, [owner, name, translateError])

  const loadIssues = useCallback(async () => {
    try {
      const res = await api.listIssues(owner, name, 'all')
      setIssues(res.issues)
    } catch (err) {
      setError(translateError(err))
    }
  }, [owner, name, translateError])

  const loadPipelines = useCallback(async () => {
    try {
      const res = await api.listPipelines(owner, name)
      setPipelines(res.pipelines)
    } catch (err) {
      setError(translateError(err))
    }
  }, [owner, name, translateError])

  useEffect(() => {
    void loadBranches()
  }, [loadBranches])

  useEffect(() => {
    switch (tab) {
      case 'files':
        void loadTree()
        break
      case 'commits':
        void loadCommits()
        break
      case 'prs':
        void loadPrs()
        break
      case 'issues':
        void loadIssues()
        break
      case 'pipelines':
        void loadPipelines()
        break
      default:
        break
    }
  }, [tab, loadTree, loadCommits, loadPrs, loadIssues, loadPipelines])

  async function openEntry(entry: TreeEntry) {
    if (entry.kind === 'tree') {
      setPath(entry.path)
    } else {
      try {
        const f = await api.getFile(owner, name, entry.path, ref)
        setFile(f)
      } catch (err) {
        setError(translateError(err))
      }
    }
  }

  return (
    <>
      <PageHeader
        icon={<BookIcon size={18} />}
        title={
          <>
            {owner} <span style={{ color: 'var(--fg-subtle)', fontWeight: 400 }}>/</span> {name}
          </>
        }
        subtitle={`${branches.length > 0 ? branches.join(', ') : ''}`}
        actions={
          <div className="page-actions">
            <button className="btn btn-ghost" onClick={onBack} style={{ display: 'inline-flex', alignItems: 'center', gap: 4 }}>
              <ChevronRightIcon size={14} style={{ transform: 'rotate(180deg)' }} />
              {t('projects.backToList')}
            </button>
            <select className="select" value={ref} onChange={(e) => setRef(e.target.value)} title={t('projects.defaultBranch')} style={{ width: 'auto' }}>
              {branches.map((b) => (
                <option key={b} value={b}>{b}</option>
              ))}
            </select>
          </div>
        }
      />

      {error && <Alert type="error">{error}</Alert>}

      {tab === 'files' && (
        <FilesPanel
          path={path}
          setPath={setPath}
          tree={tree}
          file={file}
          setFile={setFile}
          openEntry={openEntry}
          isEmptyRepo={loaded && commits.length === 0 && tree.length === 0}
          owner={owner}
          name={name}
          repoKind={repoKind}
          groupName={groupName}
        />
      )}
      {tab === 'commits' && <CommitsPanel commits={commits} />}
      {tab === 'prs' && <PrPanel owner={owner} name={name} prs={prs} onChanged={() => void loadPrs()} />}
      {tab === 'issues' && <IssuePanel owner={owner} name={name} issues={issues} onChanged={() => void loadIssues()} />}
      {tab === 'pipelines' && (
        <PipelinesPanel owner={owner} name={name} pipelines={pipelines} onChanged={() => void loadPipelines()} />
      )}
    </>
  )
}

// ---------------------------------------------------------------------------
// Files panel
// ---------------------------------------------------------------------------

function FilesPanel({
  path,
  setPath,
  tree,
  file,
  setFile,
  openEntry,
  isEmptyRepo,
  owner,
  name,
  repoKind,
  groupName,
}: {
  path: string
  setPath: (p: string) => void
  tree: TreeEntry[]
  file: FileContent | null
  setFile: (f: FileContent | null) => void
  openEntry: (entry: TreeEntry) => void
  isEmptyRepo: boolean
  owner: string
  name: string
  repoKind?: 'personal' | 'group'
  groupName?: string
}) {
  const { t } = useI18n()
  const parts = path.split('/').filter(Boolean)

  const crumbParts = ['', ...parts.map((_, idx) => path.split('/').slice(0, idx + 1).join('/'))]

  if (tree.length === 0 && isEmptyRepo) {
    // Fresh, never-committed repo: show the GitHub-style quick setup card.
    return (
      <EmptyRepoWelcome
        owner={owner}
        name={name}
        repoKind={repoKind ?? 'personal'}
        groupName={groupName}
      />
    )
  }

  return (
    <div className="file-browser">
      {file ? (
        <div className="file-view">
          <div className="file-view-header">
            <span className="file-path">
              <FileCodeIcon size={14} />
              {file.path}
            </span>
            <button className="btn btn-ghost btn-sm" onClick={() => setFile(null)}>
              ← {t('repo.backToDir')}
            </button>
          </div>
          <pre className="code-block">{file.content}</pre>
        </div>
      ) : (
        <div className="box">
          <div className="box-header">
            <div className="breadcrumb">
              <button className={`crumb crumb-root${parts.length === 0 ? ' crumb-current' : ''}`} onClick={() => setPath('')}>
                <FolderIcon size={14} />
                root
              </button>
              {parts.map((part, idx) => {
                const isLast = idx === parts.length - 1
                return (
                  <span key={idx} style={{ display: 'inline-flex', alignItems: 'center', gap: 2 }}>
                    <span className="sep">/</span>
                    <button
                      className={`crumb${isLast ? ' crumb-current' : ''}`}
                      onClick={() => !isLast && setPath(crumbParts[idx + 1])}
                    >
                      {part}
                    </button>
                  </span>
                )
              })}
            </div>
          </div>
          {tree.length === 0 ? (
            <EmptyState icon={<FolderIcon size={22} />} title={t('repo.emptyDirTitle')} description={t('repo.emptyDirDescription')} />
          ) : (
            <table className="tree-table">
              <tbody>
                {tree.map((entry) => (
                  <tr key={entry.path} onClick={() => void openEntry(entry)} className="clickable">
                    <td>
                      <div className="tree-entry">
                        {entry.kind === 'tree' ? (
                          <FolderIcon size={15} className="entry-icon folder" />
                        ) : (
                          <FileIcon size={15} className="entry-icon" />
                        )}
                        <span className="entry-name">{entry.name}</span>
                      </div>
                    </td>
                    <td style={{ textAlign: 'right', width: 120 }}>
                      {entry.kind === 'blob' && entry.size != null ? (
                        <span className="entry-size">{formatSize(entry.size)}</span>
                      ) : null}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>
      )}
    </div>
  )
}

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}

// ---------------------------------------------------------------------------
// Commits panel
// ---------------------------------------------------------------------------

function CommitsPanel({ commits }: { commits: Commit[] }) {
  const { t } = useI18n()
  return (
    <>
      <PageHeader
        icon={<GitCommitIcon size={18} />}
        title={t('repo.commitsTitle')}
        subtitle={t('repo.commitsSubtitle', { n: commits.length })}
      />
      {commits.length === 0 ? (
        <EmptyState
          icon={<GitCommitIcon size={22} />}
          title={t('repo.emptyCommitsTitle')}
          description={t('repo.emptyCommitsDescription')}
        />
      ) : (
        <div className="commit-list">
          {commits.map((c) => (
            <div key={c.sha} className="commit-item">
              <span className="commit-avatar">{c.author_name.slice(0, 1).toUpperCase()}</span>
              <div className="commit-body">
                <div className="commit-message">{c.message.split('\n')[0]}</div>
                <div className="commit-meta">
                  <span className="mono">{c.sha.slice(0, 10)}</span>
                  <span className="dot">•</span>
                  <span>{c.author_name}</span>
                  <span className="dot">•</span>
                  <span>{new Date(c.authored_at).toLocaleString()}</span>
                </div>
              </div>
            </div>
          ))}
        </div>
      )}
    </>
  )
}

// ---------------------------------------------------------------------------
// PR panel
// ---------------------------------------------------------------------------

function PrPanel({
  owner,
  name,
  prs,
  onChanged,
}: {
  owner: string
  name: string
  prs: PullRequest[]
  onChanged: () => void
}) {
  const { t } = useI18n()
  const translateError = useErrorTranslator()
  const [creating, setCreating] = useState(false)
  const [title, setTitle] = useState('')
  const [body, setBody] = useState('')
  const [source, setSource] = useState('')
  const [target, setTarget] = useState('main')
  const [selected, setSelected] = useState<PullRequest | null>(null)
  const [detail, setDetail] = useState<PullRequest | null>(null)
  const [error, setError] = useState('')

  async function createPr() {
    setError('')
    try {
      await api.createPr(owner, name, { title, body, source_branch: source, target_branch: target })
      setCreating(false)
      setTitle('')
      setBody('')
      setSource('')
      onChanged()
    } catch (err) {
      setError(translateError(err))
    }
  }

  async function openPr(pr: PullRequest) {
    setSelected(pr)
    setDetail(null)
    try {
      const res = await api.getPr(owner, name, pr.number)
      setDetail(res.pr)
    } catch (err) {
      setError(translateError(err))
    }
  }

  async function closePr() {
    if (!selected) return
    setError('')
    try {
      await api.closePr(owner, name, selected.number)
      setDetail(null)
      setSelected(null)
      onChanged()
    } catch (err) {
      setError(translateError(err))
    }
  }

  if (selected && detail) {
    return (
      <PrDetail
        pr={detail}
        owner={owner}
        name={name}
        onBack={() => { setSelected(null); setDetail(null) }}
        onMerged={async () => {
          setDetail(null)
          setSelected(null)
          onChanged()
        }}
        onClose={() => void closePr()}
      />
    )
  }

  return (
    <>
      <PageHeader
        icon={<GitPullRequestIcon size={18} />}
        title={t('repo.prsTitle')}
        subtitle={t('repo.prsSubtitle', {
          open: prs.filter((p) => p.status === 'open').length,
          merged: prs.filter((p) => p.status === 'merged').length,
        })}
        actions={
          <button className="btn btn-primary" onClick={() => setCreating((v) => !v)}>
            {creating ? <XIcon size={14} /> : <PlusIcon size={14} />}
            {creating ? t('common.cancel') : t('repo.createPr')}
          </button>
        }
      />

      {creating && (
        <div className="create-form">
          <div className="field field-full">
            <label className="field-label" htmlFor="pr-title">{t('repo.prTitle')}</label>
            <input id="pr-title" className="input" placeholder={t('repo.prTitlePlaceholder')} value={title} onChange={(e) => setTitle(e.target.value)} />
          </div>
          <div className="field field-full">
            <label className="field-label" htmlFor="pr-body">{t('repo.prBody')}</label>
            <textarea id="pr-body" className="textarea" placeholder={t('repo.prBodyPlaceholder')} value={body} onChange={(e) => setBody(e.target.value)} />
          </div>
          <div className="field">
            <label className="field-label" htmlFor="pr-source">{t('repo.sourceBranch')}</label>
            <input id="pr-source" className="input" placeholder={t('repo.sourceBranchPlaceholder')} value={source} onChange={(e) => setSource(e.target.value)} />
          </div>
          <div className="field">
            <label className="field-label" htmlFor="pr-target">{t('repo.targetBranch')}</label>
            <input id="pr-target" className="input" placeholder={t('repo.targetBranchPlaceholder')} value={target} onChange={(e) => setTarget(e.target.value)} />
          </div>
          <div className="field-actions">
            <button className="btn btn-ghost" onClick={() => setCreating(false)}>{t('common.cancel')}</button>
            <button className="btn btn-primary" onClick={() => void createPr()}>{t('repo.createPrSubmit')}</button>
          </div>
        </div>
      )}

      {error && <Alert type="error">{error}</Alert>}

      {prs.length === 0 ? (
        <EmptyState
          icon={<GitPullRequestIcon size={22} />}
          title={t('repo.emptyPrsTitle')}
          description={t('repo.emptyPrsDescription')}
        />
      ) : (
        <div className="row-list">
          {prs.map((pr) => (
            <div key={pr.id} className="row-item" onClick={() => void openPr(pr)}>
              <span className={`row-icon ${pr.status}`}>
                {pr.status === 'open' ? <GitPullRequestIcon size={16} /> : pr.status === 'merged' ? <GitPullRequestIcon size={16} /> : <CircleDotIcon size={16} />}
              </span>
              <div className="row-main">
                <div className="row-title">{pr.title}</div>
                <div className="row-meta">
                  <span className="row-number">#{pr.number}</span>
                  <span>{t('repo.prAuthor', { name: pr.author })}</span>
                  <span>{pr.source_branch} → {pr.target_branch}</span>
                </div>
              </div>
              <StatusLabel status={pr.status} />
            </div>
          ))}
        </div>
      )}
    </>
  )
}

function PrDetail({
  pr,
  owner,
  name,
  onBack,
  onMerged,
  onClose,
}: {
  pr: PullRequest
  owner: string
  name: string
  onBack: () => void
  onMerged: () => void
  onClose: () => void
}) {
  const { t } = useI18n()
  const translateError = useErrorTranslator()
  const [comments, setComments] = useState<Comment[]>([])
  const [commentText, setCommentText] = useState('')
  const [commentError, setCommentError] = useState('')
  const [diff, setDiff] = useState<string | null>(null)
  const [showDiff, setShowDiff] = useState(false)
  const [mergeError, setMergeError] = useState('')
  const [merging, setMerging] = useState(false)

  const loadComments = useCallback(async () => {
    try {
      const res = await api.listPrComments(owner, name, pr.number)
      setComments(res.comments)
    } catch (err) {
      setCommentError(translateError(err))
    }
  }, [owner, name, pr.number, translateError])

  useEffect(() => {
    void loadComments()
  }, [loadComments])

  async function toggleDiff() {
    if (diff !== null || showDiff) {
      setShowDiff(!showDiff)
      return
    }
    setMergeError('')
    try {
      const res = await api.getPrDiff(owner, name, pr.number)
      setDiff(res.diff)
      setShowDiff(true)
    } catch (err) {
      setMergeError(translateError(err))
    }
  }

  async function merge() {
    setMerging(true)
    setMergeError('')
    try {
      await api.mergePr(owner, name, pr.number)
      onMerged()
    } catch (err) {
      setMergeError(translateError(err))
    } finally {
      setMerging(false)
    }
  }

  return (
    <>
      <div className="pr-detail-header">
        <div className="pr-detail-title">
          <span className="pr-number">#{pr.number}</span>
          {pr.title}
          <StatusLabel status={pr.status} />
        </div>
        <div className="page-actions">
          <button className="btn btn-ghost" onClick={onBack}>{t('repo.backToList')}</button>
          {pr.status === 'open' && (
            <>
              <button className="btn btn-ghost" onClick={() => void toggleDiff()}>
                {showDiff ? t('repo.hideDiff') : t('repo.viewDiff')}
              </button>
              <button className="btn btn-primary" disabled={merging} onClick={() => void merge()}>
                {t('repo.merge')}
              </button>
              <button className="btn btn-danger" onClick={onClose}>{t('repo.close')}</button>
            </>
          )}
        </div>
      </div>

      {mergeError && <Alert type="error">{mergeError}</Alert>}

      {pr.conflicts && pr.conflicts.length > 0 && pr.status === 'open' && (
        <div className="alert alert-warning" style={{ marginBottom: 12 }}>
          <strong>{t('repo.mergeConflictTitle')}:</strong>{' '}
          {t('repo.mergeConflictDesc')} {pr.conflicts.join(', ')}
        </div>
      )}
      {pr.status === 'open' && pr.conflicts.length === 0 && (
        <div className="alert alert-success" style={{ marginBottom: 12 }}>
          {t('repo.noConflicts')}
        </div>
      )}

      <div className="pr-meta-grid">
        <div className="pr-meta-item">
          <span className="label">{t('repo.author')}</span>
          <span className="value">{pr.author}</span>
        </div>
        <div className="pr-meta-item">
          <span className="label">{t('repo.sourceBranch')}</span>
          <span className="value">{pr.source_branch}</span>
        </div>
        <div className="pr-meta-item">
          <span className="label">{t('repo.targetBranch')}</span>
          <span className="value">{pr.target_branch}</span>
        </div>
        <div className="pr-meta-item">
          <span className="label">{t('repo.createdTime')}</span>
          <span className="value">{new Date(pr.created_at).toLocaleString()}</span>
        </div>
      </div>

      <p className="pr-body">{pr.body || t('repo.noBody')}</p>

      {showDiff && diff !== null && (
        <div className="pr-diff-box">
          <h4>{t('repo.viewDiff')}</h4>
          <pre className="code-block">{diff}</pre>
        </div>
      )}

      <CommentSection
        comments={comments}
        text={commentText}
        setText={setCommentText}
        error={commentError}
        submitLabel={t('repo.commentSubmit')}
        emptyText={t('repo.commentEmpty')}
        onSubmit={async () => {
          setCommentError('')
          try {
            await api.createPrComment(owner, name, pr.number, commentText)
            setCommentText('')
            await loadComments()
          } catch (err) {
            setCommentError(translateError(err))
          }
        }}
        onDelete={async (id) => {
          setCommentError('')
          try {
            await api.deleteComment(id)
            await loadComments()
          } catch (err) {
            setCommentError(translateError(err))
          }
        }}
      />
    </>
  )
}

// ---------------------------------------------------------------------------
// Issues panel
// ---------------------------------------------------------------------------

function IssuePanel({
  owner,
  name,
  issues,
  onChanged,
}: {
  owner: string
  name: string
  issues: Issue[]
  onChanged: () => void
}) {
  const { t } = useI18n()
  const translateError = useErrorTranslator()
  const [creating, setCreating] = useState(false)
  const [title, setTitle] = useState('')
  const [body, setBody] = useState('')
  const [error, setError] = useState('')
  const [selected, setSelected] = useState<Issue | null>(null)
  const [comments, setComments] = useState<Comment[]>([])
  const [commentText, setCommentText] = useState('')
  const [commentError, setCommentError] = useState('')

  async function createIssue() {
    setError('')
    try {
      await api.createIssue(owner, name, title, body)
      setCreating(false)
      setTitle('')
      setBody('')
      onChanged()
    } catch (err) {
      setError(translateError(err))
    }
  }

  async function closeIssue(number: number) {
    setError('')
    try {
      await api.closeIssue(owner, name, number)
      onChanged()
    } catch (err) {
      setError(translateError(err))
    }
  }

  const loadComments = useCallback(async (issueNumber: number) => {
    try {
      const res = await api.listIssueComments(owner, name, issueNumber)
      setComments(res.comments)
    } catch (err) {
      setCommentError(translateError(err))
    }
  }, [owner, name, translateError])

  async function openIssue(issue: Issue) {
    setSelected(issue)
    setCommentError('')
    await loadComments(issue.number)
  }

  if (selected) {
    return (
      <IssueDetail
        issue={selected}
        comments={comments}
        commentText={commentText}
        setCommentText={setCommentText}
        commentError={commentError}
        onBack={() => setSelected(null)}
        onClose={async () => {
          setError('')
          try {
            await closeIssue(selected.number)
            const updated = { ...selected, status: 'closed' as const }
            setSelected(updated)
            onChanged()
          } catch (err) {
            setError(translateError(err))
          }
        }}
        onSubmitComment={async () => {
          setCommentError('')
          try {
            await api.createIssueComment(owner, name, selected.number, commentText)
            setCommentText('')
            await loadComments(selected.number)
          } catch (err) {
            setCommentError(translateError(err))
          }
        }}
        onDeleteComment={async (id) => {
          setCommentError('')
          try {
            await api.deleteComment(id)
            await loadComments(selected.number)
          } catch (err) {
            setCommentError(translateError(err))
          }
        }}
      />
    )
  }

  return (
    <>
      <PageHeader
        icon={<CircleDotIcon size={18} />}
        title={t('repo.issuesTitle')}
        subtitle={t('repo.issuesSubtitle', {
          open: issues.filter((i) => i.status === 'open').length,
          closed: issues.filter((i) => i.status === 'closed').length,
        })}
        actions={
          <button className="btn btn-primary" onClick={() => setCreating((v) => !v)}>
            {creating ? <XIcon size={14} /> : <PlusIcon size={14} />}
            {creating ? t('common.cancel') : t('repo.createIssue')}
          </button>
        }
      />

      {creating && (
        <div className="create-form">
          <div className="field field-full">
            <label className="field-label" htmlFor="issue-title">{t('repo.issueTitle')}</label>
            <input id="issue-title" className="input" placeholder={t('repo.issueTitlePlaceholder')} value={title} onChange={(e) => setTitle(e.target.value)} />
          </div>
          <div className="field field-full">
            <label className="field-label" htmlFor="issue-body">{t('repo.issueBody')}</label>
            <textarea id="issue-body" className="textarea" placeholder={t('repo.issueBodyPlaceholder')} value={body} onChange={(e) => setBody(e.target.value)} />
          </div>
          <div className="field-actions">
            <button className="btn btn-ghost" onClick={() => setCreating(false)}>{t('common.cancel')}</button>
            <button className="btn btn-primary" onClick={() => void createIssue()}>{t('repo.createIssueSubmit')}</button>
          </div>
        </div>
      )}

      {error && <Alert type="error">{error}</Alert>}

      {issues.length === 0 ? (
        <EmptyState
          icon={<CircleDotIcon size={22} />}
          title={t('repo.emptyIssuesTitle')}
          description={t('repo.emptyIssuesDescription')}
        />
      ) : (
        <div className="row-list">
          {issues.map((issue) => (
            <div key={issue.id} className="row-item" onClick={() => void openIssue(issue)}>
              <span className={`row-icon ${issue.status === 'open' ? 'open' : 'closed'}`}>
                {issue.status === 'open' ? <CircleDotIcon size={16} /> : <CheckIcon size={16} />}
              </span>
              <div className="row-main">
                <div className="row-title">{issue.title}</div>
                <div className="row-meta">
                  <span className="row-number">#{issue.number}</span>
                  <span>{t('repo.prAuthor', { name: issue.author })}</span>
                </div>
              </div>
              <StatusLabel status={issue.status} />
              {issue.status === 'open' && (
                <div className="row-actions">
                  <button className="btn btn-ghost btn-sm" onClick={(e) => { e.stopPropagation(); void closeIssue(issue.number) }}>
                    {t('repo.close')}
                  </button>
                </div>
              )}
            </div>
          ))}
        </div>
      )}
    </>
  )
}

// ---------------------------------------------------------------------------
// Issue detail (with comments)
// ---------------------------------------------------------------------------

function IssueDetail({
  issue,
  comments,
  commentText,
  setCommentText,
  commentError,
  onBack,
  onClose,
  onSubmitComment,
  onDeleteComment,
}: {
  issue: Issue
  comments: Comment[]
  commentText: string
  setCommentText: (s: string) => void
  commentError: string
  onBack: () => void
  onClose: () => void
  onSubmitComment: () => void
  onDeleteComment: (id: number) => void
}) {
  const { t } = useI18n()
  return (
    <>
      <div className="pr-detail-header">
        <div className="pr-detail-title">
          <span className="pr-number">#{issue.number}</span>
          {issue.title}
          <StatusLabel status={issue.status} />
        </div>
        <div className="page-actions">
          <button className="btn btn-ghost" onClick={onBack}>{t('repo.backToList')}</button>
          {issue.status === 'open' && (
            <button className="btn btn-danger" onClick={onClose}>{t('repo.close')}</button>
          )}
        </div>
      </div>

      <div className="pr-meta-grid">
        <div className="pr-meta-item">
          <span className="label">{t('repo.author')}</span>
          <span className="value">{issue.author}</span>
        </div>
        <div className="pr-meta-item">
          <span className="label">{t('repo.createdTime')}</span>
          <span className="value">{new Date(issue.created_at).toLocaleString()}</span>
        </div>
      </div>

      <p className="pr-body">{issue.body || t('repo.noBody')}</p>

      <CommentSection
        comments={comments}
        text={commentText}
        setText={setCommentText}
        error={commentError}
        submitLabel={t('repo.commentSubmit')}
        emptyText={t('repo.commentEmpty')}
        onSubmit={onSubmitComment}
        onDelete={onDeleteComment}
      />
    </>
  )
}

// ---------------------------------------------------------------------------
// Comments
// ---------------------------------------------------------------------------

function CommentSection({
  comments,
  text,
  setText,
  error,
  submitLabel,
  emptyText,
  onSubmit,
  onDelete,
}: {
  comments: Comment[]
  text: string
  setText: (s: string) => void
  error: string
  submitLabel: string
  emptyText: string
  onSubmit: () => void
  onDelete: (id: number) => void
}) {
  const { t } = useI18n()
  const [busy, setBusy] = useState(false)

  return (
    <div className="comments-section">
      <h3 className="comments-title">
        <MessageSquareIcon size={14} />
        {t('repo.commentsTitle')}
      </h3>

      {error && <Alert type="error">{error}</Alert>}

      {comments.length === 0 ? (
        <p className="comments-empty">{emptyText}</p>
      ) : (
        <div className="comment-list">
          {comments.map((c) => (
            <div key={c.id} className="comment-item">
              <div className="comment-meta">
                <span className="comment-author">{c.author}</span>
                <span className="comment-time">{new Date(c.created_at).toLocaleString()}</span>
                <button
                  className="btn btn-ghost btn-xs"
                  onClick={() => void onDelete(c.id)}
                  title={t('repo.deleteComment')}
                >
                  {t('repo.deleteComment')}
                </button>
              </div>
              <div className="comment-body">{c.body}</div>
            </div>
          ))}
        </div>
      )}

      <div className="comment-form">
        <textarea
          className="textarea"
          value={text}
          onChange={(e) => setText(e.target.value)}
          placeholder={t('repo.commentPlaceholder')}
        />
        <div className="comment-form-actions">
          <button
            className="btn btn-primary"
            disabled={busy || text.trim() === ''}
            onClick={() => {
              setBusy(true)
              onSubmit()
              setBusy(false)
            }}
          >
            {submitLabel}
          </button>
        </div>
      </div>
    </div>
  )
}
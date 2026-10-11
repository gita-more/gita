import { useCallback, useEffect, useState } from 'react'
import {
  api,
  type Commit,
  type FileContent,
  type PullRequest,
  type Review,
  type ReviewComment,
  type ReviewDiscussionComment,
  type Issue,
  type TreeEntry,
} from '../lib/api'

type Tab = 'files' | 'commits' | 'prs' | 'issues' | 'reviews' | 'search'

export function RepoDetail({
  owner,
  name,
  onBack,
}: {
  owner: string
  name: string
  onBack: () => void
}) {
  const [tab, setTab] = useState<Tab>('files')
  const [path, setPath] = useState('')
  const [ref, setRef] = useState('main')
  const [branches, setBranches] = useState<string[]>([])
  const [tree, setTree] = useState<TreeEntry[]>([])
  const [file, setFile] = useState<FileContent | null>(null)
  const [commits, setCommits] = useState<Commit[]>([])
  const [prs, setPrs] = useState<PullRequest[]>([])
  const [issues, setIssues] = useState<Issue[]>([])
  const [reviews, setReviews] = useState<Review[]>([])
  const [error, setError] = useState('')

  const loadBranches = useCallback(async () => {
    try {
      const bs = await api.getBranches(owner, name)
      setBranches(bs)
      if (bs.length > 0 && !bs.includes(ref)) setRef(bs[0])
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }, [owner, name, ref])

  const loadTree = useCallback(async () => {
    try {
      setFile(null)
      const res = await api.getTree(owner, name, path, ref)
      setTree(res.entries)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }, [owner, name, path, ref])

  const loadCommits = useCallback(async () => {
    try {
      const res = await api.getCommits(owner, name, ref, 50)
      setCommits(res.commits)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }, [owner, name, ref])

  const loadPrs = useCallback(async () => {
    try {
      const res = await api.listPrs(owner, name, 'all')
      setPrs(res.prs)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }, [owner, name])

  const loadIssues = useCallback(async () => {
    try {
      const res = await api.listIssues(owner, name, 'all')
      setIssues(res.issues)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }, [owner, name])

  const loadReviews = useCallback(async () => {
    try {
      const res = await api.listReviews()
      const filtered = res.reviews.filter((r) => r.repo_id >= 0)
      // Reviews don't expose repo_id in the listing; fall back to all reviews.
      setReviews(filtered)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }, [])

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
      case 'reviews':
        void loadReviews()
        break
      default:
        break
    }
  }, [tab, loadTree, loadCommits, loadPrs, loadIssues, loadReviews])

  async function openEntry(entry: TreeEntry) {
    if (entry.kind === 'tree') {
      setPath(entry.path)
    } else {
      try {
        const f = await api.getFile(owner, name, entry.path, ref)
        setFile(f)
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err))
      }
    }
  }

  return (
    <div className="page">
      <header className="topbar">
        <div className="topbar-left">
          <button className="btn-ghost" onClick={onBack}>← 返回</button>
          <span className="logo">{owner}/{name}</span>
        </div>
        <div className="topbar-right">
          <select value={ref} onChange={(e) => setRef(e.target.value)}>
            {branches.map((b) => (
              <option key={b} value={b}>{b}</option>
            ))}
          </select>
        </div>
      </header>

      <div className="repo-tabs">
        {(['files', 'commits', 'prs', 'issues', 'reviews', 'search'] as Tab[]).map((t) => (
          <button
            key={t}
            className={tab === t ? 'active' : ''}
            onClick={() => setTab(t)}
          >
            {t === 'files' ? '文件' : t === 'commits' ? '提交' : t === 'prs' ? 'PR' : t === 'issues' ? 'Issue' : t === 'reviews' ? 'AI 评审' : '搜索'}
          </button>
        ))}
      </div>

      {error && <div className="error-banner">{error}</div>}

      {tab === 'files' && (
        <div className="file-browser">
          <div className="breadcrumb">
            <button onClick={() => setPath('')}>root</button>
            {path.split('/').filter(Boolean).map((part, idx) => {
              const upTo = path.split('/').slice(0, idx + 1).join('/')
              return (
                <span key={idx}>
                  <span className="sep">/</span>
                  <button onClick={() => setPath(upTo)}>{part}</button>
                </span>
              )
            })}
          </div>
          {file ? (
            <div className="file-view">
              <div className="file-header">
                <span>{file.path}</span>
                <button className="btn-ghost" onClick={() => setFile(null)}>← 返回目录</button>
              </div>
              <pre className="code-block">{file.content}</pre>
            </div>
          ) : (
            <table className="tree-table">
              <tbody>
                {tree.map((entry) => (
                  <tr key={entry.path} onClick={() => void openEntry(entry)} className="clickable">
                    <td>{entry.kind === 'tree' ? '📁' : '📄'}</td>
                    <td>{entry.name}</td>
                    <td>{entry.kind === 'blob' && entry.size != null ? `${entry.size} B` : ''}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>
      )}

      {tab === 'commits' && (
        <div className="commit-list">
          {commits.map((c) => (
            <div key={c.sha} className="commit-item">
              <div className="commit-message">{c.message.split('\n')[0]}</div>
              <div className="commit-meta">
                <span>{c.author_name}</span>
                <span>{new Date(c.authored_at).toLocaleString()}</span>
                <code>{c.sha.slice(0, 10)}</code>
              </div>
            </div>
          ))}
        </div>
      )}

      {tab === 'prs' && <PrPanel owner={owner} name={name} prs={prs} onChanged={() => void loadPrs()} />}
      {tab === 'issues' && <IssuePanel owner={owner} name={name} issues={issues} onChanged={() => void loadIssues()} />}
      {tab === 'reviews' && <ReviewsPanel reviews={reviews} owner={owner} />}
      {tab === 'search' && <SearchPanel owner={owner} name={name} />}
    </div>
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
  const [creating, setCreating] = useState(false)
  const [title, setTitle] = useState('')
  const [body, setBody] = useState('')
  const [source, setSource] = useState('')
  const [target, setTarget] = useState('main')
  const [selected, setSelected] = useState<PullRequest | null>(null)
  const [detail, setDetail] = useState<{ pr: PullRequest; reviews: Review[] } | null>(null)
  const [error, setError] = useState('')

  async function createPr() {
    try {
      await api.createPr(owner, name, { title, body, source_branch: source, target_branch: target })
      setCreating(false)
      setTitle('')
      setBody('')
      setSource('')
      onChanged()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }

  async function openPr(pr: PullRequest) {
    setSelected(pr)
    try {
      const res = await api.getPr(owner, name, pr.number)
      setDetail(res)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }

  async function mergePr() {
    if (!selected) return
    try {
      await api.mergePr(owner, name, selected.number)
      setDetail(null)
      setSelected(null)
      onChanged()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }

  async function closePr() {
    if (!selected) return
    try {
      await api.closePr(owner, name, selected.number)
      setDetail(null)
      setSelected(null)
      onChanged()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }

  async function runReview() {
    if (!selected) return
    try {
      await api.triggerReview(owner, name, selected.number)
      const res = await api.getPr(owner, name, selected.number)
      setDetail(res)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }

  return (
    <div className="pr-panel">
      <div className="panel-header">
        <h3>Pull Requests</h3>
        <button className="btn-primary" onClick={() => setCreating(!creating)}>
          {creating ? '取消' : '+ 新建 PR'}
        </button>
      </div>

      {creating && (
        <div className="create-pr-form">
          <input placeholder="标题" value={title} onChange={(e) => setTitle(e.target.value)} />
          <textarea placeholder="描述" value={body} onChange={(e) => setBody(e.target.value)} />
          <input placeholder="源分支" value={source} onChange={(e) => setSource(e.target.value)} />
          <input placeholder="目标分支" value={target} onChange={(e) => setTarget(e.target.value)} />
          <button className="btn-primary" onClick={() => void createPr()}>创建</button>
        </div>
      )}

      {error && <div className="error-banner">{error}</div>}

      {selected && detail ? (
        <div className="pr-detail">
          <div className="pr-detail-header">
            <h3>#{selected.number} {selected.title}</h3>
            <div>
              <button className="btn-ghost" onClick={() => void runReview()}>触发 AI 评审</button>
              {selected.status === 'open' && (
                <>
                  <button className="btn-primary" onClick={() => void mergePr()}>合并</button>
                  <button className="btn-danger" onClick={() => void closePr()}>关闭</button>
                </>
              )}
              <button className="btn-ghost" onClick={() => { setSelected(null); setDetail(null) }}>返回</button>
            </div>
          </div>
          <p className="pr-body">{selected.body}</p>
          <div className="pr-meta">
            <span>源: {selected.source_branch}</span>
            <span>目标: {selected.target_branch}</span>
            <span>作者: {selected.author}</span>
            <span>状态: {selected.status}</span>
          </div>
          {detail.reviews.length > 0 ? (
            <div className="review-section">
              <h4>AI 评审</h4>
              {detail.reviews.map((r) => (
                <div key={r.id} className="review-card">
                  <div className="review-header">
                    <span>风险分: <b>{r.risk_score}</b></span>
                    <span>{new Date(r.created_at).toLocaleString()}</span>
                  </div>
                  <p>{r.summary}</p>
                  {r.comments.map((c) => (
                    <ReviewCommentRow key={c.id} comment={c} />
                  ))}
                </div>
              ))}
            </div>
          ) : (
            <div className="empty">还没有 AI 评审，点击「触发 AI 评审」。</div>
          )}
        </div>
      ) : (
        <table className="table">
          <thead>
            <tr>
              <th>#</th>
              <th>标题</th>
              <th>作者</th>
              <th>源 → 目标</th>
              <th>状态</th>
            </tr>
          </thead>
          <tbody>
            {prs.map((pr) => (
              <tr key={pr.id} className="clickable" onClick={() => void openPr(pr)}>
                <td>{pr.number}</td>
                <td>{pr.title}</td>
                <td>{pr.author}</td>
                <td>{pr.source_branch} → {pr.target_branch}</td>
                <td>{pr.status}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  )
}

function ReviewCommentRow({
  comment,
  isOwner = false,
  onResolvedChange,
}: {
  comment: ReviewComment
  isOwner?: boolean
  onResolvedChange?: () => void
}) {
  return (
    <div className={`review-comment severity-${comment.severity}${comment.resolved ? ' resolved' : ''}`}>
      <div className="review-comment-header">
        <span className="severity-badge">{comment.severity}</span>
        <code>{comment.file_path}{comment.line ? `:${comment.line}` : ''}</code>
        {comment.resolved && <span className="resolved-badge">✓ 已解决</span>}
        {isOwner && onResolvedChange && (
          <button
            className="btn-ghost btn-small"
            onClick={onResolvedChange}
          >
            {comment.resolved ? '取消解决' : '标记解决'}
          </button>
        )}
      </div>
      <p>{comment.message}</p>
      {comment.suggestion && (
        <pre className="suggestion">{comment.suggestion}</pre>
      )}
    </div>
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
  const [creating, setCreating] = useState(false)
  const [title, setTitle] = useState('')
  const [body, setBody] = useState('')
  const [error, setError] = useState('')

  async function createIssue() {
    try {
      await api.createIssue(owner, name, title, body)
      setCreating(false)
      setTitle('')
      setBody('')
      onChanged()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }

  async function closeIssue(number: number) {
    try {
      await api.closeIssue(owner, name, number)
      onChanged()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }

  return (
    <div className="issue-panel">
      <div className="panel-header">
        <h3>Issues</h3>
        <button className="btn-primary" onClick={() => setCreating(!creating)}>
          {creating ? '取消' : '+ 新建 Issue'}
        </button>
      </div>

      {creating && (
        <div className="create-issue-form">
          <input placeholder="标题" value={title} onChange={(e) => setTitle(e.target.value)} />
          <textarea placeholder="描述" value={body} onChange={(e) => setBody(e.target.value)} />
          <button className="btn-primary" onClick={() => void createIssue()}>创建</button>
        </div>
      )}

      {error && <div className="error-banner">{error}</div>}

      <table className="table">
        <thead>
          <tr>
            <th>#</th>
            <th>标题</th>
            <th>作者</th>
            <th>状态</th>
            <th></th>
          </tr>
        </thead>
        <tbody>
          {issues.map((issue) => (
            <tr key={issue.id}>
              <td>{issue.number}</td>
              <td>{issue.title}</td>
              <td>{issue.author}</td>
              <td>{issue.status}</td>
              <td>
                {issue.status === 'open' && (
                  <button className="btn-ghost" onClick={() => void closeIssue(issue.number)}>关闭</button>
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

// ---------------------------------------------------------------------------
// Reviews panel
// ---------------------------------------------------------------------------

function ReviewsPanel({
  reviews,
  owner,
}: {
  reviews: Review[]
  owner: string
}) {
  const [currentUser, setCurrentUser] = useState<{ id: number; username: string } | null>(null)

  useEffect(() => {
    api.me().then(setCurrentUser).catch(() => setCurrentUser(null))
  }, [])

  const isOwner = currentUser?.username === owner

  return (
    <div className="reviews-panel">
      <div className="panel-header">
        <h3>AI 评审</h3>
      </div>
      {reviews.length === 0 ? (
        <div className="empty">暂无 AI 评审。在 PR 详情页点击「触发 AI 评审」。</div>
      ) : (
        reviews.map((r) => (
          <ReviewCard
            key={r.id}
            review={r}
            isOwner={isOwner}
            currentUserId={currentUser?.id ?? null}
          />
        ))
      )}
    </div>
  )
}

function ReviewCard({
  review,
  isOwner,
  currentUserId,
}: {
  review: Review
  isOwner: boolean
  currentUserId: number | null
}) {
  const [comments, setComments] = useState<ReviewDiscussionComment[]>([])
  const [newComment, setNewComment] = useState('')
  const [editingCommentId, setEditingCommentId] = useState<number | null>(null)
  const [editBody, setEditBody] = useState('')
  const [editingReview, setEditingReview] = useState(false)
  const [summary, setSummary] = useState(review.summary)
  const [riskScore, setRiskScore] = useState(review.risk_score)
  const [error, setError] = useState('')

  const loadComments = useCallback(async () => {
    try {
      const res = await api.listReviewComments(review.id)
      setComments(res.comments)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }, [review.id])

  useEffect(() => {
    void loadComments()
  }, [loadComments])

  async function addComment() {
    setError('')
    const body = newComment.trim()
    if (!body) return
    try {
      await api.addReviewComment(review.id, body)
      setNewComment('')
      await loadComments()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }

  async function saveEdit(commentId: number) {
    setError('')
    try {
      await api.editReviewComment(commentId, editBody)
      setEditingCommentId(null)
      setEditBody('')
      await loadComments()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }

  async function deleteComment(commentId: number) {
    setError('')
    try {
      await api.deleteReviewComment(commentId)
      await loadComments()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }

  async function saveReview() {
    setError('')
    try {
      const updated = await api.updateReview(review.id, summary, riskScore)
      review.summary = updated.summary
      review.risk_score = updated.risk_score
      setEditingReview(false)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }

  async function toggleResolved(commentId: number, resolved: boolean) {
    setError('')
    try {
      if (resolved) {
        await api.unresolveReviewComment(commentId)
      } else {
        await api.resolveReviewComment(commentId)
      }
      // Reload the review so the resolved state refreshes.
      const updated = await api.getReview(review.id)
      review.comments = updated.comments
      review.risk_score = updated.risk_score
      review.summary = updated.summary
      // Force a re-render by refreshing local state.
      setSummary(updated.summary)
      setRiskScore(updated.risk_score)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }

  return (
    <div className="review-card">
      <div className="review-header">
        <span>风险分: <b>{review.risk_score}</b></span>
        <code>{review.commit_sha.slice(0, 10)}</code>
        <span>{new Date(review.created_at).toLocaleString()}</span>
        {isOwner && (
          <button className="btn-ghost btn-small" onClick={() => setEditingReview(!editingReview)}>
            {editingReview ? '取消' : '编辑'}
          </button>
        )}
      </div>

      {editingReview && isOwner ? (
        <div className="review-edit-form">
          <textarea
            placeholder="评审摘要"
            value={summary}
            onChange={(e) => setSummary(e.target.value)}
          />
          <input
            type="number"
            min={0}
            max={100}
            placeholder="风险分 (0-100)"
            value={riskScore}
            onChange={(e) => setRiskScore(Number(e.target.value))}
          />
          <button className="btn-primary" onClick={() => void saveReview()}>保存</button>
        </div>
      ) : (
        <p>{review.summary}</p>
      )}

      {review.comments.map((c) => (
        <ReviewCommentRow
          key={c.id}
          comment={c}
          isOwner={isOwner}
          onResolvedChange={
            isOwner ? () => void toggleResolved(c.id, c.resolved) : undefined
          }
        />
      ))}

      {error && <div className="error-banner">{error}</div>}

      {/* User discussion */}
      {comments.length > 0 && (
        <div className="review-discussion">
          <h4>讨论</h4>
          {comments.map((c) => (
            <div key={c.id} className="discussion-comment">
              <div className="discussion-comment-header">
                <b>{c.author_name}</b>
                <span>{new Date(c.created_at).toLocaleString()}</span>
                {currentUserId === c.author_id && (
                  <span className="discussion-actions">
                    <button
                      className="btn-ghost btn-small"
                      onClick={() => {
                        setEditingCommentId(c.id)
                        setEditBody(c.body)
                      }}
                    >
                      编辑
                    </button>
                    <button
                      className="btn-danger btn-small"
                      onClick={() => void deleteComment(c.id)}
                    >
                      删除
                    </button>
                  </span>
                )}
              </div>
              {editingCommentId === c.id ? (
                <div className="discussion-edit">
                  <textarea value={editBody} onChange={(e) => setEditBody(e.target.value)} />
                  <button className="btn-primary btn-small" onClick={() => void saveEdit(c.id)}>保存</button>
                  <button className="btn-ghost btn-small" onClick={() => setEditingCommentId(null)}>取消</button>
                </div>
              ) : (
                <p>{c.body}</p>
              )}
            </div>
          ))}
        </div>
      )}

      <div className="discussion-input">
        <input
          placeholder="发表评论…"
          value={newComment}
          onChange={(e) => setNewComment(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && void addComment()}
        />
        <button className="btn-primary btn-small" onClick={() => void addComment()}>评论</button>
      </div>
    </div>
  )
}

// ---------------------------------------------------------------------------
// Search panel
// ---------------------------------------------------------------------------

function SearchPanel({ owner, name }: { owner: string; name: string }) {
  const [q, setQ] = useState('')
  const [hits, setHits] = useState<{ repo_name: string; file_path: string; content: string; score: number }[]>([])
  const [error, setError] = useState('')

  async function doSearch() {
    setError('')
    try {
      const res = await api.search(q, `${owner}/${name}`)
      setHits(res.hits)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }

  return (
    <div className="search-panel">
      <h3>代码搜索</h3>
      <div className="search-bar">
        <input
          placeholder="输入关键词或自然语言查询…"
          value={q}
          onChange={(e) => setQ(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && void doSearch()}
        />
        <button className="btn-primary" onClick={() => void doSearch()}>搜索</button>
      </div>
      {error && <div className="error-banner">{error}</div>}
      {hits.length === 0 ? (
        <div className="empty">输入关键词搜索代码。</div>
      ) : (
        hits.map((hit, idx) => (
          <div key={idx} className="search-hit">
            <div className="search-hit-header">
              <code>{hit.repo_name}/{hit.file_path}</code>
              <span>score: {hit.score.toFixed(2)}</span>
            </div>
            <pre className="search-hit-content">{hit.content}</pre>
          </div>
        ))
      )}
    </div>
  )
}
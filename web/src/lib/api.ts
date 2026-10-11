// Gita API client types and helpers.

export interface User {
  id: number
  username: string
  email: string
}

export interface Repository {
  id: number
  owner: string
  name: string
  full_name: string
  description: string
  is_private: boolean
  default_branch: string
  created_at: string
}

export interface TreeEntry {
  path: string
  name: string
  kind: string
  size: number | null
}

export interface Commit {
  id: number
  repo_id: number
  sha: string
  author_name: string
  author_email: string
  message: string
  parent_shas: string[]
  authored_at: string
}

export interface FileContent {
  path: string
  content: string
  sha: string
}

export interface ReviewComment {
  id: number
  review_id: number
  file_path: string
  line: number | null
  severity: 'info' | 'warning' | 'error'
  message: string
  suggestion: string
  created_at: string
  resolved: boolean
  resolved_by: number | null
  resolved_at: string | null
}

export interface Review {
  id: number
  pr_id: number | null
  repo_id: number
  commit_sha: string
  status: string
  risk_score: number
  summary: string
  created_at: string
  updated_at: string
  comments: ReviewComment[]
}

export interface ReviewDiscussionComment {
  id: number
  review_id: number
  author_id: number
  author_name: string
  body: string
  created_at: string
  updated_at: string
  deleted_at: string | null
}

export interface PullRequest {
  id: number
  number: number
  title: string
  body: string
  source_branch: string
  target_branch: string
  status: 'open' | 'merged' | 'closed'
  author: string
  base_sha: string
  head_sha: string
  created_at: string
  updated_at: string
}

export interface Issue {
  id: number
  number: number
  title: string
  body: string
  status: 'open' | 'closed'
  author: string
  created_at: string
}

export interface SearchHit {
  repo_id: number
  repo_name: string
  file_path: string
  content: string
  score: number
}

export interface EventItem {
  id: number
  event_type: string
  repo_id: number | null
  actor_id: number | null
  payload: Record<string, unknown>
  created_at: string
}

let token: string | null = null
export function setToken(t: string | null) {
  token = t
}
export function getToken() {
  return token
}

async function request<T>(path: string, options: RequestInit = {}): Promise<T> {
  const headers: Record<string, string> = {
    'Content-Type': 'application/json',
  }
  if (token) headers['Authorization'] = `Bearer ${token}`
  const res = await fetch(path, { ...options, headers })
  if (!res.ok) {
    let message = `HTTP ${res.status}`
    try {
      const body = await res.json()
      if (body?.error?.message) message = body.error.message
    } catch {
      // ignore parse errors
    }
    throw new Error(message)
  }
  return res.json() as Promise<T>
}

export const api = {
  // auth
  register: (username: string, email: string, password: string) =>
    request<{ token: string; user: User }>('/api/auth/register', {
      method: 'POST',
      body: JSON.stringify({ username, email, password }),
    }),
  login: (username: string, password: string) =>
    request<{ token: string; user: User }>('/api/auth/login', {
      method: 'POST',
      body: JSON.stringify({ username, password }),
    }),
  me: () => request<User>('/api/auth/me'),

  // repos
  listRepos: (owner?: string) =>
    request<Repository[]>(`/api/repos${owner ? `?owner=${owner}` : ''}`),
  createRepo: (name: string, description: string, default_branch: string, is_private: boolean) =>
    request<Repository>('/api/repos', {
      method: 'POST',
      body: JSON.stringify({ name, description, default_branch, is_private }),
    }),
  deleteRepo: (owner: string, name: string) =>
    request<{ message: string }>(`/api/repos/${owner}/${name}`, { method: 'DELETE' }),
  getRepo: (owner: string, name: string) =>
    request<Repository>(`/api/repos/${owner}/${name}`),
  getTree: (owner: string, name: string, path: string, ref: string) =>
    request<{ path: string; ref: string; entries: TreeEntry[] }>(
      `/api/repos/${owner}/${name}/tree?path=${encodeURIComponent(path)}&ref=${encodeURIComponent(ref)}`,
    ),
  getFile: (owner: string, name: string, path: string, ref: string) =>
    request<FileContent>(
      `/api/repos/${owner}/${name}/file?path=${encodeURIComponent(path)}&ref=${encodeURIComponent(ref)}`,
    ),
  getCommits: (owner: string, name: string, ref: string, limit = 30) =>
    request<{ ref: string; commits: Commit[] }>(
      `/api/repos/${owner}/${name}/commits?ref=${encodeURIComponent(ref)}&limit=${limit}`,
    ),
  getBranches: (owner: string, name: string) =>
    request<string[]>(`/api/repos/${owner}/${name}/branches`),

  // PRs
  createPr: (owner: string, name: string, pr: {
    title: string
    body: string
    source_branch: string
    target_branch: string
  }) =>
    request<PullRequest>(`/api/repos/${owner}/${name}/prs`, {
      method: 'POST',
      body: JSON.stringify(pr),
    }),
  listPrs: (owner: string, name: string, status?: string) =>
    request<{ prs: PullRequest[] }>(
      `/api/repos/${owner}/${name}/prs${status ? `?status=${status}` : ''}`,
    ),
  getPr: (owner: string, name: string, number: number) =>
    request<{ pr: PullRequest; reviews: Review[] }>(`/api/repos/${owner}/${name}/prs/${number}`),
  mergePr: (owner: string, name: string, number: number) =>
    request<{ message: string }>(`/api/repos/${owner}/${name}/prs/${number}/merge`, {
      method: 'POST',
    }),
  closePr: (owner: string, name: string, number: number) =>
    request<{ message: string }>(`/api/repos/${owner}/${name}/prs/${number}/close`, {
      method: 'POST',
    }),
  triggerReview: (owner: string, name: string, number: number) =>
    request<Review>(`/api/repos/${owner}/${name}/prs/${number}/review`, { method: 'POST' }),

  // reviews
  listReviews: () =>
    request<{ reviews: Review[] }>('/api/reviews'),
  getReview: (id: number) => request<Review>(`/api/reviews/${id}`),

  // review discussion comments
  listReviewComments: (reviewId: number) =>
    request<{ review_id: number; comments: ReviewDiscussionComment[] }>(`/api/reviews/${reviewId}/comments`),
  addReviewComment: (reviewId: number, body: string) =>
    request<ReviewDiscussionComment>(`/api/reviews/${reviewId}/comments`, {
      method: 'POST',
      body: JSON.stringify({ body }),
    }),
  editReviewComment: (commentId: number, body: string) =>
    request<ReviewDiscussionComment>(`/api/reviews/comments/${commentId}`, {
      method: 'PATCH',
      body: JSON.stringify({ body }),
    }),
  deleteReviewComment: (commentId: number) =>
    request<{ message: string }>(`/api/reviews/comments/${commentId}`, { method: 'DELETE' }),

  // review modification (owner only)
  updateReview: (id: number, summary?: string, riskScore?: number) =>
    request<Review>(`/api/reviews/${id}`, {
      method: 'PATCH',
      body: JSON.stringify({ summary, risk_score: riskScore }),
    }),
  resolveReviewComment: (commentId: number) =>
    request<{ message: string }>(`/api/reviews/comments/${commentId}/resolve`, { method: 'POST' }),
  unresolveReviewComment: (commentId: number) =>
    request<{ message: string }>(`/api/reviews/comments/${commentId}/unresolve`, { method: 'POST' }),

  // issues
  createIssue: (owner: string, name: string, title: string, body: string) =>
    request<Issue>(`/api/repos/${owner}/${name}/issues`, {
      method: 'POST',
      body: JSON.stringify({ title, body }),
    }),
  listIssues: (owner: string, name: string, status?: string) =>
    request<{ issues: Issue[] }>(
      `/api/repos/${owner}/${name}/issues${status ? `?status=${status}` : ''}`,
    ),
  closeIssue: (owner: string, name: string, number: number) =>
    request<{ message: string }>(`/api/repos/${owner}/${name}/issues/${number}/close`, {
      method: 'POST',
    }),

  // search
  search: (q: string, repo?: string) =>
    request<{ query: string; hits: SearchHit[] }>(
      `/api/search?q=${encodeURIComponent(q)}${repo ? `&repo=${encodeURIComponent(repo)}` : ''}`,
    ),

  // events
  listEvents: (limit = 50) =>
    request<{ events: EventItem[] }>(`/api/events?limit=${limit}`),

  // health
  health: () => request<{ status: string; db: string; ai_configured: boolean; git: string }>('/api/health'),
}
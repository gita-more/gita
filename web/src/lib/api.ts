// Gita API client types and helpers.

export interface User {
  id: number
  username: string
  email: string
  /** UI language preference ("zh" | "en"). */
  locale: string
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
  /** Files that conflict when merging head into target (empty = clean). */
  conflicts: string[]
  merged_at: string | null
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

export interface Comment {
  id: number
  repo_id: number
  pr_id: number | null
  issue_id: number | null
  author: string
  body: string
  created_at: string
}

export interface Token {
  id: number
  token: string
  name: string
  created_at: string
}

export interface RepoSearchHit {
  id: number
  owner: string
  name: string
  full_name: string
  description: string
  is_private: boolean
}

export interface CodeSearchHit {
  owner: string
  repo: string
  path: string
  line: number
  content: string
}

export interface Group {
  id: number
  name: string
  description: string
  owner_id: number
  created_at: string
}

/** Issue item as returned by the global /api/issues feed. */
export interface GlobalIssueItem {
  id: number
  number: number
  title: string
  body: string
  status: 'open' | 'closed'
  author: string
  /** Owner of the repository this issue belongs to (e.g. "acme" or a username). */
  repo_owner: string
  /** Repository name. */
  repo_name: string
  /** "owner/name" convenience field. */
  full_name: string
  created_at: string
}

export interface EventItem {
  id: number
  event_type: string
  repo_id: number | null
  actor_id: number | null
  payload: Record<string, unknown>
  created_at: string
}

export interface Pipeline {
  id: number
  repo_id: number
  path: string
  ref: string
  ref_sha: string
  content: string
  created_at: string
  updated_at: string
}

export interface PipelineRun {
  id: number
  pipeline_id: number
  repo_id: number
  ref: string
  head_sha: string
  trigger_event_id: number | null
  trigger_pusher_id: number | null
  status: 'queued' | 'running' | 'success' | 'failed' | 'canceled'
  queued_at: string
  started_at: string | null
  finished_at: string | null
}

export interface Job {
  id: number
  run_id: number
  name: string
  stage: string
  needs: string[]
  script: string[]
  status: 'queued' | 'running' | 'success' | 'failed' | 'skipped' | 'canceled'
  queued_at: string
  started_at: string | null
  finished_at: string | null
  exit_code: number | null
}

export interface JobLogLine {
  seq: number
  stream: 'stdout' | 'stderr' | 'system'
  line: string
  ts: string
}

let token: string | null = null
export function setToken(t: string | null) {
  token = t
}
export function getToken() {
  return token
}

export class ApiError extends Error {
  /** The raw error message returned by the server (English). */
  serverMessage: string
  status: number

  constructor(message: string, serverMessage: string, status: number) {
    super(message)
    this.serverMessage = serverMessage
    this.status = status
  }
}

async function request<T>(path: string, options: RequestInit = {}): Promise<T> {
  const headers: Record<string, string> = {
    'Content-Type': 'application/json',
  }
  if (token) headers['Authorization'] = `Bearer ${token}`
  const res = await fetch(path, { ...options, headers })
  if (!res.ok) {
    let message = `HTTP ${res.status}`
    let serverMessage = message
    let status = res.status
    try {
      const body = await res.json()
      if (body?.error?.message) {
        message = body.error.message
        serverMessage = body.error.message
        status = body.error.status ?? res.status
      }
    } catch {
      // ignore parse errors
    }
    throw new ApiError(message, serverMessage, status)
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
  updateLocale: (locale: string) =>
    request<User>('/api/auth/me', {
      method: 'PATCH',
      body: JSON.stringify({ locale }),
    }),

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
  createBranch: (owner: string, name: string, branch: string, ref?: string) =>
    request<{ branch: string }>(`/api/repos/${owner}/${name}/branches`, {
      method: 'POST',
      body: JSON.stringify({ name: branch, ref }),
    }),
  deleteBranch: (owner: string, name: string, branch: string) =>
    request<{ message: string }>(`/api/repos/${owner}/${name}/branches/${encodeURIComponent(branch)}`, {
      method: 'DELETE',
    }),
  listTags: (owner: string, name: string) =>
    request<string[]>(`/api/repos/${owner}/${name}/tags`),
  createTag: (owner: string, name: string, tag: string, ref?: string) =>
    request<{ tag: string }>(`/api/repos/${owner}/${name}/tags`, {
      method: 'POST',
      body: JSON.stringify({ name: tag, ref }),
    }),
  deleteTag: (owner: string, name: string, tag: string) =>
    request<{ message: string }>(`/api/repos/${owner}/${name}/tags/${encodeURIComponent(tag)}`, {
      method: 'DELETE',
    }),

  // tokens
  createToken: (name?: string) =>
    request<{ id: number; token: string; name: string }>('/api/auth/tokens', {
      method: 'POST',
      body: JSON.stringify({ name }),
    }),
  listTokens: () => request<Token[]>(`/api/auth/tokens`),
  deleteToken: (id: number) =>
    request<{ message: string }>(`/api/auth/tokens/${id}`, { method: 'DELETE' }),

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
    request<{ pr: PullRequest }>(`/api/repos/${owner}/${name}/prs/${number}`),
  getPrDiff: (owner: string, name: string, number: number) =>
    request<{ diff: string }>(`/api/repos/${owner}/${name}/prs/${number}/diff`),
  getPrFiles: (owner: string, name: string, number: number) =>
    request<{ files: [string, string][] }>(`/api/repos/${owner}/${name}/prs/${number}/files`),
  mergePr: (owner: string, name: string, number: number) =>
    request<{ message: string; merge_sha: string }>(`/api/repos/${owner}/${name}/prs/${number}/merge`, {
      method: 'POST',
    }),
  closePr: (owner: string, name: string, number: number) =>
    request<{ message: string }>(`/api/repos/${owner}/${name}/prs/${number}/close`, {
      method: 'POST',
    }),

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

  // comments
  listPrComments: (owner: string, name: string, number: number) =>
    request<{ comments: Comment[] }>(
      `/api/repos/${owner}/${name}/prs/${number}/comments`,
    ),
  createPrComment: (owner: string, name: string, number: number, body: string) =>
    request<Comment>(`/api/repos/${owner}/${name}/prs/${number}/comments`, {
      method: 'POST',
      body: JSON.stringify({ body }),
    }),
  listIssueComments: (owner: string, name: string, number: number) =>
    request<{ comments: Comment[] }>(
      `/api/repos/${owner}/${name}/issues/${number}/comments`,
    ),
  createIssueComment: (owner: string, name: string, number: number, body: string) =>
    request<Comment>(`/api/repos/${owner}/${name}/issues/${number}/comments`, {
      method: 'POST',
      body: JSON.stringify({ body }),
    }),
  deleteComment: (id: number) =>
    request<{ message: string }>(`/api/comments/${id}`, { method: 'DELETE' }),

  // pipelines
  listPipelines: (owner: string, name: string) =>
    request<{ pipelines: Pipeline[] }>(`/api/repos/${owner}/${name}/pipelines`),
  listRuns: (owner: string, name: string, path = '.gita-ci.yml') =>
    request<{ runs: PipelineRun[] }>(
      `/api/repos/${owner}/${name}/pipelines/${encodeURIComponent(path)}/runs`,
    ),
  triggerRun: (owner: string, name: string, path = '.gita-ci.yml') =>
    request<{ run: PipelineRun }>(
      `/api/repos/${owner}/${name}/pipelines/${encodeURIComponent(path)}/runs`,
      { method: 'POST' },
    ),
  getRun: (owner: string, name: string, runId: number) =>
    request<{ run: PipelineRun; jobs: Job[] }>(
      `/api/repos/${owner}/${name}/runs/${runId}`,
    ),
  getJobLogs: (
    owner: string,
    name: string,
    runId: number,
    jobId: number,
    since = -1,
  ) =>
    request<{ lines: JobLogLine[] }>(
      `/api/repos/${owner}/${name}/runs/${runId}/jobs/${jobId}/logs?since=${since}`,
    ),
  /**
   * Subscribe to live log + status events for one job over Server-Sent
   * Events. Returns an EventSource-like object:
   *   - onLog(line) is called for every new log row.
   *   - onStatus(job) is called once when the job enters a terminal state.
   *   - onError() is called on a network or parse error.
   * Call .close() to abort the connection.
   */
  streamJobLogs: (
    owner: string,
    name: string,
    runId: number,
    jobId: number,
    handlers: {
      onLog: (line: JobLogLine) => void
      onStatus: (job: Job) => void
      onError?: (e: unknown) => void
    },
  ): { close: () => void } => {
    const url = `/api/repos/${owner}/${name}/runs/${runId}/jobs/${jobId}/logs/stream`
    const es = new EventSource(url, { withCredentials: true })
    const onLog = (e: MessageEvent) => {
      try {
        handlers.onLog(JSON.parse(e.data) as JobLogLine)
      } catch (err) {
        handlers.onError?.(err)
      }
    }
    const onStatus = (e: MessageEvent) => {
      try {
        handlers.onStatus(JSON.parse(e.data) as Job)
      } catch (err) {
        handlers.onError?.(err)
      }
    }
    es.addEventListener('log', onLog as EventListener)
    es.addEventListener('status', onStatus as EventListener)
    es.onerror = (e) => handlers.onError?.(e)
    return {
      close: () => {
        es.removeEventListener('log', onLog as EventListener)
        es.removeEventListener('status', onStatus as EventListener)
        es.close()
      },
    }
  },
  cancelRun: (owner: string, name: string, runId: number) =>
    request<{ message: string }>(
      `/api/repos/${owner}/${name}/runs/${runId}/cancel`,
      { method: 'POST' },
    ),

  // global
  listGlobalIssues: (status?: 'open' | 'closed' | 'all', limit = 50) =>
    request<{ issues: GlobalIssueItem[] }>(
      `/api/issues${status ? `?status=${status}` : ''}&limit=${limit}`,
    ),

  // groups
  listGroups: () => request<Group[]>('/api/groups'),
  createGroup: (name: string, description: string) =>
    request<Group>('/api/groups', {
      method: 'POST',
      body: JSON.stringify({ name, description }),
    }),
  getGroup: (name: string) =>
    request<{ group: Group; repos: Repository[] }>(`/api/groups/${encodeURIComponent(name)}`),
  createGroupRepo: (
    group: string,
    name: string,
    description: string,
    is_private: boolean,
    default_branch: string,
  ) =>
    request<Repository>(`/api/groups/${encodeURIComponent(group)}/repos`, {
      method: 'POST',
      body: JSON.stringify({ name, description, is_private, default_branch }),
    }),

  // search
  search: (q: string, type_?: 'repos' | 'code' | 'all') =>
    request<{ repos?: RepoSearchHit[]; code?: CodeSearchHit[] }>(
      `/api/search?q=${encodeURIComponent(q)}${type_ ? `&type=${type_}` : ''}`,
    ),
  searchRepos: (q: string) =>
    request<{ hits: RepoSearchHit[] }>(
      `/api/search/repos?q=${encodeURIComponent(q)}`,
    ),
  searchCode: (q: string) =>
    request<{ hits: CodeSearchHit[] }>(
      `/api/search/code?q=${encodeURIComponent(q)}`,
    ),

  // events
  listEvents: (limit = 50) =>
    request<{ events: EventItem[] }>(`/api/events?limit=${limit}`),

  // health
  health: () => request<{ status: string; db: string; git: string }>('/api/health'),
}
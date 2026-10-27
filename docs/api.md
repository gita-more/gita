# Gita REST API

AI-native Git server REST API. Standard GitHub-like endpoints, PR/Issue workflows and an AI-readable event stream.

规范版本 `0.0.1`。运行中的服务器同时提供：

- OpenAPI 文档：`/api/docs/openapi.json`
- Swagger UI：`/api/docs`

> 本文件由 `crates/gita-api/src/docs.rs` 里的 OpenAPI 文档生成，不要手工编辑。
> 重新生成：`cargo run -p gita-api --bin gen-api-docs > docs/api.md`

## 约定

- **认证**：除特别说明外，接口需要 `Authorization: Bearer <token>`。Token 可以是登录得到的 JWT，也可以是个人访问令牌（PAT）。
- **错误**：所有失败都返回同一个信封 `{"error": {"message": "…", "status": 404}}`，`status` 与 HTTP 状态码一致。
- **拒绝方式**：没有权限访问的仓库一律返回 `404`，而不是 `403`——拒绝本身不应该泄露“这个仓库存在”。
- **路径参数**：`{owner}` 可以是用户名，也可以是群组名。

## auth

User registration, login and personal access tokens

### `POST /api/auth/init`

Initialize the system by creating the first admin account

Only accepted when the users table is completely empty. Can only ever run once. Returns a JWT for the new admin.

- 无需认证
- 响应码：`200`, `400`, `409`

### `GET /api/auth/initialized`

Whether the system has been initialized yet

True once any user account exists. Unauthenticated, so the frontend can decide whether to show the init page before any account exists.

- 无需认证
- 响应码：`200`

### `GET /api/auth/keys`

List SSH public keys of the current user

- 需要认证
- 响应码：`200`, `401`

### `POST /api/auth/keys`

Register a new SSH public key

- 需要认证
- 响应码：`200`, `400`, `401`

### `DELETE /api/auth/keys/{id}`

Delete an SSH public key

- 需要认证
- 响应码：`200`, `401`, `404`

### `POST /api/auth/login`

Log in and obtain a JWT

- 无需认证
- 响应码：`200`, `401`

### `POST /api/auth/logout`

Log out and revoke the current session

Revokes the JWT used for this request by adding its JTI to the revocation list. The token can no longer be used for authentication. When the request presents a personal access token instead of a JWT, there is no session to end: the response says so and the PAT is left untouched — revoke a PAT explicitly with DELETE /api/auth/tokens/{id}.

- 需要认证
- 响应码：`200`, `401`

### `GET /api/auth/me`

Get the current authenticated user

- 需要认证
- 响应码：`200`, `401`

### `PATCH /api/auth/me`

Update the current user's locale, e-mail or password

Every field is optional, so a body-less PATCH is a no-op. Changing the e-mail or the password requires `current_password`.

- 需要认证
- 响应码：`200`, `400`, `401`, `409`

### `POST /api/auth/password-reset/confirm`

Complete a password reset with a token

The token is single-use: it is consumed by the same statement that validates it, so a token can never be redeemed twice. Rate limited per IP.

- 无需认证
- 响应码：`200`, `400`, `404`, `429`

### `POST /api/auth/password-reset/request`

Request a password-reset link

Always returns 200, whether or not the account exists, so the endpoint cannot be used to enumerate accounts. When the server runs with `allow_insecure_password_reset` enabled, the response carries the token in a `reset_token` field; otherwise that field is absent. Rate limited per IP.

- 无需认证
- 响应码：`200`, `429`

### `POST /api/auth/register`

Register a new user

- 无需认证
- 响应码：`200`, `400`, `409`

### `GET /api/auth/tokens`

List personal access tokens

- 需要认证
- 响应码：`200`

### `POST /api/auth/tokens`

Create a personal access token

- 需要认证
- 响应码：`200`

### `DELETE /api/auth/tokens/{id}`

Revoke a personal access token

- 需要认证
- 响应码：`200`, `404`

## admin

Administrator user management

### `GET /api/admin/users`

List all users (admin only)

Requires the caller to be a system administrator. Returns every registered account with its admin flag.

- 需要认证
- 响应码：`200`, `403`

### `POST /api/admin/users`

Create a user (admin only)

Requires the caller to be a system administrator. Creates an account with the given username, email and password; optionally promotes it to administrator.

- 需要认证
- 响应码：`200`, `400`, `403`, `409`

### `DELETE /api/admin/users/{username}`

Delete a user (admin only)

Requires the caller to be a system administrator. Deletes the account together with its personal repositories and owned groups. Refuses to delete the last administrator or the caller's own account.

- 需要认证
- 响应码：`200`, `400`, `403`, `404`

### `PATCH /api/admin/users/{username}`

Update a user (admin only)

Requires the caller to be a system administrator. Can change the admin flag, email, locale or password. Refuses to demote the very last administrator.

- 需要认证
- 响应码：`200`, `400`, `403`, `404`, `409`

## repos

Repository CRUD and content browsing

### `GET /api/repos`

List repositories

- 需要认证
- 查询参数：`owner`, `limit`
- 响应码：`200`

### `POST /api/repos`

Create a repository

- 需要认证
- 响应码：`200`, `400`, `409`

### `DELETE /api/repos/{*path}`

Delete a repository or a sub-resource (owner only)

Suffix dispatch: (empty) delete the repository, /star unstar, /hooks/{id} delete a webhook, /branches/{branch} delete a branch, /tags/{tag} delete a tag.

- 需要认证
- 响应码：`200`, `404`

### `GET /api/repos/{*path}`

Repository metadata or a read-only sub-resource

Suffix dispatch: (empty) get the repository, /tree list a directory (query: ref, path), /file read a file (query: ref, path), /commits list commits (query: ref, limit), /commits/{sha} get one commit with its diff, /branches list branches, /tags list tags, /star get star status, /hooks list webhooks.

- 需要认证
- 响应码：`200`, `404`

### `PATCH /api/repos/{*path}`

Update a repository (owner only)

Change the description, the visibility, the name, or transfer the repository to another user or group. A non-owner gets 404, not 403: the refusal must not disclose that the repository exists.

- 需要认证
- 响应码：`200`, `400`, `404`, `409`

### `POST /api/repos/{*path}`

Create a sub-resource

Suffix dispatch: /fork fork the repository under the caller's namespace (optional body: {name}), /hooks create a webhook, /branches create a branch ({name, ref}), /tags create a tag ({name, ref}).

- 需要认证
- 响应码：`200`, `400`, `403`, `404`, `409`

### `PUT /api/repos/{*path}`

Star a repository

- 需要认证
- 响应码：`200`, `401`, `404`

### `DELETE /api/repos/{owner}/{repo}/pages`

Disable Pages for a repository

- 需要认证
- 响应码：`200`, `404`

### `GET /api/repos/{owner}/{repo}/pages`

Get the Pages publishing configuration for a repository

- 需要认证
- 响应码：`200`, `404`

### `PUT /api/repos/{owner}/{repo}/pages`

Set the Pages publishing configuration

- 需要认证
- 响应码：`200`, `400`, `404`

## prs

Pull request workflow

### `GET /api/repos/{owner}/{repo}/prs`

List pull requests

- 无需认证
- 查询参数：`status`
- 响应码：`200`

### `POST /api/repos/{owner}/{repo}/prs`

Create a pull request

- 需要认证
- 响应码：`200`, `400`

### `GET /api/repos/{owner}/{repo}/prs/{number}`

Get a pull request

- 无需认证
- 响应码：`200`, `404`

### `POST /api/repos/{owner}/{repo}/prs/{number}/close`

Close a pull request

- 需要认证
- 响应码：`200`

### `GET /api/repos/{owner}/{repo}/prs/{number}/comments`

List a pull request's comments

- 需要认证
- 响应码：`200`

### `POST /api/repos/{owner}/{repo}/prs/{number}/comments`

Comment on a pull request

- 需要认证
- 响应码：`200`

### `GET /api/repos/{owner}/{repo}/prs/{number}/diff`

Get a pull request's diff

- 需要认证
- 响应码：`200`

### `GET /api/repos/{owner}/{repo}/prs/{number}/files`

List the files a pull request changes

- 需要认证
- 响应码：`200`

### `POST /api/repos/{owner}/{repo}/prs/{number}/merge`

Merge a pull request

- 需要认证
- 响应码：`200`, `409`

## issues

Issue tracking

### `GET /api/issues`

List issues across repositories

- 需要认证
- 响应码：`200`

### `GET /api/repos/{owner}/{repo}/issues`

List issues

- 无需认证
- 查询参数：`status`
- 响应码：`200`

### `POST /api/repos/{owner}/{repo}/issues`

Create an issue

- 需要认证
- 响应码：`200`

### `GET /api/repos/{owner}/{repo}/issues/{number}`

Get an issue

- 需要认证
- 响应码：`200`, `404`

### `POST /api/repos/{owner}/{repo}/issues/{number}/close`

Close an issue

- 需要认证
- 响应码：`200`

### `GET /api/repos/{owner}/{repo}/issues/{number}/comments`

List an issue's comments

- 需要认证
- 响应码：`200`

### `POST /api/repos/{owner}/{repo}/issues/{number}/comments`

Comment on an issue

- 需要认证
- 响应码：`200`

## events

AI-readable event stream

### `GET /api/events`

List the AI-readable event stream

- 需要认证
- 查询参数：`repo`, `type`, `limit`
- 响应码：`200`

## groups

Groups and membership

### `GET /api/groups`

List groups

- 需要认证
- 响应码：`200`

### `POST /api/groups`

Create a group

- 需要认证
- 响应码：`200`

### `DELETE /api/groups/{*path}`

Remove a group member

- 需要认证
- 响应码：`200`, `403`, `404`

### `GET /api/groups/{*path}`

Get a group and its repositories, or list subgroups/members

Use path suffix to select: (empty) for group detail, /subgroups for subgroup list, /members for member list.

- 需要认证
- 响应码：`200`, `404`

### `PATCH /api/groups/{*path}`

Update a group member's role

- 需要认证
- 响应码：`200`, `403`, `404`

### `POST /api/groups/{*path}`

Create a repository or subgroup, or add a member

Use path suffix to select action: /repos to create repo, /subgroups to create subgroup, /members to add member.

- 需要认证
- 响应码：`200`, `403`, `404`

## search

Repository and code search

### `GET /api/search`

Search repositories and code

No authentication required. An anonymous caller only sees public repositories; an authenticated one also sees the private repositories they may read.

- 无需认证
- 查询参数：`q`, `type`, `limit`
- 响应码：`200`, `400`

### `GET /api/search/code`

Search code in all repositories

No authentication required. The search only covers the repositories the caller may read, so an anonymous caller only ever matches public code.

- 无需认证
- 查询参数：`q`, `limit`
- 响应码：`200`

### `GET /api/search/repos`

Search repositories by name/description

No authentication required. An anonymous caller only sees public repositories.

- 无需认证
- 查询参数：`q`, `limit`
- 响应码：`200`

## pipelines

In-repo CI pipelines, runs and job logs

### `GET /api/repos/{owner}/{repo}/pipelines`

List the pipelines defined in the repository's .gita-ci.yml

- 需要认证
- 响应码：`200`

### `GET /api/repos/{owner}/{repo}/pipelines/{path}/runs`

List runs of a pipeline

- 需要认证
- 响应码：`200`

### `POST /api/repos/{owner}/{repo}/pipelines/{path}/runs`

Trigger a run of a pipeline

- 需要认证
- 响应码：`200`, `404`

### `GET /api/repos/{owner}/{repo}/runs/{run_id}`

Get a run with its jobs

- 需要认证
- 响应码：`200`, `404`

### `POST /api/repos/{owner}/{repo}/runs/{run_id}/cancel`

Cancel a run

- 需要认证
- 响应码：`200`, `404`

### `GET /api/repos/{owner}/{repo}/runs/{run_id}/jobs/{job_id}/logs`

Get a job's captured logs

- 需要认证
- 响应码：`200`

### `GET /api/repos/{owner}/{repo}/runs/{run_id}/jobs/{job_id}/logs/stream`

Stream a job's logs as they are produced (SSE)

- 需要认证
- 响应码：`200`

## pages

Static site (Pages) publishing configuration

### `GET /api/repos/{owner}/{repo}/pages/status`

Whether the repository has a Pages configuration

- 需要认证
- 响应码：`200`

## misc

Health and metadata

### `DELETE /api/comments/{id}`

Delete a comment

- 需要认证
- 响应码：`200`, `404`

### `GET /api/health`

Health check

- 无需认证
- 响应码：`200`

### `GET /api/mcp/tools`

List the tools exposed to MCP clients

- 无需认证
- 响应码：`200`

### `GET /api/resolve/{*path}`

Resolve a clean URL path to the entity it addresses

A clean URL (/owner/name) can address a repository or a group full path. Tries the path as a repository first (read access enforced), then as a group. Returns {kind: "repo", owner, name, owner_kind} or {kind: "group", full_path}; 404 when neither matches.

- 需要认证
- 响应码：`200`, `404`

### `POST /api/users/me/avatar`

Upload the current user's avatar image

- 需要认证
- 响应码：`200`, `400`

### `GET /api/users/{username}/avatar`

Fetch a user's avatar image

- 无需认证
- 响应码：`200`, `404`


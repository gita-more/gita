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

### `DELETE /api/repos/{owner}/{repo}`

Delete a repository (owner only)

- 需要认证
- 响应码：`200`, `404`

### `GET /api/repos/{owner}/{repo}`

Get a repository

- 无需认证
- 响应码：`200`, `404`

### `PATCH /api/repos/{owner}/{repo}`

Update a repository (owner only)

Change the description, the visibility, the name, or transfer the repository to another user or group. A non-owner gets 404, not 403: the refusal must not disclose that the repository exists.

- 需要认证
- 响应码：`200`, `400`, `404`, `409`

### `GET /api/repos/{owner}/{repo}/branches`

List branches

- 无需认证
- 响应码：`200`

### `POST /api/repos/{owner}/{repo}/branches`

Create a branch

Creating a branch changes the repository's refs, so it takes the same rights as pushing to it.

- 需要认证
- 响应码：`200`, `400`, `404`

### `DELETE /api/repos/{owner}/{repo}/branches/{branch}`

Delete a branch

- 需要认证
- 响应码：`200`, `404`

### `GET /api/repos/{owner}/{repo}/commits/{sha}`

Get a single commit with its diff

- 需要认证
- 响应码：`200`

### `GET /api/repos/{owner}/{repo}/file`

Read a file's content at a ref

- 需要认证
- 响应码：`200`, `404`

### `POST /api/repos/{owner}/{repo}/fork`

Fork this repository under the caller's namespace

- 需要认证
- 响应码：`200`, `409`

### `GET /api/repos/{owner}/{repo}/hooks`

List webhooks for a repository

- 需要认证
- 响应码：`200`

### `POST /api/repos/{owner}/{repo}/hooks`

Create a webhook

- 需要认证
- 响应码：`200`, `400`, `404`

### `DELETE /api/repos/{owner}/{repo}/hooks/{id}`

Delete a webhook

- 需要认证
- 响应码：`200`, `404`

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

### `DELETE /api/repos/{owner}/{repo}/star`

Unstar a repository

- 需要认证
- 响应码：`200`

### `GET /api/repos/{owner}/{repo}/star`

Check if the authenticated user starred this repo

- 需要认证
- 响应码：`200`

### `PUT /api/repos/{owner}/{repo}/star`

Star a repository

- 需要认证
- 响应码：`200`

### `GET /api/repos/{owner}/{repo}/tags`

List tags

- 需要认证
- 响应码：`200`

### `POST /api/repos/{owner}/{repo}/tags`

Create a tag

- 需要认证
- 响应码：`200`

### `DELETE /api/repos/{owner}/{repo}/tags/{tag}`

Delete a tag

- 需要认证
- 响应码：`200`, `404`

### `GET /api/repos/{owner}/{repo}/tree`

List files in a directory of a repository

- 无需认证
- 查询参数：`path`, `ref`
- 响应码：`200`, `404`

## commits

Commit history

### `GET /api/repos/{owner}/{repo}/commits`

List recent commits

- 无需认证
- 查询参数：`ref`, `limit`
- 响应码：`200`

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

### `GET /api/groups/{group}`

Get a group and its repositories

Readable without authentication, but only public repositories are listed to an anonymous caller.

- 需要认证
- 响应码：`200`, `404`

### `GET /api/groups/{group}/members`

List group members

Only the group owner or an existing member may read the membership list.

- 需要认证
- 响应码：`200`, `401`, `403`

### `POST /api/groups/{group}/members`

Add a member

- 需要认证
- 响应码：`200`, `400`, `403`

### `DELETE /api/groups/{group}/members/{username}`

Remove a member

- 需要认证
- 响应码：`200`, `404`

### `PATCH /api/groups/{group}/members/{username}`

Update a member's role

- 需要认证
- 响应码：`200`

### `POST /api/groups/{group}/repos`

Create a repository owned by a group

- 需要认证
- 响应码：`200`, `403`

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

### `POST /api/users/me/avatar`

Upload the current user's avatar image

- 需要认证
- 响应码：`200`, `400`

### `GET /api/users/{username}/avatar`

Fetch a user's avatar image

- 无需认证
- 响应码：`200`, `404`


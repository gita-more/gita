//! Repository routes: create, list, get, delete, browse tree/files/commits.
//!
//! Repository paths may address a *nested group* namespace
//! (`/api/repos/groupa/subgroupa/repo`), so every sub-resource is served
//! through one catch-all route whose handler dispatches on the path's
//! suffix (`/tree`, `/file`, `/commits`, ...). A plain two-segment
//! pattern cannot express a multi-segment owner, and axum only allows a
//! catch-all at the very end of a route — hence one route, one dispatcher.

use crate::error::{ApiError, ApiResult};
use crate::extract::{AuthUserExtractor, OptionalAuthUser};
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::{Path, Query, RawQuery, State};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use gita_core::{EventType, Repository};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct CreateRepoRequest {
    pub name: String,
    pub description: Option<String>,
    pub is_private: Option<bool>,
    pub default_branch: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RepoResponse {
    pub id: i64,
    pub owner: String,
    pub name: String,
    pub full_name: String,
    pub description: String,
    pub is_private: bool,
    pub default_branch: String,
    pub created_at: String,
}

impl From<Repository> for RepoResponse {
    fn from(r: Repository) -> Self {
        let owner_name = r.owner_name.clone();
        let name = r.name.clone();
        Self {
            id: r.id,
            owner: owner_name.clone(),
            name: name.clone(),
            full_name: format!("{}/{}", owner_name, name),
            description: r.description,
            is_private: r.is_private,
            default_branch: r.default_branch,
            created_at: r.created_at.to_rfc3339(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ListReposQuery {
    pub owner: Option<String>,
    pub limit: Option<usize>,
}

pub fn repo_routes() -> Router<AppState> {
    Router::new()
        .route("/api/repos", post(create_repo).get(list_repos))
        // Single catch-all with dispatcher for all sub-resources
        .route(
            "/api/repos/{*path}",
            get(repo_get_dispatcher)
                .delete(repo_delete_dispatcher)
                .patch(repo_patch_dispatcher)
                .post(repo_post_dispatcher)
                .put(repo_put_dispatcher),
        )
}

// ---------------------------------------------------------------------------
// Path / query helpers
// ---------------------------------------------------------------------------

/// Split `owner.../repo` from a catch-all path. The repository name is the
/// last segment; everything before it is the owner (a username, a group
/// name, or a nested group path like `groupa/subgroupa`).
fn parse_owner_repo(path: &str) -> Result<(String, String), ApiError> {
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() < 2 {
        return Err(ApiError::BadRequest("invalid repository path".into()));
    }
    let repo = parts.last().unwrap().to_string();
    let owner = parts[..parts.len() - 1].join("/");
    Ok((owner, repo))
}

/// Split at the *last* occurrence of `sep`, returning `(prefix, remainder)`.
/// The remainder may itself contain slashes (a branch like `feature/x`).
fn split_suffix<'p>(path: &'p str, sep: &str) -> Option<(&'p str, &'p str)> {
    path.rsplit_once(sep)
}

/// Split `{owner}/{repo}/{marker}/...` at the first known sub-resource
/// marker segment (`prs`, `issues`, `pipelines`, `runs`, `pages`).
/// Returns `(owner, repo, marker, rest)`; `owner` may be a nested group
/// path, `rest` is everything after the marker (possibly empty). The first
/// marker occurrence wins, so a namespace literally named like a marker
/// shadows the sub-resource of the same name one level up.
fn split_repo_subpath(path: &str) -> Option<(String, String, &str, String)> {
    const MARKERS: [&str; 5] = ["prs", "issues", "pipelines", "runs", "pages"];
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() < 3 {
        return None;
    }
    for i in 2..parts.len() {
        if MARKERS.contains(&parts[i]) {
            let owner = parts[..i - 1].join("/");
            let repo = parts[i - 1].to_string();
            let rest = parts[i + 1..].join("/");
            return Some((owner, repo, parts[i], rest));
        }
    }
    None
}

/// Decode a raw query string into a map. `RawQuery` is used instead of
/// `Query<T>` so one dispatcher can serve endpoints with different query
/// shapes (`ref`/`path` for trees, `ref`/`limit` for commits).
fn parse_query(raw: &Option<String>) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let Some(raw) = raw else { return map };
    for pair in raw.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        map.insert(percent_decode(k), percent_decode(v));
    }
    map
}

/// Minimal percent-decoding for query keys/values (`+` as space included).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                    out.push(v);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ---------------------------------------------------------------------------
// Top-level /api/repos handlers (kept as normal extractors)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CreateBranchRequest {
    pub name: String,
    #[serde(rename = "ref")]
    pub r#ref: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateTagRequest {
    pub name: String,
    #[serde(rename = "ref")]
    pub r#ref: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct UpdateRepoRequest {
    pub description: Option<String>,
    pub is_private: Option<bool>,
    /// New repository name (rename within the same namespace).
    pub new_name: Option<String>,
    /// Transfer target: a username or group name.
    pub new_owner: Option<String>,
}

async fn create_repo(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Json(req): Json<CreateRepoRequest>,
) -> ApiResult<Json<RepoResponse>> {
    let name = req.name.trim().to_string();
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(ApiError::BadRequest(
            "repository name may only contain letters, numbers, '-', '_', '.'".into(),
        ));
    }
    let description = req.description.unwrap_or_default();
    let is_private = req.is_private.unwrap_or(false);
    let default_branch = req.default_branch.unwrap_or_else(|| "main".to_string());

    if state
        .db
        .get_repository_by_name(&user.username, &name)
        .await?
        .is_some()
    {
        return Err(ApiError::Conflict("repository already exists".into()));
    }

    let repo = state
        .db
        .create_repository(
            user.id,
            &user.username,
            &name,
            &description,
            is_private,
            &default_branch,
            None,
        )
        .await?;

    // Create the bare repo on disk.
    state
        .git
        .create_repo(&user.username, &name, &default_branch)
        .await?;

    state.bus.publish(
        EventType::RepoCreated,
        Some(repo.id),
        Some(user.id),
        serde_json::json!({
            "owner": user.username,
            "repo": name,
            "description": description,
        }),
    );

    Ok(Json(RepoResponse::from(repo)))
}

async fn list_repos(
    State(state): State<AppState>,
    user: OptionalAuthUser,
    Query(q): Query<ListReposQuery>,
) -> ApiResult<Json<Vec<RepoResponse>>> {
    let limit = q.limit.unwrap_or(100).min(500);
    let repos = state
        .db
        .list_repositories(q.owner.as_deref(), limit)
        .await?;
    // Filter out private repositories the caller cannot see. The list is
    // not owner-scoped, so without this an anonymous caller learns that a
    // private repository exists.
    let visible: Vec<RepoResponse> = state
        .db
        .filter_readable(repos, user.0.as_ref().map(|u| u.id))
        .await?
        .into_iter()
        .map(RepoResponse::from)
        .collect();
    Ok(Json(visible))
}

// ---------------------------------------------------------------------------
// Method dispatchers (the only axum handlers for /api/repos/{*path})
//
// Repository paths may address a *nested group* namespace
// (`/api/repos/groupa/subgroupa/repo`), so every sub-resource is served
// through one catch-all route whose handler dispatches on the path's
// suffix (`/tree`, `/file`, `/commits`, ...). A plain two-segment
// pattern cannot express a multi-segment owner, and axum only allows a
// catch-all at the very end of a route — hence one route, one dispatcher
// per HTTP method.
// ---------------------------------------------------------------------------

/// GET /api/repos/{owner}/{repo}[/(tree|file|commits[/sha]|branches|tags|star|hooks)]
async fn repo_get_dispatcher(
    State(state): State<AppState>,
    user: OptionalAuthUser,
    Path(path): Path<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult<axum::response::Response> {
    let q = parse_query(&raw);

    // A specific commit: `.../commits/{sha}` — containment is checked before
    // the bare `/commits` listing so the sha segment is not swallowed.
    if let Some((repo_path, sha)) = split_suffix(&path, "/commits/") {
        if !sha.is_empty() {
            let (owner, repo) = parse_owner_repo(repo_path)?;
            let res = get_commit(&state, &user, &owner, &repo, sha).await?;
            return Ok(res.into_response());
        }
    }
    if let Some(repo_path) = path.strip_suffix("/tree") {
        let (owner, repo) = parse_owner_repo(repo_path)?;
        let res = get_tree(
            &state,
            &user,
            &owner,
            &repo,
            q.get("ref").cloned(),
            q.get("path").cloned(),
        )
        .await?;
        return Ok(res.into_response());
    }
    if let Some(repo_path) = path.strip_suffix("/file") {
        let (owner, repo) = parse_owner_repo(repo_path)?;
        let res = get_file(
            &state,
            &user,
            &owner,
            &repo,
            q.get("ref").cloned(),
            q.get("path").cloned(),
        )
        .await?;
        return Ok(res.into_response());
    }
    if let Some(repo_path) = path.strip_suffix("/commits") {
        let (owner, repo) = parse_owner_repo(repo_path)?;
        let limit = q.get("limit").and_then(|v| v.parse::<usize>().ok());
        let res = list_commits(&state, &user, &owner, &repo, q.get("ref").cloned(), limit).await?;
        return Ok(res.into_response());
    }
    if let Some(repo_path) = path.strip_suffix("/branches") {
        let (owner, repo) = parse_owner_repo(repo_path)?;
        let res = list_branches(&state, &user, &owner, &repo).await?;
        return Ok(res.into_response());
    }
    if let Some(repo_path) = path.strip_suffix("/tags") {
        let (owner, repo) = parse_owner_repo(repo_path)?;
        let res = list_tags(&state, &user, &owner, &repo).await?;
        return Ok(res.into_response());
    }
    if let Some(repo_path) = path.strip_suffix("/star") {
        let (owner, repo) = parse_owner_repo(repo_path)?;
        let res = get_star(&state, &user, &owner, &repo).await?;
        return Ok(res.into_response());
    }
    if let Some(repo_path) = path.strip_suffix("/hooks") {
        let (owner, repo) = parse_owner_repo(repo_path)?;
        let res = list_hooks(&state, &user, &owner, &repo).await?;
        return Ok(res.into_response());
    }
    // Sub-resources owned by the pr/issue/pipeline/pages modules: PRs,
    // issues, pipelines/runs and Pages. Their markers are matched on the
    // *first* occurrence so nested group owners resolve correctly.
    if let Some((owner, repo, marker, rest)) = split_repo_subpath(&path) {
        let res = match marker {
            "prs" => crate::routes::pr::get_dispatch(&state, &user, &owner, &repo, &rest, &q).await?,
            "issues" => {
                crate::routes::issue::get_dispatch(&state, &user, &owner, &repo, &rest, &q)
                    .await?
            }
            "pipelines" | "runs" => {
                crate::routes::pipeline::get_dispatch(&state, &user, &owner, &repo, marker, &rest, &q)
                    .await?
            }
            "pages" => {
                let auth = user
                    .0
                    .ok_or_else(|| ApiError::Unauthorized("authentication required".into()))?;
                crate::routes::pages::get_dispatch(&state, &auth, &owner, &repo, &rest).await?
            }
            _ => unreachable!("split_repo_subpath only yields known markers"),
        };
        return Ok(res);
    }
    let (owner, repo) = parse_owner_repo(&path)?;
    let res = get_repo(&state, &user, &owner, &repo).await?;
    Ok(res.into_response())
}

/// POST /api/repos/{owner}/{repo}[/fork|/hooks|/branches|/tags]
async fn repo_post_dispatcher(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path(path): Path<String>,
    body: Bytes,
) -> ApiResult<axum::response::Response> {
    if let Some(repo_path) = path.strip_suffix("/fork") {
        let (owner, repo) = parse_owner_repo(repo_path)?;
        // The fork's body is optional; an empty body means "default name".
        let req: Option<ForkRequest> = if body.is_empty() {
            None
        } else {
            Some(serde_json::from_slice(&body)
                .map_err(|e| ApiError::BadRequest(format!("invalid request body: {e}")))?)
        };
        let res = post_fork(&state, &user, &owner, &repo, req).await?;
        return Ok(res.into_response());
    }
    if let Some(repo_path) = path.strip_suffix("/hooks") {
        let (owner, repo) = parse_owner_repo(repo_path)?;
        let req: CreateHookRequest = serde_json::from_slice(&body)
            .map_err(|e| ApiError::BadRequest(format!("invalid request body: {e}")))?;
        let res = create_hook(&state, &user, &owner, &repo, req).await?;
        return Ok(res.into_response());
    }
    if let Some(repo_path) = path.strip_suffix("/branches") {
        let (owner, repo) = parse_owner_repo(repo_path)?;
        let req: CreateBranchRequest = serde_json::from_slice(&body)
            .map_err(|e| ApiError::BadRequest(format!("invalid request body: {e}")))?;
        let res = create_branch(&state, &user, &owner, &repo, req).await?;
        return Ok(res.into_response());
    }
    if let Some(repo_path) = path.strip_suffix("/tags") {
        let (owner, repo) = parse_owner_repo(repo_path)?;
        let req: CreateTagRequest = serde_json::from_slice(&body)
            .map_err(|e| ApiError::BadRequest(format!("invalid request body: {e}")))?;
        let res = create_tag(&state, &user, &owner, &repo, req).await?;
        return Ok(res.into_response());
    }
    // Sub-resources owned by the pr/issue/pipeline modules.
    if let Some((owner, repo, marker, rest)) = split_repo_subpath(&path) {
        let res = match marker {
            "prs" => crate::routes::pr::post_dispatch(&state, &user, &owner, &repo, &rest, &body).await?,
            "issues" => {
                crate::routes::issue::post_dispatch(&state, &user, &owner, &repo, &rest, &body)
                    .await?
            }
            "pipelines" | "runs" => {
                crate::routes::pipeline::post_dispatch(&state, &user, &owner, &repo, marker, &rest)
                    .await?
            }
            _ => return Err(ApiError::NotFound("endpoint not found".into())),
        };
        return Ok(res);
    }
    Err(ApiError::NotFound("endpoint not found".into()))
}

/// PUT /api/repos/{owner}/{repo}[/star|/pages]
async fn repo_put_dispatcher(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path(path): Path<String>,
    body: Bytes,
) -> ApiResult<axum::response::Response> {
    if let Some(repo_path) = path.strip_suffix("/star") {
        let (owner, repo) = parse_owner_repo(repo_path)?;
        let res = put_star(&state, &user, &owner, &repo).await?;
        return Ok(res.into_response());
    }
    // Pages is owned by the pages module.
    if let Some((owner, repo, marker, rest)) = split_repo_subpath(&path) {
        if marker == "pages" {
            let res = crate::routes::pages::put_dispatch(&state, &user, &owner, &repo, &rest, &body)
                .await?;
            return Ok(res);
        }
    }
    Err(ApiError::NotFound("endpoint not found".into()))
}

/// PATCH /api/repos/{owner}/{repo}
async fn repo_patch_dispatcher(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path(path): Path<String>,
    body: Bytes,
) -> ApiResult<axum::response::Response> {
    let (owner, repo) = parse_owner_repo(&path)?;
    let req: UpdateRepoRequest = serde_json::from_slice(&body)
        .map_err(|e| ApiError::BadRequest(format!("invalid request body: {e}")))?;
    let res = update_repo(&state, &user, &owner, &repo, req).await?;
    Ok(res.into_response())
}

/// DELETE /api/repos/{owner}/{repo}[/star|/hooks/{id}|/branches/{branch}|/tags/{tag}]
async fn repo_delete_dispatcher(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path(path): Path<String>,
) -> ApiResult<axum::response::Response> {
    if let Some((repo_path, id)) = split_suffix(&path, "/hooks/") {
        let (owner, repo) = parse_owner_repo(repo_path)?;
        let id: i64 = id
            .parse()
            .map_err(|_| ApiError::BadRequest("invalid webhook id".into()))?;
        let res = delete_hook(&state, &user, &owner, &repo, id).await?;
        return Ok(res.into_response());
    }
    if let Some((repo_path, branch)) = split_suffix(&path, "/branches/") {
        let (owner, repo) = parse_owner_repo(repo_path)?;
        let res = delete_branch(&state, &user, &owner, &repo, branch).await?;
        return Ok(res.into_response());
    }
    if let Some((repo_path, tag)) = split_suffix(&path, "/tags/") {
        let (owner, repo) = parse_owner_repo(repo_path)?;
        let res = delete_tag(&state, &user, &owner, &repo, tag).await?;
        return Ok(res.into_response());
    }
    if let Some(repo_path) = path.strip_suffix("/star") {
        let (owner, repo) = parse_owner_repo(repo_path)?;
        let res = delete_star(&state, &user, &owner, &repo).await?;
        return Ok(res.into_response());
    }
    // Pages is owned by the pages module.
    if let Some((owner, repo, marker, rest)) = split_repo_subpath(&path) {
        if marker == "pages" {
            let res =
                crate::routes::pages::delete_dispatch(&state, &user, &owner, &repo, &rest).await?;
            return Ok(res);
        }
        return Err(ApiError::NotFound("endpoint not found".into()));
    }
    let (owner, repo) = parse_owner_repo(&path)?;
    let res = delete_repo(&state, &user, &owner, &repo).await?;
    Ok(res.into_response())
}

// ---------------------------------------------------------------------------
// Sub-resource handlers (plain arguments, called from the dispatchers)
// ---------------------------------------------------------------------------

async fn get_repo(
    state: &AppState,
    user: &OptionalAuthUser,
    owner: &str,
    repo: &str,
) -> ApiResult<Json<RepoResponse>> {
    let repo = crate::routes::visibility::load_readable_repo(state, owner, repo, user.0.as_ref())
        .await?;
    Ok(Json(RepoResponse::from(repo)))
}

async fn delete_repo(
    state: &AppState,
    user: &AuthUserExtractor,
    owner: &str,
    repo: &str,
) -> ApiResult<Json<crate::error::MessageResponse>> {
    let repo = crate::routes::visibility::load_adminable_repo(state, owner, repo, user).await?;
    state.db.delete_repository(repo.id).await?;
    state.git.delete_repo(owner, &repo.name)?;
    state.bus.publish(
        EventType::RepoDeleted,
        Some(repo.id),
        Some(user.id),
        serde_json::json!({"owner": owner, "repo": repo.name}),
    );
    Ok(Json(crate::error::MessageResponse {
        message: "repository deleted".to_string(),
    }))
}

#[derive(Debug, Deserialize, Default)]
pub struct UpdateRepoRequestBody {}

async fn update_repo(
    state: &AppState,
    user: &AuthUserExtractor,
    owner: &str,
    repo: &str,
    req: UpdateRepoRequest,
) -> ApiResult<Json<RepoResponse>> {
    let mut repository = crate::routes::visibility::load_adminable_repo(state, owner, repo, user).await?;

    // Visibility change.
    if let Some(is_private) = req.is_private {
        state
            .db
            .update_repository_visibility(repository.id, is_private)
            .await?;
    }

    // Description change.
    if let Some(desc) = req.description {
        let desc = desc.trim().to_string();
        state
            .db
            .update_repository_description(repository.id, &desc)
            .await?;
    }

    // Rename (within the same namespace).
    if let Some(new_name) = req.new_name {
        let new_name = new_name.trim().to_string();
        if new_name.is_empty()
            || !new_name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        {
            return Err(ApiError::BadRequest(
                "repository name may only contain letters, numbers, '-', '_', '.'".into(),
            ));
        }
        // Conflict check on the new name.
        if state
            .db
            .get_repository_by_name(&repository.owner_name, &new_name)
            .await?
            .is_some()
        {
            return Err(ApiError::Conflict(
                "target repository already exists".into(),
            ));
        }
        state
            .git
            .rename_repo(owner, repo, &repository.owner_name, &new_name)
            .map_err(ApiError::from)?;
        state
            .db
            .rename_repository(repository.id, &repository.owner_name, &new_name)
            .await?;
        repository.name = new_name;
    }

    // Transfer to another owner (user or group).
    if let Some(new_owner) = req.new_owner {
        let new_owner = new_owner.trim().to_string();
        if new_owner.is_empty() {
            return Err(ApiError::BadRequest("new owner is required".into()));
        }
        // Resolve the target namespace: a user or a group.
        let (new_owner_id, new_owner_name, group_id) =
            if let Some(g) = state.db.get_group_by_name(&new_owner).await? {
                (g.owner_id, g.name.clone(), Some(g.id))
            } else if let Some(u) = state.db.get_user_by_username(&new_owner).await? {
                (u.id, u.username.clone(), None)
            } else {
                return Err(ApiError::NotFound("target user or group not found".into()));
            };

        // Conflict check in the target namespace.
        if state
            .db
            .get_repository_by_name(&new_owner_name, &repository.name)
            .await?
            .is_some()
        {
            return Err(ApiError::Conflict(
                "target repository already exists".into(),
            ));
        }

        // Move the bare repo on disk.
        state
            .git
            .rename_repo(owner, repo, &new_owner_name, &repository.name)
            .map_err(ApiError::from)?;
        state
            .db
            .transfer_repository(repository.id, new_owner_id, &new_owner_name, group_id)
            .await?;
        repository.owner_id = new_owner_id;
        repository.owner_name = new_owner_name;
    }

    // Publish an update event (reuse RepoCreated as a coarse "updated" marker
    // for the AI event stream; a dedicated RepoUpdated variant is cleaner).
    state.bus.publish(
        EventType::RepoCreated,
        Some(repository.id),
        Some(user.id),
        serde_json::json!({
            "owner": repository.owner_name,
            "repo": repository.name,
            "action": "updated",
        }),
    );

    let updated = state
        .db
        .get_repository(repository.id)
        .await?
        .ok_or_else(|| ApiError::NotFound("repository not found".into()))?;
    Ok(Json(RepoResponse::from(updated)))
}

async fn get_tree(
    state: &AppState,
    user: &OptionalAuthUser,
    owner: &str,
    repo: &str,
    q_ref: Option<String>,
    q_path: Option<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let repo = crate::routes::visibility::load_readable_repo(state, owner, repo, user.0.as_ref())
        .await?;
    let reference = q_ref.unwrap_or_else(|| repo.default_branch.clone());
    let path = q_path.unwrap_or_default();
    // A reference that does not exist must be reported as such. Listing
    // the tree of a non-existent branch would otherwise return an empty
    // listing, which is indistinguishable from a genuinely empty
    // directory.
    //
    // The exception is a repository with no commits at all: its default
    // branch does not exist *yet*, and the empty tree is the correct
    // answer. The web UI relies on that to show the "quick setup" card
    // for a freshly created repository.
    if !state.git.ref_exists(owner, &repo.name, &reference) {
        let repo_is_empty = state
            .git
            .list_commits(owner, &repo.name, &reference, 1)
            .unwrap_or_default()
            .is_empty()
            && state
                .git
                .list_branches(owner, &repo.name)
                .unwrap_or_default()
                .is_empty();
        if !repo_is_empty {
            return Err(ApiError::NotFound(format!("ref not found: {reference}")));
        }
    }
    let entries = state
        .git
        .list_tree(owner, &repo.name, &reference, &path)
        .map_err(ApiError::from)?;
    Ok(Json(serde_json::json!({
        "path": path,
        "ref": reference,
        "entries": entries,
    })))
}

async fn get_file(
    state: &AppState,
    user: &OptionalAuthUser,
    owner: &str,
    repo: &str,
    q_ref: Option<String>,
    q_path: Option<String>,
) -> ApiResult<Json<gita_core::FileContent>> {
    let repo = crate::routes::visibility::load_readable_repo(state, owner, repo, user.0.as_ref())
        .await?;
    let reference = q_ref.unwrap_or_else(|| repo.default_branch.clone());
    let path = q_path.ok_or_else(|| ApiError::BadRequest("missing 'path' query".into()))?;
    let file = state
        .git
        .read_file(owner, &repo.name, &reference, &path)
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::NotFound("file not found".into()))?;
    Ok(Json(file))
}

async fn list_commits(
    state: &AppState,
    user: &OptionalAuthUser,
    owner: &str,
    repo: &str,
    q_ref: Option<String>,
    q_limit: Option<usize>,
) -> ApiResult<Json<serde_json::Value>> {
    let repo = crate::routes::visibility::load_readable_repo(state, owner, repo, user.0.as_ref())
        .await?;
    let reference = q_ref.unwrap_or_else(|| repo.default_branch.clone());
    let limit = q_limit.unwrap_or(30).min(200);
    let commits = state
        .git
        .list_commits(owner, &repo.name, &reference, limit)
        .map_err(ApiError::from)?;
    Ok(Json(serde_json::json!({
        "ref": reference,
        "commits": commits,
    })))
}

async fn get_commit(
    state: &AppState,
    user: &OptionalAuthUser,
    owner: &str,
    repo: &str,
    sha: &str,
) -> ApiResult<Json<serde_json::Value>> {
    let repository =
        crate::routes::visibility::load_readable_repo(state, owner, repo, user.0.as_ref()).await?;
    let commit = state
        .git
        .get_commit(owner, repo, sha)
        .map_err(|e| crate::routes::not_found_or_internal(&format!("commit {sha}"), e))?;
    let diff = state
        .git
        .show_commit(owner, repo, &commit.sha)
        .map_err(ApiError::from)?;
    Ok(Json(serde_json::json!({
        "commit": {
            "sha": commit.sha,
            "author_name": commit.author_name,
            "author_email": commit.author_email,
            "message": commit.message,
            "parent_shas": commit.parent_shas,
            "authored_at": commit.authored_at.to_rfc3339(),
        },
        "diff": diff,
        "repository": {
            "owner": repository.owner_name,
            "name": repository.name,
        }
    })))
}

async fn list_branches(
    state: &AppState,
    user: &OptionalAuthUser,
    owner: &str,
    repo: &str,
) -> ApiResult<Json<Vec<String>>> {
    let repo = crate::routes::visibility::load_readable_repo(state, owner, repo, user.0.as_ref())
        .await?;
    let branches = state
        .git
        .list_branches(owner, &repo.name)
        .map_err(ApiError::from)?;
    Ok(Json(branches))
}

async fn put_star(
    state: &AppState,
    user: &AuthUserExtractor,
    owner: &str,
    repo: &str,
) -> ApiResult<Json<serde_json::Value>> {
    let repository =
        crate::routes::visibility::load_readable_repo(state, owner, repo, Some(user)).await?;
    let inserted = state.db.add_star(user.id, repository.id).await?;
    let total = state.db.count_stars(repository.id).await?;
    Ok(Json(serde_json::json!({
        "starred": true,
        "new": inserted,
        "count": total,
    })))
}

async fn delete_star(
    state: &AppState,
    user: &AuthUserExtractor,
    owner: &str,
    repo: &str,
) -> ApiResult<Json<serde_json::Value>> {
    let repository =
        crate::routes::visibility::load_readable_repo(state, owner, repo, Some(user)).await?;
    let removed = state.db.remove_star(user.id, repository.id).await?;
    let total = state.db.count_stars(repository.id).await?;
    Ok(Json(serde_json::json!({
        "starred": false,
        "removed": removed,
        "count": total,
    })))
}

async fn get_star(
    state: &AppState,
    user: &OptionalAuthUser,
    owner: &str,
    repo: &str,
) -> ApiResult<Json<serde_json::Value>> {
    let auth = user
        .0
        .as_ref()
        .ok_or_else(|| ApiError::Unauthorized("authentication required".into()))?;
    let repository =
        crate::routes::visibility::load_readable_repo(state, owner, repo, Some(auth)).await?;
    let starred = state.db.has_star(auth.id, repository.id).await?;
    let total = state.db.count_stars(repository.id).await?;
    Ok(Json(serde_json::json!({
        "starred": starred,
        "count": total,
    })))
}

#[derive(Debug, Deserialize, Default)]
pub struct ForkRequest {
    /// Optional override for the fork's name. Defaults to the source name.
    pub name: Option<String>,
}

async fn post_fork(
    state: &AppState,
    user: &AuthUserExtractor,
    owner: &str,
    repo: &str,
    req: Option<ForkRequest>,
) -> ApiResult<Json<RepoResponse>> {
    // The source repository must be readable by the caller. Without this
    // check any registered user could fork somebody else's *private*
    // repository — `fork_repo` performs a real `git clone --bare`, so the
    // entire history would be copied into the attacker's namespace.
    let source =
        crate::routes::visibility::load_readable_repo(state, owner, repo, Some(user)).await?;
    let fork_name = req
        .and_then(|r| r.name)
        .unwrap_or_else(|| source.name.clone());
    let fork_name = fork_name.trim().to_string();
    if fork_name.is_empty()
        || !fork_name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(ApiError::BadRequest(
            "repository name may only contain letters, numbers, '-', '_', '.'".into(),
        ));
    }
    // The fork lives under the forking user's namespace.
    let dst_owner = user.username.clone();
    if state
        .db
        .get_repository_by_name(&dst_owner, &fork_name)
        .await?
        .is_some()
    {
        return Err(ApiError::Conflict(
            "destination repository already exists".into(),
        ));
    }

    state
        .git
        .fork_repo(owner, repo, &dst_owner, &fork_name)
        .map_err(ApiError::from)?;

    let repo = state
        .db
        .create_repository(
            user.id,
            &dst_owner,
            &fork_name,
            &source.description,
            // The fork inherits the source repository's visibility. Creating
            // the fork as public would republish a private repository's
            // whole history to the world.
            source.is_private,
            &source.default_branch,
            None,
        )
        .await?;

    state.bus.publish(
        EventType::RepoCreated,
        Some(repo.id),
        Some(user.id),
        serde_json::json!({
            "owner": dst_owner,
            "repo": fork_name,
            "action": "forked",
            "source": format!("{}/{}", owner, source.name),
        }),
    );

    Ok(Json(RepoResponse::from(repo)))
}

async fn create_branch(
    state: &AppState,
    user: &AuthUserExtractor,
    owner: &str,
    repo: &str,
    req: CreateBranchRequest,
) -> ApiResult<Json<serde_json::Value>> {
    crate::routes::visibility::load_pushable_repo(state, owner, repo, user).await?;
    let branch = req.name.trim().to_string();
    if branch.is_empty() {
        return Err(ApiError::BadRequest("branch name is required".into()));
    }
    let start_sha = match req.r#ref.as_deref() {
        Some(r) if !r.is_empty() => Some(
            state
                .git
                .head_sha(owner, repo, r)
                .map_err(|_| ApiError::BadRequest(format!("ref not found: {r}")))?,
        ),
        _ => None,
    };
    state
        .git
        .create_branch(owner, repo, &branch, start_sha.as_deref())
        .map_err(ApiError::from)?;
    Ok(Json(serde_json::json!({ "branch": branch })))
}

async fn delete_branch(
    state: &AppState,
    user: &AuthUserExtractor,
    owner: &str,
    repo: &str,
    branch: &str,
) -> ApiResult<Json<crate::error::MessageResponse>> {
    crate::routes::visibility::load_pushable_repo(state, owner, repo, user).await?;
    state
        .git
        .delete_branch(owner, repo, branch)
        .map_err(ApiError::from)?;
    Ok(Json(crate::error::MessageResponse {
        message: format!("branch {branch} deleted"),
    }))
}

async fn list_tags(
    state: &AppState,
    user: &OptionalAuthUser,
    owner: &str,
    repo: &str,
) -> ApiResult<Json<Vec<String>>> {
    let repo = crate::routes::visibility::load_readable_repo(state, owner, repo, user.0.as_ref())
        .await?;
    let tags = state
        .git
        .list_tags(owner, &repo.name)
        .map_err(ApiError::from)?;
    Ok(Json(tags))
}

async fn create_tag(
    state: &AppState,
    user: &AuthUserExtractor,
    owner: &str,
    repo: &str,
    req: CreateTagRequest,
) -> ApiResult<Json<serde_json::Value>> {
    crate::routes::visibility::load_pushable_repo(state, owner, repo, user).await?;
    let tag = req.name.trim().to_string();
    if tag.is_empty() {
        return Err(ApiError::BadRequest("tag name is required".into()));
    }
    let start_sha = match req.r#ref.as_deref() {
        Some(r) if !r.is_empty() => Some(
            state
                .git
                .head_sha(owner, repo, r)
                .map_err(|_| ApiError::BadRequest(format!("ref not found: {r}")))?,
        ),
        _ => None,
    };
    state
        .git
        .create_tag(owner, repo, &tag, start_sha.as_deref())
        .map_err(ApiError::from)?;
    Ok(Json(serde_json::json!({ "tag": tag })))
}

async fn delete_tag(
    state: &AppState,
    user: &AuthUserExtractor,
    owner: &str,
    repo: &str,
    tag: &str,
) -> ApiResult<Json<crate::error::MessageResponse>> {
    crate::routes::visibility::load_pushable_repo(state, owner, repo, user).await?;
    state
        .git
        .delete_tag(owner, repo, tag)
        .map_err(ApiError::from)?;
    Ok(Json(crate::error::MessageResponse {
        message: format!("tag {tag} deleted"),
    }))
}

// ---------------------------------------------------------------------------
// Webhooks
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
pub struct CreateHookRequest {
    pub url: String,
    pub events: Option<Vec<String>>,
    pub secret: Option<String>,
    pub active: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct HookResponse {
    pub id: i64,
    pub url: String,
    pub events: Vec<String>,
    pub secret: String,
    pub active: bool,
    pub created_at: String,
}

impl From<gita_db::WebhookRow> for HookResponse {
    fn from(h: gita_db::WebhookRow) -> Self {
        Self {
            id: h.id,
            url: h.url,
            events: h.events,
            secret: h.secret,
            active: h.active,
            created_at: h.created_at,
        }
    }
}

async fn create_hook(
    state: &AppState,
    user: &AuthUserExtractor,
    owner: &str,
    repo: &str,
    req: CreateHookRequest,
) -> ApiResult<Json<HookResponse>> {
    let repository = crate::routes::visibility::load_adminable_repo(state, owner, repo, user).await?;
    let url = req.url.trim().to_string();
    if url.is_empty() || !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(ApiError::BadRequest(
            "webhook URL must be http(s)://...".into(),
        ));
    }
    let events = req.events.unwrap_or_else(|| vec!["push".to_string()]);
    if events.is_empty() {
        return Err(ApiError::BadRequest(
            "at least one event must be specified".into(),
        ));
    }
    let hook = state
        .db
        .create_webhook(
            repository.id,
            &url,
            &events,
            req.secret.unwrap_or_default().as_str(),
            req.active.unwrap_or(true),
        )
        .await?;
    Ok(Json(HookResponse::from(hook)))
}

async fn list_hooks(
    state: &AppState,
    user: &OptionalAuthUser,
    owner: &str,
    repo: &str,
) -> ApiResult<Json<serde_json::Value>> {
    let auth = user
        .0
        .as_ref()
        .ok_or_else(|| ApiError::Unauthorized("authentication required".into()))?;
    let repository = crate::routes::visibility::load_adminable_repo(state, owner, repo, auth).await?;
    let hooks = state.db.list_webhooks(repository.id).await?;
    Ok(Json(serde_json::json!({
        "hooks": hooks.into_iter().map(HookResponse::from).collect::<Vec<_>>(),
    })))
}

async fn delete_hook(
    state: &AppState,
    user: &AuthUserExtractor,
    owner: &str,
    repo: &str,
    id: i64,
) -> ApiResult<Json<crate::error::MessageResponse>> {
    let repository = crate::routes::visibility::load_adminable_repo(state, owner, repo, user).await?;
    let deleted = state.db.delete_webhook(id, repository.id).await?;
    if !deleted {
        return Err(ApiError::NotFound("webhook not found".into()));
    }
    Ok(Json(crate::error::MessageResponse {
        message: "webhook deleted".to_string(),
    }))
}

//! Repository routes: create, list, get, delete, browse tree/files/commits.

use crate::error::{ApiError, ApiResult};
use crate::extract::{AuthUserExtractor, OptionalAuthUser};
use crate::state::AppState;
use axum::extract::{Path, Query, State};
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

#[derive(Debug, Deserialize)]
pub struct TreeQuery {
    pub path: Option<String>,
    pub r#ref: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CommitsQuery {
    pub r#ref: Option<String>,
    pub limit: Option<usize>,
}

pub fn repo_routes() -> Router<AppState> {
    Router::new()
        .route("/api/repos", post(create_repo).get(list_repos))
        .route("/api/repos/{owner}/{repo}", get(get_repo).delete(delete_repo).patch(update_repo))
        .route("/api/repos/{owner}/{repo}/tree", get(get_tree))
        .route("/api/repos/{owner}/{repo}/file", get(get_file))
        .route("/api/repos/{owner}/{repo}/commits", get(list_commits))
        .route("/api/repos/{owner}/{repo}/commits/{sha}", get(get_commit))
        .route(
            "/api/repos/{owner}/{repo}/star",
            axum::routing::put(put_star).delete(delete_star).get(get_star),
        )
        .route(
            "/api/repos/{owner}/{repo}/fork",
            axum::routing::post(post_fork),
        )
        .route(
            "/api/repos/{owner}/{repo}/hooks",
            get(list_hooks).post(create_hook),
        )
        .route(
            "/api/repos/{owner}/{repo}/hooks/{id}",
            axum::routing::delete(delete_hook),
        )
        .route("/api/repos/{owner}/{repo}/branches", get(list_branches).post(create_branch))
        .route(
            "/api/repos/{owner}/{repo}/branches/{branch}",
            axum::routing::delete(delete_branch),
        )
        .route("/api/repos/{owner}/{repo}/tags", get(list_tags).post(create_tag))
        .route(
            "/api/repos/{owner}/{repo}/tags/{tag}",
            axum::routing::delete(delete_tag),
        )
}

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
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.') {
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
    let repos = state.db.list_repositories(q.owner.as_deref(), limit).await?;
    // Filter out private repositories the caller cannot see. The list is
    // not owner-scoped, so without this an anonymous caller learns that a
    // private repository exists.
    let visible: Vec<RepoResponse> = repos
        .into_iter()
        .filter(|r| crate::routes::visibility::can_read(r, user.0.as_ref()))
        .map(RepoResponse::from)
        .collect();
    Ok(Json(visible))
}

async fn get_repo(
    State(state): State<AppState>,
    user: OptionalAuthUser,
    Path((owner, repo)): Path<(String, String)>,
) -> ApiResult<Json<RepoResponse>> {
    let repo = crate::routes::visibility::load_readable_repo(
        &state,
        &owner,
        &repo,
        user.0.as_ref(),
    )
    .await?;
    Ok(Json(RepoResponse::from(repo)))
}

async fn delete_repo(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path((owner, repo)): Path<(String, String)>,
) -> ApiResult<Json<crate::error::MessageResponse>> {
    if owner != user.username {
        return Err(ApiError::Forbidden("only the owner can delete this repo".into()));
    }
    let repo = state
        .db
        .get_repository_by_name(&owner, &repo)
        .await?
        .ok_or_else(|| ApiError::NotFound("repository not found".into()))?;
    state.db.delete_repository(repo.id).await?;
    state.git.delete_repo(&owner, &repo.name)?;
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

async fn update_repo(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path((owner, repo)): Path<(String, String)>,
    Json(req): Json<UpdateRepoRequest>,
) -> ApiResult<Json<RepoResponse>> {
    let mut repository = state
        .db
        .get_repository_by_name(&owner, &repo)
        .await?
        .ok_or_else(|| ApiError::NotFound("repository not found".into()))?;
    if repository.owner_id != user.id {
        return Err(ApiError::Forbidden("only the repository owner can update this repo".into()));
    }

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
            return Err(ApiError::Conflict("target repository already exists".into()));
        }
        state
            .git
            .rename_repo(&owner, &repo, &repository.owner_name, &new_name)
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
            return Err(ApiError::Conflict("target repository already exists".into()));
        }

        // Move the bare repo on disk.
        state
            .git
            .rename_repo(&owner, &repo, &new_owner_name, &repository.name)
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
    State(state): State<AppState>,
    user: OptionalAuthUser,
    Path((owner, repo)): Path<(String, String)>,
    Query(q): Query<TreeQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let repo = crate::routes::visibility::load_readable_repo(
        &state,
        &owner,
        &repo,
        user.0.as_ref(),
    )
    .await?;
    let reference = q.r#ref.unwrap_or_else(|| repo.default_branch.clone());
    let path = q.path.unwrap_or_default();
    let entries = state
        .git
        .list_tree(&owner, &repo.name, &reference, &path)
        .map_err(ApiError::from)?;
    Ok(Json(serde_json::json!({
        "path": path,
        "ref": reference,
        "entries": entries,
    })))
}

async fn get_file(
    State(state): State<AppState>,
    user: OptionalAuthUser,
    Path((owner, repo)): Path<(String, String)>,
    Query(q): Query<TreeQuery>,
) -> ApiResult<Json<gita_core::FileContent>> {
    let repo = crate::routes::visibility::load_readable_repo(
        &state,
        &owner,
        &repo,
        user.0.as_ref(),
    )
    .await?;
    let reference = q.r#ref.unwrap_or_else(|| repo.default_branch.clone());
    let path = q.path.ok_or_else(|| ApiError::BadRequest("missing 'path' query".into()))?;
    let file = state
        .git
        .read_file(&owner, &repo.name, &reference, &path)
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::NotFound("file not found".into()))?;
    Ok(Json(file))
}

async fn list_commits(
    State(state): State<AppState>,
    user: OptionalAuthUser,
    Path((owner, repo)): Path<(String, String)>,
    Query(q): Query<CommitsQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let repo = crate::routes::visibility::load_readable_repo(
        &state,
        &owner,
        &repo,
        user.0.as_ref(),
    )
    .await?;
    let reference = q.r#ref.unwrap_or_else(|| repo.default_branch.clone());
    let limit = q.limit.unwrap_or(30).min(200);
    let commits = state
        .git
        .list_commits(&owner, &repo.name, &reference, limit)
        .map_err(ApiError::from)?;
    Ok(Json(serde_json::json!({
        "ref": reference,
        "commits": commits,
    })))
}

async fn get_commit(
    State(state): State<AppState>,
    user: OptionalAuthUser,
    Path((owner, repo, sha)): Path<(String, String, String)>,
) -> ApiResult<Json<serde_json::Value>> {
    let repository = crate::routes::visibility::load_readable_repo(
        &state,
        &owner,
        &repo,
        user.0.as_ref(),
    )
    .await?;
    let commit = state
        .git
        .get_commit(&owner, &repo, &sha)
        .map_err(ApiError::from)?;
    let diff = state
        .git
        .show_commit(&owner, &repo, &commit.sha)
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
    State(state): State<AppState>,
    user: OptionalAuthUser,
    Path((owner, repo)): Path<(String, String)>,
) -> ApiResult<Json<Vec<String>>> {
    let repo = crate::routes::visibility::load_readable_repo(
        &state,
        &owner,
        &repo,
        user.0.as_ref(),
    )
    .await?;
    let branches = state
        .git
        .list_branches(&owner, &repo.name)
        .map_err(ApiError::from)?;
    Ok(Json(branches))
}

async fn put_star(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path((owner, repo)): Path<(String, String)>,
) -> ApiResult<Json<serde_json::Value>> {
    let repository = state
        .db
        .get_repository_by_name(&owner, &repo)
        .await?
        .ok_or_else(|| ApiError::NotFound("repository not found".into()))?;
    let inserted = state
        .db
        .add_star(user.id, repository.id)
        .await?;
    let total = state.db.count_stars(repository.id).await?;
    Ok(Json(serde_json::json!({
        "starred": true,
        "new": inserted,
        "count": total,
    })))
}

async fn delete_star(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path((owner, repo)): Path<(String, String)>,
) -> ApiResult<Json<serde_json::Value>> {
    let repository = state
        .db
        .get_repository_by_name(&owner, &repo)
        .await?
        .ok_or_else(|| ApiError::NotFound("repository not found".into()))?;
    let removed = state.db.remove_star(user.id, repository.id).await?;
    let total = state.db.count_stars(repository.id).await?;
    Ok(Json(serde_json::json!({
        "starred": false,
        "removed": removed,
        "count": total,
    })))
}

async fn get_star(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path((owner, repo)): Path<(String, String)>,
) -> ApiResult<Json<serde_json::Value>> {
    let repository = state
        .db
        .get_repository_by_name(&owner, &repo)
        .await?
        .ok_or_else(|| ApiError::NotFound("repository not found".into()))?;
    let starred = state.db.has_star(user.id, repository.id).await?;
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
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path((owner, repo)): Path<(String, String)>,
    Json(req): Json<ForkRequest>,
) -> ApiResult<Json<RepoResponse>> {
    let source = state
        .db
        .get_repository_by_name(&owner, &repo)
        .await?
        .ok_or_else(|| ApiError::NotFound("source repository not found".into()))?;
    let fork_name = req.name.unwrap_or_else(|| source.name.clone());
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
        return Err(ApiError::Conflict("destination repository already exists".into()));
    }

    state
        .git
        .fork_repo(&owner, &repo, &dst_owner, &fork_name)
        .map_err(ApiError::from)?;

    let repo = state
        .db
        .create_repository(
            user.id,
            &dst_owner,
            &fork_name,
            &source.description,
            false,
            &source.default_branch,
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
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path((owner, repo)): Path<(String, String)>,
    Json(req): Json<CreateBranchRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let repository = state
        .db
        .get_repository_by_name(&owner, &repo)
        .await?
        .ok_or_else(|| ApiError::NotFound("repository not found".into()))?;
    if repository.owner_id != user.id {
        return Err(ApiError::Forbidden("only the repository owner can manage branches".into()));
    }
    let branch = req.name.trim().to_string();
    if branch.is_empty() {
        return Err(ApiError::BadRequest("branch name is required".into()));
    }
    let start_sha = match req.r#ref.as_deref() {
        Some(r) if !r.is_empty() => {
            Some(
                state
                    .git
                    .head_sha(&owner, &repo, r)
                    .map_err(|_| ApiError::BadRequest(format!("ref not found: {r}")))?,
            )
        }
        _ => None,
    };
    state
        .git
        .create_branch(&owner, &repo, &branch, start_sha.as_deref())
        .map_err(ApiError::from)?;
    Ok(Json(serde_json::json!({ "branch": branch })))
}

async fn delete_branch(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path((owner, repo, branch)): Path<(String, String, String)>,
) -> ApiResult<Json<crate::error::MessageResponse>> {
    let repository = state
        .db
        .get_repository_by_name(&owner, &repo)
        .await?
        .ok_or_else(|| ApiError::NotFound("repository not found".into()))?;
    if repository.owner_id != user.id {
        return Err(ApiError::Forbidden("only the repository owner can manage branches".into()));
    }
    state
        .git
        .delete_branch(&owner, &repo, &branch)
        .map_err(ApiError::from)?;
    Ok(Json(crate::error::MessageResponse {
        message: format!("branch {branch} deleted"),
    }))
}

async fn list_tags(
    State(state): State<AppState>,
    user: OptionalAuthUser,
    Path((owner, repo)): Path<(String, String)>,
) -> ApiResult<Json<Vec<String>>> {
    let repo = crate::routes::visibility::load_readable_repo(
        &state,
        &owner,
        &repo,
        user.0.as_ref(),
    )
    .await?;
    let tags = state
        .git
        .list_tags(&owner, &repo.name)
        .map_err(ApiError::from)?;
    Ok(Json(tags))
}

async fn create_tag(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path((owner, repo)): Path<(String, String)>,
    Json(req): Json<CreateTagRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let repository = state
        .db
        .get_repository_by_name(&owner, &repo)
        .await?
        .ok_or_else(|| ApiError::NotFound("repository not found".into()))?;
    if repository.owner_id != user.id {
        return Err(ApiError::Forbidden("only the repository owner can manage tags".into()));
    }
    let tag = req.name.trim().to_string();
    if tag.is_empty() {
        return Err(ApiError::BadRequest("tag name is required".into()));
    }
    let start_sha = match req.r#ref.as_deref() {
        Some(r) if !r.is_empty() => {
            Some(
                state
                    .git
                    .head_sha(&owner, &repo, r)
                    .map_err(|_| ApiError::BadRequest(format!("ref not found: {r}")))?,
            )
        }
        _ => None,
    };
    state
        .git
        .create_tag(&owner, &repo, &tag, start_sha.as_deref())
        .map_err(ApiError::from)?;
    Ok(Json(serde_json::json!({ "tag": tag })))
}

async fn delete_tag(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path((owner, repo, tag)): Path<(String, String, String)>,
) -> ApiResult<Json<crate::error::MessageResponse>> {
    let repository = state
        .db
        .get_repository_by_name(&owner, &repo)
        .await?
        .ok_or_else(|| ApiError::NotFound("repository not found".into()))?;
    if repository.owner_id != user.id {
        return Err(ApiError::Forbidden("only the repository owner can manage tags".into()));
    }
    state
        .git
        .delete_tag(&owner, &repo, &tag)
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
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path((owner, repo)): Path<(String, String)>,
    Json(req): Json<CreateHookRequest>,
) -> ApiResult<Json<HookResponse>> {
    let repository = state
        .db
        .get_repository_by_name(&owner, &repo)
        .await?
        .ok_or_else(|| ApiError::NotFound("repository not found".into()))?;
    if repository.owner_id != user.id {
        return Err(ApiError::Forbidden(
            "only the repository owner can manage webhooks".into(),
        ));
    }
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
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path((owner, repo)): Path<(String, String)>,
) -> ApiResult<Json<serde_json::Value>> {
    let repository = state
        .db
        .get_repository_by_name(&owner, &repo)
        .await?
        .ok_or_else(|| ApiError::NotFound("repository not found".into()))?;
    if repository.owner_id != user.id {
        return Err(ApiError::Forbidden(
            "only the repository owner can view webhooks".into(),
        ));
    }
    let hooks = state.db.list_webhooks(repository.id).await?;
    Ok(Json(serde_json::json!({
        "hooks": hooks.into_iter().map(HookResponse::from).collect::<Vec<_>>(),
    })))
}

async fn delete_hook(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path((owner, repo, id)): Path<(String, String, i64)>,
) -> ApiResult<Json<crate::error::MessageResponse>> {
    let repository = state
        .db
        .get_repository_by_name(&owner, &repo)
        .await?
        .ok_or_else(|| ApiError::NotFound("repository not found".into()))?;
    if repository.owner_id != user.id {
        return Err(ApiError::Forbidden(
            "only the repository owner can manage webhooks".into(),
        ));
    }
    let deleted = state.db.delete_webhook(id, repository.id).await?;
    if !deleted {
        return Err(ApiError::NotFound("webhook not found".into()));
    }
    Ok(Json(crate::error::MessageResponse {
        message: "webhook deleted".to_string(),
    }))
}

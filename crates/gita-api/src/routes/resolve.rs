//! SPA URL path resolution.
//!
//! A clean URL such as `/gita/tests` is inherently ambiguous: it can be the
//! repository `tests` under the namespace `gita`, or the group whose full
//! path is `gita/tests`. The URL shape alone cannot distinguish the two, so
//! the SPA asks this endpoint once and renders whatever the server says.
//!
//! The rule is deterministic and ordered:
//!
//! 1. Try the path as a *repository*: the last segment is the repository
//!    name, everything before it the owner (a username or a group full
//!    path). Read access is enforced — a private repository the caller
//!    cannot see is a miss here, exactly as in every other REST read.
//! 2. Otherwise try the path as a *group* full path (`gita`,
//!    `gita/tests`, `gita/tests/deep`, ...).
//! 3. Otherwise 404.
//!
//! A repository wins over a group when both could match, mirroring the git
//! host convention where `/owner/name` is a project URL. The creation
//! endpoints refuse to give a subgroup the same name as a repository in the
//! same namespace (and vice versa), so the overlap should not arise in the
//! first place — the ordering is only the tie-breaker for namespaces that
//! predate that rule.

use crate::error::{ApiError, ApiResult};
use crate::extract::OptionalAuthUser;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

/// What kind of entity owns a repository namespace.
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OwnerKind {
    User,
    Group,
}

/// The result of resolving a clean URL path.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResolvedPath {
    /// The path addresses a repository.
    Repo {
        /// Owner namespace: a username or a (possibly nested) group path.
        owner: String,
        name: String,
        /// Whether `owner` names a group; the UI uses this to shape clone
        /// URL fallbacks and the "back to namespace" navigation.
        owner_kind: OwnerKind,
    },
    /// The path addresses a group (possibly nested).
    Group {
        full_path: String,
    },
}

pub fn resolve_routes() -> Router<AppState> {
    Router::new().route("/api/resolve/{*path}", get(resolve))
}

/// GET /api/resolve/{*path} — resolve a clean URL path to the entity it
/// addresses. See the module docs for the resolution order.
async fn resolve(
    State(state): State<AppState>,
    user: OptionalAuthUser,
    Path(path): Path<String>,
) -> ApiResult<Json<ResolvedPath>> {
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    // Two or more segments can address a repository: `owner.../name`.
    // One segment alone cannot: a bare username has no page.
    if segments.len() >= 2 {
        let name = segments[segments.len() - 1];
        let owner = segments[..segments.len() - 1].join("/");
        if let Ok(repo) =
            crate::routes::visibility::load_readable_repo(&state, &owner, name, user.0.as_ref())
                .await
        {
            let owner_kind = if state
                .db
                .get_group_by_full_path(&owner)
                .await?
                .is_some()
            {
                OwnerKind::Group
            } else {
                OwnerKind::User
            };
            return Ok(Json(ResolvedPath::Repo {
                owner: repo.owner_name,
                name: repo.name,
                owner_kind,
            }));
        }
    }

    if let Some(g) = state.db.get_group_by_full_path(&path).await? {
        return Ok(Json(ResolvedPath::Group {
            full_path: g.full_path,
        }));
    }

    Err(ApiError::NotFound(
        "no repository or group matches this path".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolved_path_serde_uses_snake_case() {
        let repo = ResolvedPath::Repo {
            owner: "alice".to_string(),
            name: "widgets".to_string(),
            owner_kind: OwnerKind::User,
        };
        let json = serde_json::to_value(&repo).unwrap();
        assert_eq!(json["kind"], "repo");
        assert_eq!(json["owner"], "alice");
        assert_eq!(json["name"], "widgets");
        assert_eq!(json["owner_kind"], "user");

        let group = ResolvedPath::Group {
            full_path: "gita/tests".to_string(),
        };
        let json = serde_json::to_value(&group).unwrap();
        assert_eq!(json["kind"], "group");
        assert_eq!(json["full_path"], "gita/tests");
    }

    #[test]
    fn owner_kind_serde_uses_snake_case() {
        assert_eq!(serde_json::to_string(&OwnerKind::User).unwrap(), "\"user\"");
        assert_eq!(
            serde_json::to_string(&OwnerKind::Group).unwrap(),
            "\"group\""
        );
    }
}

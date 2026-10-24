//! Serde DTOs mirroring the wire shapes in `web/src/lib/api.ts` and the
//! route response structs in `crates/gita-api/src/routes/*.rs`.
//!
//! Defined locally so the client stays decoupled from server internals
//! (no axum/sqlx dependency). Only `Deserialize` (request bodies add
//! `Serialize`).

use serde::{Deserialize, Serialize};
#[derive(Debug, Deserialize, Serialize)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub email: String,
    pub locale: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Repository {
    pub id: i64,
    pub owner: String,
    pub name: String,
    pub full_name: String,
    pub description: String,
    pub is_private: bool,
    pub default_branch: String,
    pub created_at: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TreeEntry {
    pub path: String,
    pub name: String,
    pub kind: String,
    pub size: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Commit {
    pub id: i64,
    pub repo_id: i64,
    pub sha: String,
    pub author_name: String,
    pub author_email: String,
    pub message: String,
    pub parent_shas: Vec<String>,
    pub authored_at: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct CommitDetail {
    pub sha: String,
    pub author_name: String,
    pub author_email: String,
    pub message: String,
    pub parent_shas: Vec<String>,
    pub authored_at: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct FileContent {
    pub path: String,
    pub content: String,
    pub sha: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PullRequest {
    pub id: i64,
    pub number: i64,
    pub title: String,
    pub body: String,
    pub source_branch: String,
    pub target_branch: String,
    pub status: String,
    pub author: String,
    pub base_sha: String,
    pub head_sha: String,
    pub created_at: String,
    pub updated_at: String,
    pub conflicts: Vec<String>,
    pub merged_at: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Issue {
    pub id: i64,
    pub number: i64,
    pub title: String,
    pub body: String,
    pub status: String,
    pub author: String,
    pub created_at: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Comment {
    pub id: i64,
    pub repo_id: i64,
    pub pr_id: Option<i64>,
    pub issue_id: Option<i64>,
    pub author: String,
    pub body: String,
    pub created_at: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Token {
    pub id: i64,
    pub token: String,
    pub name: String,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SshKey {
    pub id: i64,
    pub title: String,
    pub public_key: String,
    pub created_at: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SshKeysResponse {
    pub keys: Vec<SshKey>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RepoSearchHit {
    pub id: i64,
    pub owner: String,
    pub name: String,
    pub full_name: String,
    pub description: String,
    pub is_private: bool,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct CodeSearchHit {
    pub owner: String,
    pub repo: String,
    pub path: String,
    pub line: i64,
    pub content: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Group {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub owner_id: i64,
    pub created_at: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct GlobalIssueItem {
    pub id: i64,
    pub number: i64,
    pub title: String,
    pub body: String,
    pub status: String,
    pub author: String,
    pub repo_owner: String,
    pub repo_name: String,
    pub full_name: String,
    pub created_at: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct EventItem {
    pub id: i64,
    pub event_type: String,
    pub repo_id: Option<i64>,
    pub actor_id: Option<i64>,
    pub payload: serde_json::Value,
    pub created_at: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Pipeline {
    pub id: i64,
    pub repo_id: i64,
    pub path: String,
    pub r#ref: String,
    pub ref_sha: String,
    pub content: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PipelineRun {
    pub id: i64,
    pub pipeline_id: i64,
    pub repo_id: i64,
    pub r#ref: String,
    pub head_sha: String,
    pub trigger_event_id: Option<i64>,
    pub trigger_pusher_id: Option<i64>,
    pub status: String,
    pub queued_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Job {
    pub id: i64,
    pub run_id: i64,
    pub name: String,
    pub stage: String,
    pub needs: Vec<String>,
    pub script: Vec<String>,
    pub status: String,
    pub queued_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub exit_code: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct JobLogLine {
    pub seq: i64,
    pub stream: String,
    pub line: String,
    pub ts: String,
}

// ---------------------------------------------------------------------------
// Response wrapper shapes (the API wraps collections in objects)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Serialize)]
pub struct AuthResponse {
    pub token: String,
    pub user: User,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PrsResponse {
    pub prs: Vec<PullRequest>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct IssuesResponse {
    pub issues: Vec<Issue>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct CommentsResponse {
    pub comments: Vec<Comment>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct CommitsResponse {
    pub r#ref: String,
    pub commits: Vec<Commit>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TreeResponse {
    pub path: String,
    pub r#ref: String,
    pub entries: Vec<TreeEntry>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RunsResponse {
    pub runs: Vec<PipelineRun>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RunDetailResponse {
    pub run: PipelineRun,
    pub jobs: Vec<Job>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct LogsResponse {
    pub lines: Vec<JobLogLine>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct HitsResponse<T> {
    pub hits: Vec<T>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct EventsResponse {
    pub events: Vec<EventItem>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct MessageResponse {
    pub message: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct StarResponse {
    pub starred: bool,
    pub count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub removed: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ForkResponse {
    pub message: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct GroupView {
    pub group: Group,
    pub repos: Vec<Repository>,
}

// ---------------------------------------------------------------------------
// Request bodies (serialized)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct RegisterRequest {
    pub username: String,
    pub email: String,
    pub password: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct CreateTokenRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CreateRepoRequest {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_private: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_branch: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CreateBranchRequest {
    pub name: String,
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    pub r#ref: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CreateTagRequest {
    pub name: String,
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    pub r#ref: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CreatePrRequest {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    pub source_branch: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_branch: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CreateIssueRequest {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CreateCommentRequest {
    pub body: String,
}

#[derive(Debug, Serialize)]
pub struct CreateGroupRequest {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CreateGroupRepoRequest {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_private: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_branch: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_repository() {
        let json = r#"{"id":1,"owner":"alice","name":"demo","full_name":"alice/demo","description":"","is_private":false,"default_branch":"main","created_at":"2026-10-23T00:00:00Z"}"#;
        let r: Repository = serde_json::from_str(json).unwrap();
        assert_eq!(r.id, 1);
        assert_eq!(r.full_name, "alice/demo");
        assert!(!r.is_private);
    }

    #[test]
    fn deserialize_prs_response() {
        let json = r#"{"prs":[{"id":1,"number":2,"title":"t","body":"b","source_branch":"feat","target_branch":"main","status":"open","author":"alice","base_sha":"a","head_sha":"b","created_at":"2026-10-23T00:00:00Z","updated_at":"2026-10-23T00:00:00Z","conflicts":[],"merged_at":null}]}"#;
        let v: PrsResponse = serde_json::from_str(json).unwrap();
        assert_eq!(v.prs.len(), 1);
        assert_eq!(v.prs[0].number, 2);
    }

    #[test]
    fn deserialize_events_response() {
        let json = r#"{"events":[{"id":1,"event_type":"push","repo_id":1,"actor_id":1,"payload":{},"created_at":"2026-10-23T00:00:00Z"}]}"#;
        let v: EventsResponse = serde_json::from_str(json).unwrap();
        assert_eq!(v.events.len(), 1);
        assert_eq!(v.events[0].event_type, "push");
    }

    #[test]
    fn deserialize_error_envelope() {
        let json = r#"{"error":{"message":"invalid credentials","status":401}}"#;
        let env: crate::api::ErrorEnvelope = serde_json::from_str(json).unwrap();
        assert_eq!(env.error.message, "invalid credentials");
        assert_eq!(env.error.status, Some(401));
    }
}

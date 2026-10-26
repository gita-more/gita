//! Core domain models for Gita.
//!
//! These types are shared across all crates. They describe the
//! fundamental entities of an AI-native git server: users, repositories,
//! commits, pull requests, issues and events.
//!
//! The repository access-control rule lives in [`access`].

pub mod access;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

/// A registered user of the Gita server.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub email: String,
    /// Argon2 hash of the password (never the plain text).
    pub password_hash: String,
    pub created_at: DateTime<Utc>,
    /// UI language preference ("zh" | "en").
    pub locale: String,
    /// Whether this account is a system administrator, allowed to manage
    /// other users from the admin settings page.
    pub is_admin: bool,
}

/// A group (namespace) that can own repositories.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Group {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub owner_id: i64,
    pub created_at: DateTime<Utc>,
    /// Parent group ID, if this is a subgroup.
    pub parent_id: Option<i64>,
    /// Full path from root group, e.g. "groupa/subgroupa".
    pub full_path: String,
}

/// A member of a group, with their role.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GroupMember {
    pub group_id: i64,
    pub user_id: i64,
    pub username: String,
    pub role: String,
}

/// A repository (bare git repository) hosted on Gita.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Repository {
    pub id: i64,
    /// Owner user id.
    ///
    /// For a repository in a group namespace this is the id of the user
    /// who created the group — see [`crate::access`] for how that affects
    /// who may access the repository.
    pub owner_id: i64,
    /// Owner username, denormalized for convenient display.
    pub owner_name: String,
    pub name: String,
    pub description: String,
    pub is_private: bool,
    pub default_branch: String,
    pub created_at: DateTime<Utc>,
    /// The group this repository belongs to, or `None` for a personal
    /// repository.
    pub group_id: Option<i64>,
}

impl Repository {
    /// The full path in the form `owner/name`.
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner_name, self.name)
    }
}

/// A git commit, denormalized into the database for querying.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Commit {
    pub id: i64,
    pub repo_id: i64,
    /// The git object SHA (hex, 40 chars).
    pub sha: String,
    pub author_name: String,
    pub author_email: String,
    pub message: String,
    /// Parent SHAs, in order.
    pub parent_shas: Vec<String>,
    pub authored_at: DateTime<Utc>,
}

/// Status of a pull request.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PrStatus {
    Open,
    Merged,
    Closed,
}

impl fmt::Display for PrStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PrStatus::Open => write!(f, "open"),
            PrStatus::Merged => write!(f, "merged"),
            PrStatus::Closed => write!(f, "closed"),
        }
    }
}

/// A pull request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PullRequest {
    pub id: i64,
    pub repo_id: i64,
    /// Human readable number within the repository (1, 2, 3, ...).
    pub number: i64,
    pub title: String,
    pub body: String,
    pub source_branch: String,
    pub target_branch: String,
    pub status: PrStatus,
    pub author_id: i64,
    pub author_name: String,
    pub base_sha: String,
    pub head_sha: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub merged_at: Option<DateTime<Utc>>,
}

/// Status of an issue.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum IssueStatus {
    Open,
    Closed,
}

impl fmt::Display for IssueStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IssueStatus::Open => write!(f, "open"),
            IssueStatus::Closed => write!(f, "closed"),
        }
    }
}

/// An issue.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Issue {
    pub id: i64,
    pub repo_id: i64,
    pub number: i64,
    pub title: String,
    pub body: String,
    pub status: IssueStatus,
    pub author_id: i64,
    pub author_name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
}

/// A comment on a pull request or an issue.
///
/// Exactly one of `pr_id` / `issue_id` is `Some`; the other is `None`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Comment {
    pub id: i64,
    pub repo_id: i64,
    pub pr_id: Option<i64>,
    pub issue_id: Option<i64>,
    pub author_id: i64,
    pub author_name: String,
    pub body: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The category of an event in the AI-readable event stream.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    RepoCreated,
    RepoDeleted,
    Push,
    PullRequestOpened,
    PullRequestMerged,
    PullRequestClosed,
    IssueCreated,
    IssueClosed,
    CommentCreated,
    CommentDeleted,
    UserRegistered,
    AgentRunStarted,
    AgentRunCompleted,
    // In-repo CI (Phase 1)
    PipelineRequested,
    PipelineStarted,
    PipelineCompleted,
    PipelineFailed,
    JobStarted,
    JobCompleted,
}

impl fmt::Display for EventType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            EventType::RepoCreated => "repo_created",
            EventType::RepoDeleted => "repo_deleted",
            EventType::Push => "push",
            EventType::PullRequestOpened => "pull_request_opened",
            EventType::PullRequestMerged => "pull_request_merged",
            EventType::PullRequestClosed => "pull_request_closed",
            EventType::IssueCreated => "issue_created",
            EventType::IssueClosed => "issue_closed",
            EventType::CommentCreated => "comment_created",
            EventType::CommentDeleted => "comment_deleted",
            EventType::UserRegistered => "user_registered",
            EventType::AgentRunStarted => "agent_run_started",
            EventType::AgentRunCompleted => "agent_run_completed",
            EventType::PipelineRequested => "pipeline_requested",
            EventType::PipelineStarted => "pipeline_started",
            EventType::PipelineCompleted => "pipeline_completed",
            EventType::PipelineFailed => "pipeline_failed",
            EventType::JobStarted => "job_started",
            EventType::JobCompleted => "job_completed",
        };
        write!(f, "{s}")
    }
}

/// A structured event in the AI-readable event stream.
///
/// The event stream is the heart of the "AI-native" design: every
/// meaningful activity is recorded as an event that AI agents can
/// subscribe to and react upon.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Event {
    pub id: i64,
    pub event_type: EventType,
    pub repo_id: Option<i64>,
    pub actor_id: Option<i64>,
    /// Arbitrary JSON payload describing the event in detail.
    pub payload: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

impl Event {
    /// A new event, stamped with the current time and not yet persisted.
    ///
    /// `id` is 0 until the event has been written to the database; see
    /// `gita_events::EventBus::publish_persisted`.
    pub fn new(
        event_type: EventType,
        repo_id: Option<i64>,
        actor_id: Option<i64>,
        payload: serde_json::Value,
    ) -> Self {
        Self {
            id: 0,
            event_type,
            repo_id,
            actor_id,
            payload,
            created_at: Utc::now(),
        }
    }
}

/// A credential token (personal access token) for API/MCP access.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccessToken {
    pub id: i64,
    pub user_id: i64,
    pub token: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
}

/// Result of listing files in a repository tree.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TreeEntry {
    pub path: String,
    pub name: String,
    /// `"blob"` or `"tree"`.
    pub kind: String,
    pub size: Option<i64>,
}

/// A file at a given commit with its raw content.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileContent {
    pub path: String,
    pub content: String,
    pub sha: String,
}

/// Metadata for a single git commit returned by the API. Used by
/// `GET /api/repos/{owner}/{repo}/commits/{sha}`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommitMeta {
    pub sha: String,
    pub author_name: String,
    pub author_email: String,
    pub message: String,
    pub parent_shas: Vec<String>,
    /// Stored as a string to mirror the rest of the codebase's
    /// RFC3339-text conventions.
    pub authored_at: chrono::DateTime<chrono::Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn user() -> User {
        User {
            id: 1,
            username: "alice".to_string(),
            email: "alice@example.com".to_string(),
            password_hash: "hash".to_string(),
            created_at: Utc::now(),
            locale: "zh".to_string(),
            is_admin: false,
        }
    }

    #[test]
    fn repository_full_name() {
        let repo = Repository {
            id: 1,
            owner_id: 1,
            owner_name: "alice".to_string(),
            name: "hello".to_string(),
            description: "desc".to_string(),
            is_private: false,
            default_branch: "main".to_string(),
            created_at: Utc::now(),
            group_id: None,
        };
        assert_eq!(repo.full_name(), "alice/hello");
    }

    #[test]
    fn pr_status_serde_roundtrip() {
        assert_eq!(serde_json::to_string(&PrStatus::Open).unwrap(), "\"open\"");
        assert_eq!(
            serde_json::to_string(&PrStatus::Merged).unwrap(),
            "\"merged\""
        );
        assert_eq!(
            serde_json::to_string(&PrStatus::Closed).unwrap(),
            "\"closed\""
        );
        assert_eq!(
            serde_json::from_str::<PrStatus>("\"open\"").unwrap(),
            PrStatus::Open
        );
        assert_eq!(PrStatus::Merged.to_string(), "merged");
    }

    #[test]
    fn issue_status_serde_roundtrip() {
        assert_eq!(
            serde_json::to_string(&IssueStatus::Open).unwrap(),
            "\"open\""
        );
        assert_eq!(
            serde_json::to_string(&IssueStatus::Closed).unwrap(),
            "\"closed\""
        );
        assert_eq!(
            serde_json::from_str::<IssueStatus>("\"closed\"").unwrap(),
            IssueStatus::Closed
        );
        assert_eq!(IssueStatus::Open.to_string(), "open");
    }

    #[test]
    fn event_type_display() {
        assert_eq!(EventType::RepoCreated.to_string(), "repo_created");
        assert_eq!(EventType::Push.to_string(), "push");
        assert_eq!(
            EventType::PullRequestMerged.to_string(),
            "pull_request_merged"
        );
        assert_eq!(
            EventType::AgentRunCompleted.to_string(),
            "agent_run_completed"
        );
    }

    #[test]
    fn event_type_serde_roundtrip() {
        assert_eq!(
            serde_json::from_str::<EventType>("\"issue_created\"").unwrap(),
            EventType::IssueCreated
        );
        assert_eq!(
            serde_json::to_string(&EventType::RepoDeleted).unwrap(),
            "\"repo_deleted\""
        );
    }

    #[test]
    fn user_serde_roundtrip() {
        let u = user();
        let json = serde_json::to_value(&u).unwrap();
        assert_eq!(json["username"], "alice");
        let back: User = serde_json::from_value(json).unwrap();
        assert_eq!(back, u);
    }

    #[test]
    fn event_serde_roundtrip() {
        let e = Event {
            id: 1,
            event_type: EventType::Push,
            repo_id: Some(10),
            actor_id: Some(2),
            payload: json!({"branch": "main"}),
            created_at: Utc::now(),
        };
        let json = serde_json::to_value(&e).unwrap();
        let back: Event = serde_json::from_value(json).unwrap();
        assert_eq!(back, e);
    }

    /// A freshly built event carries no database id — 0 is the marker the
    /// persistence hook uses for "not inserted yet".
    #[test]
    fn event_new_starts_without_an_id() {
        let e = Event::new(
            EventType::PipelineRequested,
            Some(10),
            Some(2),
            json!({"run_id": 7}),
        );
        assert_eq!(e.id, 0);
        assert_eq!(e.event_type, EventType::PipelineRequested);
        assert_eq!(e.repo_id, Some(10));
        assert_eq!(e.actor_id, Some(2));
        assert_eq!(e.payload["run_id"], json!(7));
        // Stamped with a time, and that time is not in the future.
        assert!(e.created_at <= Utc::now());
    }

    #[test]
    fn access_token_serde_roundtrip() {
        let t = AccessToken {
            id: 1,
            user_id: 1,
            token: "abc".to_string(),
            name: "default".to_string(),
            created_at: Utc::now(),
        };
        let back: AccessToken = serde_json::from_value(serde_json::to_value(&t).unwrap()).unwrap();
        assert_eq!(back, t);
    }
}

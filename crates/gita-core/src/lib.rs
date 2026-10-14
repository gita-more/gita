//! Core domain models for Gita.
//!
//! These types are shared across all crates. They describe the
//! fundamental entities of an AI-native git server: users, repositories,
//! commits, pull requests, issues and events.

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
}

/// A repository (bare git repository) hosted on Gita.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Repository {
    pub id: i64,
    /// Owner user id.
    pub owner_id: i64,
    /// Owner username, denormalized for convenient display.
    pub owner_name: String,
    pub name: String,
    pub description: String,
    pub is_private: bool,
    pub default_branch: String,
    pub created_at: DateTime<Utc>,
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
    AgentRunStarted,
    AgentRunCompleted,
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
            EventType::AgentRunStarted => "agent_run_started",
            EventType::AgentRunCompleted => "agent_run_completed",
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
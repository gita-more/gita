//! Command-line argument definitions.
//!
//! Mirrors the `Args`/`Command` split from `crates/gita-server/src/lib.rs`:
//! top-level `Args` with a `#[command(subcommand)]`, and `Command` made of
//! nested subcommand enums.

use clap::Parser;
use std::path::PathBuf;

/// gita — a GitHub-gh-like CLI client for the Gita server.
#[derive(Debug, Parser)]
#[command(name = "gita", version, about)]
pub struct Args {
    /// Base URL of the gita-server (overrides GITA_URL and config).
    #[arg(long, global = true, env = "GITA_URL")]
    pub url: Option<String>,

    /// Emit machine-readable JSON.
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, clap::Subcommand)]
pub enum Command {
    /// Authenticate with the server.
    Auth {
        #[command(subcommand)]
        cmd: AuthCommand,
    },
    /// Repositories.
    Repo {
        #[command(subcommand)]
        cmd: RepoCommand,
    },
    /// Pull requests.
    Pr {
        #[command(subcommand)]
        cmd: PrCommand,
    },
    /// Issues.
    Issue {
        #[command(subcommand)]
        cmd: IssueCommand,
    },
    /// Groups.
    Group {
        #[command(subcommand)]
        cmd: GroupCommand,
    },
    /// Search.
    Search {
        #[command(subcommand)]
        cmd: SearchCommand,
    },
    /// CI pipelines.
    Pipeline {
        #[command(subcommand)]
        cmd: PipelineCommand,
    },
    /// SSH public keys.
    SshKey {
        #[command(subcommand)]
        cmd: SshKeyCommand,
    },
    /// Server events.
    Event {
        #[command(subcommand)]
        cmd: EventCommand,
    },
    /// Client configuration.
    Config {
        #[command(subcommand)]
        cmd: ConfigCommand,
    },
}

#[derive(Debug, clap::Subcommand)]
pub enum AuthCommand {
    /// Log in and obtain a JWT.
    Login {
        #[arg(long)]
        url: Option<String>,
    },
    /// Register a new account and obtain a JWT.
    Register {
        #[arg(long)]
        username: Option<String>,
        #[arg(long)]
        email: Option<String>,
        #[arg(long)]
        password: Option<String>,
    },
    /// Log out and revoke the current session.
    Logout,
    /// Show sign-in status.
    Status,
    /// Show the authenticated user.
    Whoami,
    /// Update your profile (locale, email, password).
    Me {
        #[arg(long)]
        locale: Option<String>,
        #[arg(long)]
        email: Option<String>,
        #[arg(long)]
        current_password: Option<String>,
        #[arg(long)]
        new_password: Option<String>,
    },
    /// Personal access token management.
    Token {
        #[command(subcommand)]
        cmd: TokenCommand,
    },
}

#[derive(Debug, clap::Subcommand)]
pub enum TokenCommand {
    /// Create a personal access token (secret shown once).
    Create {
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        store: bool,
    },
    /// List personal access tokens (prefixes only).
    List,
    /// Delete a personal access token.
    Delete {
        #[arg(long)]
        id: i64,
    },
}

#[derive(Debug, clap::Subcommand)]
pub enum RepoCommand {
    /// List repositories.
    List {
        #[arg(long)]
        owner: Option<String>,
    },
    /// View a repository.
    View { owner: String, repo: String },
    /// Create a repository.
    Create {
        name: String,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        default_branch: Option<String>,
        #[arg(long)]
        private: bool,
    },
    /// Edit repository settings (description, visibility, rename, transfer).
    Edit {
        owner: String,
        repo: String,
        #[arg(long)]
        description: Option<String>,
        /// Make the repository private.
        #[arg(long)]
        private: bool,
        /// Make the repository public.
        #[arg(long)]
        public: bool,
        /// Rename the repository.
        #[arg(long)]
        name: Option<String>,
        /// Transfer the repository to another owner.
        #[arg(long)]
        new_owner: Option<String>,
    },
    /// Delete a repository (owner only).
    Delete { owner: String, repo: String },
    /// Clone a repository over git smart HTTP.
    Clone {
        repo: String,
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// List files in a directory.
    Tree {
        owner: String,
        repo: String,
        #[arg(long, default_value = "/")]
        path: String,
        #[arg(long, default_value = "HEAD")]
        r#ref: String,
    },
    /// Read a file at a ref.
    File {
        owner: String,
        repo: String,
        path: String,
        #[arg(long, default_value = "HEAD")]
        r#ref: String,
    },
    /// List branches.
    Branches { owner: String, repo: String },
    /// Create a branch.
    BranchCreate {
        owner: String,
        repo: String,
        name: String,
        /// Ref to branch from (defaults to the default branch).
        #[arg(long)]
        r#ref: Option<String>,
    },
    /// Delete a branch.
    BranchDelete { owner: String, repo: String, branch: String },
    /// List tags.
    Tags { owner: String, repo: String },
    /// Create a tag.
    TagCreate {
        owner: String,
        repo: String,
        name: String,
        /// Ref to tag from (defaults to the default branch).
        #[arg(long)]
        r#ref: Option<String>,
    },
    /// Delete a tag.
    TagDelete { owner: String, repo: String, tag: String },
    /// Get a single commit with its diff.
    Commit {
        owner: String,
        repo: String,
        sha: String,
    },
    /// List recent commits.
    Commits {
        owner: String,
        repo: String,
        #[arg(long)]
        r#ref: Option<String>,
        #[arg(long, default_value_t = 30)]
        limit: usize,
    },
    /// Star a repository.
    Star { owner: String, repo: String },
    /// Unstar a repository.
    Unstar { owner: String, repo: String },
    /// Show whether you have starred a repository.
    StarStatus { owner: String, repo: String },
    /// Fork a repository under your namespace.
    Fork {
        owner: String,
        repo: String,
        #[arg(long)]
        name: Option<String>,
    },
}

#[derive(Debug, clap::Subcommand)]
pub enum PrCommand {
    /// List pull requests.
    List {
        owner: String,
        repo: String,
        #[arg(long)]
        status: Option<String>,
    },
    /// Get a pull request.
    View {
        owner: String,
        repo: String,
        number: i64,
    },
    /// Create a pull request.
    Create {
        owner: String,
        repo: String,
        #[arg(long)]
        title: String,
        #[arg(long)]
        body: Option<String>,
        #[arg(long)]
        source_branch: String,
        #[arg(long)]
        target_branch: Option<String>,
    },
    /// Comment on a pull request.
    Comment {
        owner: String,
        repo: String,
        number: i64,
        #[arg(long)]
        body: String,
    },
    /// Merge a pull request.
    Merge {
        owner: String,
        repo: String,
        number: i64,
    },
    /// Close a pull request.
    Close {
        owner: String,
        repo: String,
        number: i64,
    },
    /// Get a pull request diff.
    Diff {
        owner: String,
        repo: String,
        number: i64,
    },
}

#[derive(Debug, clap::Subcommand)]
pub enum IssueCommand {
    /// List issues. Pass `--all` to list across every repository.
    List {
        owner: Option<String>,
        repo: Option<String>,
        /// List issues across all repositories you can see.
        #[arg(long)]
        all: bool,
        #[arg(long)]
        status: Option<String>,
    },
    /// Get an issue.
    View {
        owner: String,
        repo: String,
        number: i64,
    },
    /// Create an issue.
    Create {
        owner: String,
        repo: String,
        #[arg(long)]
        title: String,
        #[arg(long)]
        body: Option<String>,
    },
    /// Comment on an issue.
    Comment {
        owner: String,
        repo: String,
        number: i64,
        #[arg(long)]
        body: String,
    },
    /// Close an issue.
    Close {
        owner: String,
        repo: String,
        number: i64,
    },
}

#[derive(Debug, clap::Subcommand)]
pub enum GroupCommand {
    /// List groups.
    List,
    /// Create a group.
    Create {
        name: String,
        #[arg(long)]
        description: Option<String>,
    },
    /// View a group and its repositories.
    View { name: String },
    /// List group members.
    Members { name: String },
    /// Add a member to a group.
    MemberAdd {
        group: String,
        username: String,
        #[arg(long)]
        role: Option<String>,
    },
    /// Remove a member from a group.
    MemberRemove { group: String, username: String },
    /// Set a group member's role.
    MemberSetRole {
        group: String,
        username: String,
        #[arg(long)]
        role: String,
    },
    /// Create a repository under a group.
    Repo {
        group: String,
        name: String,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        private: bool,
        #[arg(long)]
        default_branch: Option<String>,
    },
}

#[derive(Debug, clap::Subcommand)]
pub enum SearchCommand {
    /// Search repositories.
    Repos { q: String },
    /// Search code.
    Code { q: String },
}

#[derive(Debug, clap::Subcommand)]
pub enum PipelineCommand {
    /// List pipelines.
    List { owner: String, repo: String },
    /// List runs of a pipeline.
    Runs {
        owner: String,
        repo: String,
        #[arg(long, default_value = ".gita-ci.yml")]
        path: String,
    },
    /// Trigger a pipeline run.
    Trigger {
        owner: String,
        repo: String,
        #[arg(long, default_value = ".gita-ci.yml")]
        path: String,
    },
    /// Get a job's logs.
    Logs {
        owner: String,
        repo: String,
        run: i64,
        job: i64,
        #[arg(long, default_value_t = -1)]
        since: i64,
    },
    /// Cancel a run.
    Cancel {
        owner: String,
        repo: String,
        run: i64,
    },
}

#[derive(Debug, clap::Subcommand)]
pub enum SshKeyCommand {
    /// Add an SSH public key to your account.
    Add {
        #[arg(long)]
        title: Option<String>,
        /// Path to a public key file, or the key content.
        #[arg(value_name = "KEY")]
        key: String,
    },
    /// List your SSH public keys.
    List,
    /// Delete an SSH public key.
    Delete {
        #[arg(long)]
        id: i64,
    },
}

#[derive(Debug, clap::Subcommand)]
pub enum EventCommand {
    /// List AI-readable events.
    List {
        #[arg(long, default_value_t = 50)]
        limit: usize,
        /// Filter to a repository (`owner/name`).
        #[arg(long)]
        repo: Option<String>,
        /// Filter by event type (e.g. push, pr_opened, issue_created).
        #[arg(long)]
        r#type: Option<String>,
    },
}

#[derive(Debug, clap::Subcommand)]
pub enum ConfigCommand {
    /// Set the default server URL in the config file.
    SetUrl { url: String },
    /// Print the resolved configuration.
    Show,
}
